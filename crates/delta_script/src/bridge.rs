//! The Rust implementations of the [`HostApi`] table, and the per-world
//! state they share.

use std::{alloc::Layout, slice, sync::OnceLock};

use delta::{
    CommandQueue,
    ComponentId,
    ComponentInfo,
    Entity,
    Kind,
    QueryDesc,
    QueryState,
    Stage,
    Storage,
    Tick,
    World,
    WorldCell,
    query::Cursor,
    reflect::{FieldAttrs, FieldReflect, FieldType, ReflectId},
};

use crate::ffi::*;

/// Frees the `GCHandle` behind a managed component's column value. Set once
/// from the script runtime, which exists once per process.
pub(crate) static FREE_HANDLE: OnceLock<unsafe extern "C" fn(u64)> =
    OnceLock::new();

/// The drop function of every managed (`Behaviour`) component column.
///
/// # Safety
/// `p` points at a stored `u64` handle value.
pub(crate) unsafe fn drop_gc_handle(p: *mut u8) {
    // SAFETY: per contract.
    let handle = unsafe { p.cast::<u64>().read_unaligned() };
    if handle == 0 {
        return; // already freed, e.g. by a reload that saved this value
    }
    if let Some(free) = FREE_HANDLE.get() {
        // SAFETY: the callback was handed out by the script runtime.
        unsafe { free(handle) };
    }
}

/// A system a script assembly asked to be scheduled.
#[derive(Clone, Debug)]
pub struct PendingSystem {
    pub id: u32,
    pub name: String,
    pub stage: Stage,
    pub order: i32,
}

/// Lives in the world as a resource so that host calls, which receive only a
/// world pointer, can find everything they need.
pub(crate) struct ScriptState {
    /// `None` only while a query is checked out by `query_next`.
    pub queries: Vec<Option<QueryState>>,
    /// The running script system's command buffer, set around each run.
    pub current_queue: *mut CommandQueue,
    pub current_last_run: Tick,
    pub pending_systems: Vec<PendingSystem>,
    pub next_system_id: u32,
    pub last_error: String,
    /// While a hot reload loads the new assembly, registering an existing
    /// component with a different definition redefines it instead of failing.
    pub reloading: bool,
    /// Every script component the current load registered or confirmed.
    pub load_components: Vec<ComponentId>,
    /// Behaviour state saved from entities whose class is gone (or whose
    /// reload failed). Restored if the class comes back.
    pub removed_records: Vec<RemovedRecord>,
}

/// Saved state of a behaviour that currently has no class to live in.
#[derive(Clone, Debug)]
pub struct RemovedRecord {
    /// The behaviour class, as the stable component name.
    pub class: String,
    pub entity: Entity,
    pub bytes: Vec<u8>,
}

// SAFETY: scripts run on the thread that owns the world; the raw pointer is
// only dereferenced during a script system's run on that thread.
unsafe impl Send for ScriptState {}
unsafe impl Sync for ScriptState {}

impl ScriptState {
    pub(crate) fn new() -> Self {
        Self {
            queries: Vec::new(),
            current_queue: std::ptr::null_mut(),
            current_last_run: Tick::new(0),
            pending_systems: Vec::new(),
            next_system_id: 0,
            last_error: String::new(),
            reloading: false,
            load_components: Vec::new(),
            removed_records: Vec::new(),
        }
    }
}

pub(crate) fn state(world: &mut World) -> &mut ScriptState {
    world
        .resource_mut::<ScriptState>()
        .expect("the script host was not started for this world")
}

const ERR_ID: u32 = u32::MAX;

fn fail(world: &mut World, message: impl Into<String>) -> u32 {
    state(world).last_error = message.into();
    ERR_ID
}

/// # Safety
/// `ptr..ptr+len` is valid UTF-8 memory.
unsafe fn str_from<'a>(ptr: *const u8, len: usize) -> Result<&'a str, String> {
    // SAFETY: per contract.
    std::str::from_utf8(unsafe { slice::from_raw_parts(ptr, len) })
        .map_err(|e| format!("invalid utf-8 from script: {e}"))
}

fn field_type(code: u32, aux: u32) -> Result<FieldType, String> {
    Ok(match code {
        FT_BOOL => FieldType::Bool,
        FT_I8 => FieldType::I8,
        FT_I16 => FieldType::I16,
        FT_I32 => FieldType::I32,
        FT_I64 => FieldType::I64,
        FT_U8 => FieldType::U8,
        FT_U16 => FieldType::U16,
        FT_U32 => FieldType::U32,
        FT_U64 => FieldType::U64,
        FT_F32 => FieldType::F32,
        FT_F64 => FieldType::F64,
        FT_ENTITY => FieldType::Entity,
        FT_STRUCT => FieldType::Struct(ReflectId::from_index(aux)),
        other => return Err(format!("unknown field type code {other}")),
    })
}

