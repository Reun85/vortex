//! Script (`Behaviour`) components as editor data. delta cannot read a
//! managed object, so each managed component carries an
//! [`ExternalSerde`] whose functions forward to the script runtime, which
//! serializes the object's persisted members as JSON.

use std::{ptr, sync::OnceLock};

use delta::{ComponentId, Entity, ExternalSerde, OwnedValue, World};
use serde_json::{Value, json};

use crate::ffi::MODE_PERSIST;

/// The managed half of the contract, as plain function pointers. Set once
/// from the script runtime, which exists once per process.
#[derive(Clone, Copy)]
pub(crate) struct ManagedFns {
    pub to_json: unsafe extern "C" fn(*mut World, u64, u32) -> usize,
    pub from_json:
        unsafe extern "C" fn(*mut World, u32, *const u8, usize) -> u64,
    pub get_member:
        unsafe extern "C" fn(*mut World, u64, *const u8, usize) -> usize,
    pub set_member: unsafe extern "C" fn(
        *mut World,
        u64,
        *const u8,
        usize,
        *const u8,
        usize,
    ) -> usize,
    pub schema: unsafe extern "C" fn(u32) -> usize,
    pub default_json: unsafe extern "C" fn(*mut World, u32) -> usize,
    pub remap_entity: unsafe extern "C" fn(*mut World, u64, u64, u64) -> u32,
    pub take_result: unsafe extern "C" fn(*mut u8, usize) -> usize,
    pub last_error: unsafe extern "C" fn(*mut u8, usize) -> usize,
}

pub(crate) static MANAGED: OnceLock<ManagedFns> = OnceLock::new();

fn fns() -> Result<&'static ManagedFns, String> {
    MANAGED
        .get()
        .ok_or_else(|| "the script runtime is not started".to_owned())
}

fn text_of(
    buffer: impl Fn(*mut u8, usize) -> usize,
    len: usize,
) -> Result<String, String> {
    let mut bytes = vec![0u8; len];
    if len > 0 {
        let n = buffer(bytes.as_mut_ptr(), len);
        bytes.truncate(n.min(len));
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

fn error_text(m: &ManagedFns) -> String {
    // The error is at most a few KB; ask for the length first.
    // SAFETY: a zero-capacity call only reports the length.
    let len = unsafe { (m.last_error)(ptr::null_mut(), 0) };
    // SAFETY: the pointer and capacity describe the buffer `text_of` owns.
    text_of(|p, n| unsafe { (m.last_error)(p, n) }, len).unwrap_or_else(|e| e)
}

/// The text a managed call stored, given the length it returned.
fn result(m: &ManagedFns, len: usize) -> Result<String, String> {
    if len == usize::MAX {
        return Err(error_text(m));
    }
    // SAFETY: the pointer and capacity describe the buffer `text_of` owns.
    text_of(|p, n| unsafe { (m.take_result)(p, n) }, len)
}

fn parse(text: Result<String, String>) -> Result<Value, String> {
    text.and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
}

/// The handle stored in a managed component's column cell.
fn handle_at(p: *const u8) -> u64 {
    // SAFETY: callers pass a pointer to a managed component's cell.
    unsafe { p.cast::<u64>().read_unaligned() }
}

fn world_ptr(world: &World) -> *mut World {
    ptr::from_ref(world).cast_mut()
}

/// Runs `f` with the world's deferred buffer as the current command queue,
/// so `Spawn`/`Insert`/`Despawn` from script code (`OnAfterDeserialize`,
/// property setters) are applied at the sync point.
fn with_deferred<R>(world: &mut World, f: impl FnOnce(*mut World) -> R) -> R {
    let queue = world.deferred_queue_ptr();
    let previous = world
        .resource_mut::<crate::bridge::ScriptState>()
        .map(|s| std::mem::replace(&mut s.current_queue, queue));
    let raw: *mut World = world;
    let out = f(raw);
    if let (Some(previous), Some(s)) =
        (previous, world.resource_mut::<crate::bridge::ScriptState>())
    {
        s.current_queue = previous;
    }
    out
}

fn to_json(world: &World, p: *const u8) -> Value {
    let (Ok(m), h) = (fns(), handle_at(p)) else {
        return Value::Null;
    };
    if h == 0 {
        return Value::Null; // saved away by a reload
    }
    // SAFETY: valid world pointer and live handle.
    let len = unsafe { (m.to_json)(world_ptr(world), h, MODE_PERSIST) };
    parse(result(m, len)).unwrap_or_else(|e| json!({ "$error": e }))
}

fn from_json(
    world: &mut World,
    c: ComponentId,
    value: &Value,
) -> Result<OwnedValue, String> {
    let m = fns()?;
    let info = world.components().info(c);
    let mut owned = OwnedValue::zeroed(info.layout, info.drop);
    let text = value.to_string();
    let handle = with_deferred(world, |w| {
        // SAFETY: valid world, and the text outlives the call.
        unsafe { (m.from_json)(w, c.index() as u32, text.as_ptr(), text.len()) }
    });
    if handle == 0 {
        return Err(error_text(m));
    }
    // SAFETY: the cell is one u64 handle, now owned by `owned`.
    unsafe { owned.as_mut_ptr().cast::<u64>().write_unaligned(handle) };
    Ok(owned)
}

fn default_json(world: &World, c: ComponentId) -> Value {
    let Ok(m) = fns() else {
        return Value::Null;
    };
    // SAFETY: valid world pointer.
    let len = unsafe { (m.default_json)(world_ptr(world), c.index() as u32) };
    parse(result(m, len)).unwrap_or(Value::Null)
}

fn get_path(world: &World, p: *const u8, path: &str) -> Result<Value, String> {
    let m = fns()?;
    let h = handle_at(p);
    if h == 0 {
        return Err("the behaviour is not available right now".into());
    }
    // SAFETY: valid world pointer, live handle, path bytes outlive the call.
    let len = unsafe {
        (m.get_member)(world_ptr(world), h, path.as_ptr(), path.len())
    };
    parse(result(m, len))
}

fn set_path(
    world: &mut World,
    p: *mut u8,
    path: &str,
    value: &Value,
) -> Result<Value, String> {
    let m = fns()?;
    let h = handle_at(p);
    if h == 0 {
        return Err("the behaviour is not available right now".into());
    }
    let text = value.to_string();
    let len = with_deferred(world, |w| {
        // SAFETY: valid world, live handle, buffers outlive the call.
        unsafe {
            (m.set_member)(
                w,
                h,
                path.as_ptr(),
                path.len(),
                text.as_ptr(),
                text.len(),
            )
        }
    });
    parse(result(m, len))
}

fn schema(_: &World, c: ComponentId) -> Value {
    let Ok(m) = fns() else {
        return json!([]);
    };
    // SAFETY: a plain query function of the managed side.
    let len = unsafe { (m.schema)(c.index() as u32) };
    parse(result(m, len)).unwrap_or_else(|_| json!([]))
}

fn remap_entity(world: &World, p: *mut u8, from: Entity, to: Entity) -> usize {
    let (Ok(m), h) = (fns(), handle_at(p)) else {
        return 0;
    };
    if h == 0 {
        return 0;
    }
    // SAFETY: valid world pointer and live handle.
    unsafe {
        (m.remap_entity)(world_ptr(world), h, from.to_bits(), to.to_bits())
            as usize
    }
}

/// What every managed component's [`ComponentInfo::external`] is set to.
pub(crate) const SERDE: ExternalSerde = ExternalSerde {
    to_json,
    from_json,
    default_json,
    get_path,
    set_path,
    schema,
    remap_entity,
};
