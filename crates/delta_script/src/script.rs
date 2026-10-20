//! Starting the scripting runtime for a world, and the systems it provides.

use std::{fmt, mem::MaybeUninit, path::Path};

use delta::{Access, Schedule, System, Tick, World};

use crate::{
    bridge::{self, FREE_HANDLE, PendingSystem, ScriptState, state},
    ffi::{API_VERSION, HostApi, ScriptApi},
    host::{DotnetHost, HostError},
};

#[derive(Debug)]
pub enum ScriptError {
    Host(HostError),
    /// The managed side refused to initialize or load.
    Script(String),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(e) => write!(f, "{e}"),
            Self::Script(m) => write!(f, "script error: {m}"),
        }
    }
}

impl std::error::Error for ScriptError {}

impl From<HostError> for ScriptError {
    fn from(e: HostError) -> Self {
        Self::Host(e)
    }
}

type InitializeFn = unsafe extern "C" fn(*const HostApi, *mut ScriptApi) -> i32;

/// The scripting runtime attached to one world.
///
/// The C# side keeps per-world registries in statics, so for now one world
/// per process may use scripting.
pub struct ScriptHost {
    load_assembly: unsafe extern "C" fn(*mut World, *const u8, usize) -> i32,
    run_system: unsafe extern "C" fn(u32, *mut World),
    live_handles: unsafe extern "C" fn() -> u64,
    managed_error: unsafe extern "C" fn(*mut u8, usize) -> usize,
}

impl ScriptHost {
    /// Starts (or joins) the .NET runtime, loads `bridge_assembly`
    /// (`Delta.Scripting.dll`), and exchanges function tables with it.
    pub fn start(
        world: &mut World,
        runtime_config: &Path,
        bridge_assembly: &Path,
    ) -> Result<Self, ScriptError> {
        let host = DotnetHost::get_or_init(runtime_config)?;
        // SAFETY: `Bridge.Initialize` is declared with exactly this
        // signature on the managed side.
        let initialize: InitializeFn = unsafe {
            host.function(
                bridge_assembly,
                "Delta.Bridge, Delta.Scripting",
                "Initialize",
            )
        }?;

        world.insert_resource(ScriptState::new());
        let host_api = bridge::host_api();
        let mut script_api = MaybeUninit::<ScriptApi>::zeroed();
        // SAFETY: `ScriptApi` is plain data whose function slots are
        // `Option`s, for which all-zero is a valid (empty) value.
        let status = unsafe { initialize(&host_api, script_api.as_mut_ptr()) };
        if status != 0 {
            return Err(ScriptError::Script(format!(
                "Bridge.Initialize failed with {status}"
            )));
        }
        // SAFETY: zeroed above, then filled in by the managed side.
        let api = unsafe { script_api.assume_init() };
        if api.version != API_VERSION {
            return Err(ScriptError::Script(format!(
                "script API version {} does not match host version {API_VERSION}",
                api.version
            )));
        }
        let missing = |name: &str| {
            ScriptError::Script(format!("script API did not provide `{name}`"))
        };
        let free_handle =
            api.free_handle.ok_or_else(|| missing("free_handle"))?;
        let _ = FREE_HANDLE.set(free_handle);
        Ok(Self {
            load_assembly: api
                .load_assembly
                .ok_or_else(|| missing("load_assembly"))?,
            run_system: api.run_system.ok_or_else(|| missing("run_system"))?,
            live_handles: api
                .live_handles
                .ok_or_else(|| missing("live_handles"))?,
            managed_error: api
                .last_error
                .ok_or_else(|| missing("last_error"))?,
        })
    }

    /// Loads a script assembly: its components are registered in `world`
    /// and the systems it declares are returned, ready for
    /// [`install`](Self::install).
    pub fn load_assembly(
        &self,
        world: &mut World,
        assembly: &Path,
    ) -> Result<Vec<PendingSystem>, ScriptError> {
        let path = assembly.to_string_lossy();
        // SAFETY: valid world pointer and UTF-8 path bytes.
        let status =
            unsafe { (self.load_assembly)(world, path.as_ptr(), path.len()) };
        if status < 0 {
            return Err(ScriptError::Script(self.managed_error_text()));
        }
        Ok(std::mem::take(&mut state(world).pending_systems))
    }

    /// Adds the loaded systems to `schedule`: by stage, then by their
    /// declared order, then by name, so the run order is deterministic.
    pub fn install(
        &self,
        schedule: &mut Schedule,
        mut systems: Vec<PendingSystem>,
    ) {
        systems.sort_by(|a, b| {
            (a.stage, a.order, &a.name).cmp(&(b.stage, b.order, &b.name))
        });
        for s in systems {
            schedule.add_boxed_system(
                s.stage,
                Box::new(ScriptSystem {
                    id: s.id,
                    name: s.name,
                    run: self.run_system,
                    queue: delta::CommandQueue::default(),
                    last_run: Tick::new(0),
                    access: Access::default(),
                }),
            );
        }
    }

    fn managed_error_text(&self) -> String {
        let mut buffer = vec![0u8; 8192];
        // SAFETY: the pointer and capacity describe `buffer`.
        let len =
            unsafe { (self.managed_error)(buffer.as_mut_ptr(), buffer.len()) };
        buffer.truncate(len.min(buffer.len()));
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// How many managed component values currently hold a `GCHandle`.
    pub fn live_handles(&self) -> u64 {
        // SAFETY: a plain query function of the managed side.
        unsafe { (self.live_handles)() }
    }
}

/// A system implemented in C#: the schedule runs it like any other, and it
/// calls back into the world through the host API.
pub struct ScriptSystem {
    id: u32,
    name: String,
    run: unsafe extern "C" fn(u32, *mut World),
    queue: delta::CommandQueue,
    last_run: Tick,
    access: Access,
}

impl System for ScriptSystem {
    fn name(&self) -> &str {
        &self.name
    }

    fn access(&self) -> &Access {
        &self.access
    }

    fn initialize(&mut self, _: &mut World) {}

    fn run(&mut self, world: &mut World) {
        let this_run = world.change_tick();
        {
            let st = state(world);
            st.current_queue = &raw mut self.queue;
            st.current_last_run = self.last_run;
        }
        // SAFETY: the script runtime resolved `run`; the world pointer stays
        // valid and exclusively ours for the call.
        unsafe { (self.run)(self.id, world) };
        state(world).current_queue = std::ptr::null_mut();
        self.last_run = this_run;
        world.increment_change_tick();
    }

    fn apply_deferred(&mut self, world: &mut World) {
        self.queue.apply(world);
    }

    fn check_change_tick(&mut self, now: Tick) {
        self.last_run.check_tick(now);
    }
}