/// # Safety
/// `desc` and everything it points to is valid.
unsafe fn register_component_impl(
    world: &mut World,
    desc: &ComponentDescFfi,
) -> Result<u32, String> {
    // SAFETY: per contract.
    let name = unsafe { str_from(desc.name, desc.name_len)? };
    let fields = if desc.field_count == 0 {
        &[]
    } else {
        // SAFETY: per contract.
        unsafe { slice::from_raw_parts(desc.fields, desc.field_count as usize) }
    };
    let mut reflect_fields = Vec::with_capacity(fields.len());
    for f in fields {
        // SAFETY: per contract.
        let fname = unsafe { str_from(f.name, f.name_len)? };
        reflect_fields.push(FieldReflect::new(
            fname,
            f.offset as usize,
            field_type(f.ty, f.aux)?,
            FieldAttrs::default(),
        ));
    }

    let reloading = state(world).reloading;

    if desc.kind == KIND_STRUCT {
        let registry = world.reflection_mut();
        return if reloading {
            Ok(registry
                .replace_dynamic(name, desc.size, desc.align, reflect_fields)
                .index() as u32)
        } else {
            registry
                .register_dynamic(name, desc.size, desc.align, reflect_fields)
                .map(|id| id.index() as u32)
        };
    }

    let layout = Layout::from_size_align(desc.size, desc.align)
        .map_err(|e| format!("bad layout for `{name}`: {e}"))?;
    let (kind, managed) = match desc.kind {
        KIND_UNMANAGED => (Kind::ScriptUnmanaged, false),
        KIND_MANAGED => {
            if layout.size() != size_of::<u64>() {
                return Err(format!(
                    "managed component `{name}` must be one 8-byte handle"
                ));
            }
            (Kind::ScriptManaged, true)
        }
        KIND_TAG => (Kind::Tag, false),
        other => return Err(format!("unknown component kind {other}")),
    };

    let reflect = if managed {
        None
    } else if reloading {
        Some(world.reflection_mut().replace_dynamic(
            name,
            desc.size,
            desc.align,
            reflect_fields,
        ))
    } else {
        // An existing component with a different shape is rejected below;
        // registering the shape first keeps that error specific.
        Some(world.reflection_mut().register_dynamic(
            name,
            desc.size,
            desc.align,
            reflect_fields,
        )?)
    };

    let id = if let Some(existing) = world.components().id_by_name(name) {
        let info = world.components().info(existing);
        if info.layout != layout || info.kind != kind {
            if !reloading {
                return Err(format!(
                    "component `{name}` is already registered with a different \
                     layout (only a reload may redefine it)"
                ));
            }
            // SAFETY: the reload migrates every column of this component
            // before the world runs again (see `ScriptHost::reload`).
            let info = unsafe { world.components_mut().info_mut(existing) };
            info.layout = layout;
            info.kind = kind;
            info.drop = None;
        }
        if reloading {
            // SAFETY: only the reflection id changes here; the layout case
            // above is the one that needs migration.
            unsafe { world.components_mut().info_mut(existing) }.reflect =
                reflect;
        }
        existing
    } else {
        world.components_mut().register_raw(ComponentInfo {
            name: name.into(),
            layout,
            drop: managed.then_some(drop_gc_handle as delta::blob_vec::DropFn),
            clone: None,
            kind,
            storage: Storage::Table,
            hooks: delta::ComponentHooks::default(),
            reflect,
        })
    };
    let st = state(world);
    if !st.load_components.contains(&id) {
        st.load_components.push(id);
    }
    Ok(id.index() as u32)
}

pub(crate) unsafe extern "C" fn register_component(
    world: *mut World,
    desc: *const ComponentDescFfi,
) -> u32 {
    // SAFETY: the host passes the world it is running in and a valid desc.
    let (world, desc) = unsafe { (&mut *world, &*desc) };
    // SAFETY: as above.
    match unsafe { register_component_impl(world, desc) } {
        Ok(id) => id,
        Err(message) => fail(world, message),
    }
}

