//! Loading the .NET runtime through `hostfxr`.
//!
//! The runtime can be started only once per process, so [`DotnetHost`] is a
//! process-wide singleton. Everything else talks to managed code through
//! function pointers obtained here.

use std::{
    ffi::{CString, c_char, c_int, c_void},
    fmt,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use libloading::Library;

/// `hostfxr_delegate_type::hdt_load_assembly_and_get_function_pointer`.
const HDT_LOAD_ASSEMBLY_AND_GET_FUNCTION_POINTER: c_int = 5;
/// `UNMANAGEDCALLERSONLY_METHOD`: `(const char_t*)-1`.
const UNMANAGEDCALLERSONLY_METHOD: *const c_char = usize::MAX as *const c_char;

type InitializeForRuntimeConfig = unsafe extern "C" fn(
    runtime_config_path: *const c_char,
    parameters: *const InitializeParameters,
    host_context_handle: *mut *mut c_void,
) -> c_int;
type GetRuntimeDelegate = unsafe extern "C" fn(
    host_context_handle: *mut c_void,
    delegate_type: c_int,
    delegate: *mut *mut c_void,
) -> c_int;
type Close = unsafe extern "C" fn(host_context_handle: *mut c_void) -> c_int;
type LoadAssemblyAndGetFunctionPointer = unsafe extern "C" fn(
    assembly_path: *const c_char,
    type_name: *const c_char,
    method_name: *const c_char,
    delegate_type_name: *const c_char,
    reserved: *mut c_void,
    delegate: *mut *mut c_void,
) -> c_int;

#[repr(C)]
struct InitializeParameters {
    size: usize,
    host_path: *const c_char,
    dotnet_root: *const c_char,
}

#[derive(Debug)]
pub enum HostError {
    /// No .NET runtime could be located.
    NotFound(String),
    /// `libhostfxr` or one of its functions could not be loaded.
    Load(String),
    /// A hostfxr call failed with this status code.
    Hostfxr { call: &'static str, code: i32 },
    /// A string passed to the host contained a NUL byte.
    BadString(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m) => write!(f, "no .NET runtime found: {m}"),
            Self::Load(m) => write!(f, "cannot load hostfxr: {m}"),
            Self::Hostfxr { call, code } => {
                write!(f, "{call} failed with status {code:#010x}")
            }
            Self::BadString(s) => write!(f, "string contains NUL: {s:?}"),
        }
    }
}

impl std::error::Error for HostError {}

/// Success codes also include 1 (`HostAlreadyInitialized`) and 2
/// (`DifferentRuntimeProperties`).
fn check(call: &'static str, code: c_int) -> Result<(), HostError> {
    if code >= 0 {
        Ok(())
    } else {
        Err(HostError::Hostfxr { call, code })
    }
}

fn cstring(s: &str) -> Result<CString, HostError> {
    CString::new(s).map_err(|_| HostError::BadString(s.to_owned()))
}

fn path_cstring(p: &Path) -> Result<CString, HostError> {
    cstring(&p.to_string_lossy())
}

/// The root of a .NET installation (the directory holding `host/fxr`).
///
/// Uses `DOTNET_ROOT` when set, otherwise asks the `dotnet` on `PATH` where
/// its shared runtime lives.
pub fn discover_dotnet_root() -> Result<PathBuf, HostError> {
    if let Some(root) = std::env::var_os("DOTNET_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let output = Command::new("dotnet")
        .arg("--list-runtimes")
        .output()
        .map_err(|e| {
            HostError::NotFound(format!(
                "DOTNET_ROOT is not set and `dotnet` cannot be run: {e}"
            ))
        })?;
    // Lines look like:
    //   Microsoft.NETCore.App 10.0.12 [/root/shared/Microsoft.NETCore.App]
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if !line.starts_with("Microsoft.NETCore.App ") {
            continue;
        }
        if let (Some(open), Some(close)) = (line.find('['), line.rfind(']')) {
            let shared = Path::new(&line[open + 1..close]);
            // <root>/shared/Microsoft.NETCore.App
            if let Some(root) = shared.parent().and_then(Path::parent) {
                return Ok(root.to_path_buf());
            }
        }
    }
    Err(HostError::NotFound(
        "`dotnet --list-runtimes` lists no Microsoft.NETCore.App".into(),
    ))
}

/// The newest `libhostfxr.so` under `<root>/host/fxr/<version>/`.
fn find_hostfxr(root: &Path) -> Result<PathBuf, HostError> {
    let fxr = root.join("host").join("fxr");
    let entries = std::fs::read_dir(&fxr)
        .map_err(|e| HostError::NotFound(format!("{}: {e}", fxr.display())))?;
    let mut versions: Vec<(Vec<u32>, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| {
            let key = e
                .file_name()
                .to_string_lossy()
                .split(|c: char| !c.is_ascii_digit())
                .filter_map(|p| p.parse().ok())
                .collect();
            (key, e.path())
        })
        .collect();
    versions.sort();
    let (_, dir) = versions.pop().ok_or_else(|| {
        HostError::NotFound(format!("no hostfxr under {}", fxr.display()))
    })?;
    let lib = dir.join(libloading::library_filename("hostfxr"));
    if lib.exists() {
        Ok(lib)
    } else {
        Err(HostError::NotFound(format!(
            "{} does not exist",
            lib.display()
        )))
    }
}