pub(crate) unsafe extern "C" fn component_id_by_name(
    world: *mut World,
    name: *const u8,
    len: usize,
) -> u32 {
    // SAFETY: valid world and UTF-8 name per the contract.
    let world = unsafe { &mut *world };
    // SAFETY: as above.
    match unsafe { str_from(name, len) } {
        Ok(name) => world
            .components()
            .id_by_name(name)
            .map_or(ERR_ID, |id| id.index() as u32),
        Err(m) => fail(world, m),
    }
}

pub(crate) unsafe extern "C" fn register_system(
    world: *mut World,
    desc: *const SystemDescFfi,
) -> u32 {
    // SAFETY: valid world and desc per the contract.
    let (world, desc) = unsafe { (&mut *world, &*desc) };
    // SAFETY: as above.
    let name = match unsafe { str_from(desc.name, desc.name_len) } {
        Ok(n) => n.to_owned(),
        Err(m) => return fail(world, m),
    };
    let stage = match Stage::ALL.get(desc.stage as usize) {
        Some(&s) => s,
        None => return fail(world, format!("unknown stage {}", desc.stage)),
    };
    let st = state(world);
    let id = st.next_system_id;
    st.next_system_id += 1;
    st.pending_systems.push(PendingSystem {
        id,
        name,
        stage,
        order: desc.order,
    });
    id
}

/// # Safety
/// `list` points at `len` valid ids.
unsafe fn ids(list: &IdList) -> Vec<ComponentId> {
    if list.len == 0 {
        return Vec::new();
    }
    // SAFETY: per contract.
    unsafe { slice::from_raw_parts(list.ptr, list.len) }
        .iter()
        .map(|&i| ComponentId::from_index(i))
        .collect()
}

pub(crate) unsafe extern "C" fn query_create(
    world: *mut World,
    desc: *const QueryDescFfi,
) -> u32 {
    // SAFETY: valid world and desc per the contract.
    let (world, d) = unsafe { (&mut *world, &*desc) };
    // SAFETY: as above.
    let desc = unsafe {
        QueryDesc {
            read: ids(&d.read),
            write: ids(&d.write),
            with: ids(&d.with),
            without: ids(&d.without),
            optional: ids(&d.optional),
            added: ids(&d.added),
            changed: ids(&d.changed),
        }
    };
    if let Err(e) = desc.validate() {
        return fail(world, e.to_string());
    }
    if desc.term_count() > MAX_TERMS {
        return fail(
            world,
            format!("a query may have at most {MAX_TERMS} terms"),
        );
    }
    let st = state(world);
    st.queries.push(Some(QueryState::new(desc)));
    (st.queries.len() - 1) as u32
}

pub(crate) unsafe extern "C" fn query_next(
    world: *mut World,
    query: u32,
    cursor: *mut Cursor,
    out: *mut ChunkFfi,
) -> u8 {
    // SAFETY: the host passes the world it is running in.
    let world = unsafe { &mut *world };
    let st = state(world);
    let last_run = st.current_last_run;
    // Check the query out so it can borrow the world alongside the state.
    let Some(mut q) = st.queries.get_mut(query as usize).and_then(Option::take)
    else {
        return 0;
    };
    // SAFETY: the cursor lives in the host's memory for the whole iteration.
    let cursor = unsafe { &mut *cursor };
    if cursor.is_start() {
        q.update(world);
    }
    let stamp = !q.desc().write.is_empty();
    let terms = q.desc().term_count();
    let mut found = 0;
    {
        let cell = WorldCell::new(world);
        // SAFETY: the query is up to date; a script system has exclusive
        // access for the duration of its run, and no chunk is retained
        // across calls (the host copies pointers out immediately).
        if let Some(chunk) =
            unsafe { q.next_chunk(cell, cursor, last_run, stamp) }
        {
            // SAFETY: `out` is a valid ChunkFfi supplied by the host.
            let out = unsafe { &mut *out };
            out.archetype = chunk.archetype().index() as u32;
            out.len = chunk.len() as u32;
            out.entities = chunk.entities().as_ptr().cast::<u64>();
            for (term, slot) in out.columns.iter_mut().enumerate() {
                *slot = if term < terms {
                    chunk.ptr(term).unwrap_or(std::ptr::null_mut())
                } else {
                    std::ptr::null_mut()
                };
            }
            found = 1;
        }
    }
    state(world).queries[query as usize] = Some(q);
    found
}

pub(crate) unsafe extern "C" fn get_ptr(
    world: *mut World,
    entity: u64,
    component: u32,
    write: u8,
) -> *mut u8 {
    // SAFETY: the host passes the world it is running in.
    let world = unsafe { &*world };
    let e = Entity::from_bits(entity);
    let c = ComponentId::from_index(component);
    let Some(loc) = world.location(e) else {
        return std::ptr::null_mut();
    };
    let Some(column) = world.archetypes().get(loc.archetype).column(c) else {
        return std::ptr::null_mut();
    };
    if write != 0 {
        // SAFETY: a script system has exclusive access while it runs.
        unsafe { column.changed_cell(loc.row).write(world.change_tick()) };
    }
    column.get_ptr(loc.row)
}

pub(crate) unsafe extern "C" fn is_alive(world: *mut World, entity: u64) -> u8 {
    // SAFETY: the host passes the world it is running in.
    u8::from(unsafe { &*world }.is_alive(Entity::from_bits(entity)))
}

/// The command buffer of the running script system.
///
/// # Safety
/// Only valid while a script system runs.
unsafe fn queue<'a>(world: &mut World) -> Option<&'a mut CommandQueue> {
    let ptr = state(world).current_queue;
    // SAFETY: set by `ScriptSystem::run` to its own queue for the duration of
    // the call, on this thread.
    unsafe { ptr.as_mut() }
}

pub(crate) unsafe extern "C" fn cmd_spawn(world: *mut World) -> u64 {
    // SAFETY: the host passes the world it is running in.
    unsafe { &*world }.entities().reserve().to_bits()
}

pub(crate) unsafe extern "C" fn cmd_despawn(world: *mut World, entity: u64) {
    // SAFETY: as above.
    let world = unsafe { &mut *world };
    // SAFETY: called from a running script system.
    match unsafe { queue(world) } {
        Some(q) => q.despawn(Entity::from_bits(entity)),
        None => state(world).last_error = "cmd_despawn outside a system".into(),
    }
}

pub(crate) unsafe extern "C" fn cmd_insert(
    world: *mut World,
    entity: u64,
    component: u32,
    bytes: *const u8,
    len: usize,
) {
    // SAFETY: the host passes the world it is running in.
    let world = unsafe { &mut *world };
    let c = ComponentId::from_index(component);
    let (layout, drop) = {
        let info = world.components().info(c);
        (info.layout, info.drop)
    };
    if len != layout.size() {
        state(world).last_error =
            format!("cmd_insert: expected {} bytes, got {len}", layout.size());
        return;
    }
    // SAFETY: called from a running script system.
    match unsafe { queue(world) } {
        // SAFETY: `bytes` holds a value of the component's layout; for a
        // managed component its ownership (the handle) moves into the queue.
        Some(q) => unsafe {
            q.insert_by_id(Entity::from_bits(entity), c, layout, drop, bytes);
        },
        None => state(world).last_error = "cmd_insert outside a system".into(),
    }
}

pub(crate) unsafe extern "C" fn cmd_remove(
    world: *mut World,
    entity: u64,
    component: u32,
) {
    // SAFETY: the host passes the world it is running in.
    let world = unsafe { &mut *world };
    // SAFETY: called from a running script system.
    match unsafe { queue(world) } {
        Some(q) => q.remove_by_id(
            Entity::from_bits(entity),
            ComponentId::from_index(component),
        ),
        None => state(world).last_error = "cmd_remove outside a system".into(),
    }
}

pub(crate) unsafe extern "C" fn log(level: u32, text: *const u8, len: usize) {
    // SAFETY: valid UTF-8 per the contract.
    let text = unsafe { str_from(text, len) }.unwrap_or("<invalid utf-8>");
    let tag = ["trace", "info", "warn", "error"]
        .get(level as usize)
        .unwrap_or(&"log");
    eprintln!("[script {tag}] {text}");
}

pub(crate) unsafe extern "C" fn last_error(
    world: *mut World,
    buffer: *mut u8,
    capacity: usize,
) -> usize {
    // SAFETY: the host passes the world it is running in.
    let message = &state(unsafe { &mut *world }).last_error;
    let n = message.len().min(capacity);
    // SAFETY: `buffer` has room for `capacity` bytes.
    unsafe { std::ptr::copy_nonoverlapping(message.as_ptr(), buffer, n) };
    message.len()
}

pub(crate) fn host_api() -> HostApi {
    HostApi {
        version: API_VERSION,
        size: size_of::<HostApi>() as u32,
        register_component,
        component_id_by_name,
        register_system,
        query_create,
        query_next,
        get_ptr,
        is_alive,
        cmd_spawn,
        cmd_despawn,
        cmd_insert,
        cmd_remove,
        log,
        last_error,
    }
}