/// A running .NET runtime.
pub struct DotnetHost {
    load_assembly: LoadAssemblyAndGetFunctionPointer,
    dotnet_root: PathBuf,
    // Never unloaded: the runtime keeps calling into hostfxr's memory.
    _hostfxr: Library,
}

// SAFETY: the function pointers are plain C entry points, callable from any
// thread, and the library handle is only held to keep it loaded.
unsafe impl Send for DotnetHost {}
unsafe impl Sync for DotnetHost {}

static HOST: OnceLock<Result<DotnetHost, String>> = OnceLock::new();

impl DotnetHost {
    /// Starts the runtime described by `runtime_config` (a
    /// `*.runtimeconfig.json`), or returns the one already running.
    ///
    /// The runtime exists once per process: the first call decides which
    /// runtime config and .NET installation are used, and later calls return
    /// the same host whatever their arguments.
    pub fn get_or_init(
        runtime_config: &Path,
    ) -> Result<&'static Self, HostError> {
        HOST.get_or_init(|| {
            Self::start(runtime_config).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|m| HostError::Load(m.clone()))
    }

    fn start(runtime_config: &Path) -> Result<Self, HostError> {
        let root = discover_dotnet_root()?;
        let hostfxr_path = find_hostfxr(&root)?;
        // SAFETY: loading the shared library runs its initializers, as the
        // hosting documentation intends.
        let lib = unsafe { Library::new(&hostfxr_path) }.map_err(|e| {
            HostError::Load(format!("{}: {e}", hostfxr_path.display()))
        })?;

        // SAFETY: the symbol names and signatures are hostfxr's documented C
        // API (`hostfxr.h`).
        let (initialize, get_delegate, close) = unsafe {
            let sym = |name: &[u8]| {
                lib.get::<*mut c_void>(name).map_err(|e| {
                    HostError::Load(format!(
                        "{}: {e}",
                        String::from_utf8_lossy(name)
                    ))
                })
            };
            (
                *sym(b"hostfxr_initialize_for_runtime_config\0")?
                    as *const c_void,
                *sym(b"hostfxr_get_runtime_delegate\0")? as *const c_void,
                *sym(b"hostfxr_close\0")? as *const c_void,
            )
        };
        // SAFETY: function pointers of exactly these signatures.
        let (initialize, get_delegate, close) = unsafe {
            (
                std::mem::transmute::<*const c_void, InitializeForRuntimeConfig>(
                    initialize,
                ),
                std::mem::transmute::<*const c_void, GetRuntimeDelegate>(
                    get_delegate,
                ),
                std::mem::transmute::<*const c_void, Close>(close),
            )
        };

        let config = path_cstring(runtime_config)?;
        let root_c = path_cstring(&root)?;
        let host_path = path_cstring(
            &std::env::current_exe().unwrap_or_else(|_| PathBuf::from("host")),
        )?;
        let params = InitializeParameters {
            size: size_of::<InitializeParameters>(),
            host_path: host_path.as_ptr(),
            dotnet_root: root_c.as_ptr(),
        };

        let mut context: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated strings and out-pointer.
        let code =
            unsafe { initialize(config.as_ptr(), &params, &mut context) };
        check("hostfxr_initialize_for_runtime_config", code)?;

        let mut delegate: *mut c_void = std::ptr::null_mut();
        // SAFETY: `context` was just produced by hostfxr.
        let code = unsafe {
            get_delegate(
                context,
                HDT_LOAD_ASSEMBLY_AND_GET_FUNCTION_POINTER,
                &mut delegate,
            )
        };
        let result = check("hostfxr_get_runtime_delegate", code);
        // The context only configures the runtime; the delegate stays valid.
        // SAFETY: `context` is closed exactly once.
        unsafe { close(context) };
        result?;
        assert!(!delegate.is_null(), "hostfxr returned no delegate");

        Ok(Self {
            // SAFETY: this delegate type has exactly this signature.
            load_assembly: unsafe {
                std::mem::transmute::<
                    *mut c_void,
                    LoadAssemblyAndGetFunctionPointer,
                >(delegate)
            },
            dotnet_root: root,
            _hostfxr: lib,
        })
    }

    pub fn dotnet_root(&self) -> &Path {
        &self.dotnet_root
    }

    /// Loads `assembly_path` (into the runtime's default load context) and
    /// returns the address of a `[UnmanagedCallersOnly]` static method.
    ///
    /// `type_name` is `"Namespace.Type, AssemblyName"`.
    ///
    /// # Safety
    /// `F` must be an `extern "C"` function pointer type matching the managed
    /// method's signature exactly.
    pub unsafe fn function<F: Copy>(
        &self,
        assembly_path: &Path,
        type_name: &str,
        method: &str,
    ) -> Result<F, HostError> {
        assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
        let assembly = path_cstring(assembly_path)?;
        let type_name = cstring(type_name)?;
        let method = cstring(method)?;
        let mut out: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated strings; the sentinel selects an
        // `UnmanagedCallersOnly` method.
        let code = unsafe {
            (self.load_assembly)(
                assembly.as_ptr(),
                type_name.as_ptr(),
                method.as_ptr(),
                UNMANAGEDCALLERSONLY_METHOD,
                std::ptr::null_mut(),
                &mut out,
            )
        };
        check("load_assembly_and_get_function_pointer", code)?;
        // SAFETY: sizes match (asserted) and the pointer is a function.
        Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&out) })
    }
}
