//! The Rust implementations of the [`HostApi`] table, and the per-world
//! state they share.

use std::{
    alloc::Layout,
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    slice,
    sync::OnceLock,
};

use delta::{
    CommandQueue,
    ComponentId,
    ComponentInfo,
    DeferredWorld,
    Entity,
    HookContext,
    Kind,
    QueryDesc,
    QueryState,
    Stage,
    Storage,
    Tick,
    World,
    WorldCell,
    query::Cursor,
    reflect::{
        FieldAttrs,
        FieldReflect,
        FieldType,
        ReflectId,
        TypeRegistry,
        types_equal,
    },
};

use crate::ffi::*;

/// Frees the `GCHandle` behind a managed component's column value. Set once
/// from the script runtime, which exists once per process.
pub(crate) static FREE_HANDLE: OnceLock<unsafe extern "C" fn(u64)> =
    OnceLock::new();

/// Delivers lifecycle events to behaviour objects. Set once from the script
/// runtime, like [`FREE_HANDLE`].
pub(crate) static BEHAVIOUR_EVENT: OnceLock<
    unsafe extern "C" fn(*mut World, u64, u64, u32),
> = OnceLock::new();

/// The `on_add` hook of every behaviour class: `Awake`.
fn behaviour_added(world: DeferredWorld<'_>, cx: HookContext) {
    behaviour_lifecycle(world, cx, EVENT_AWAKE);
}

/// The `on_remove` hook of every behaviour class: `OnDestroy`. The hook
/// contract guarantees the value is still in the column.
fn behaviour_removed(world: DeferredWorld<'_>, cx: HookContext) {
    behaviour_lifecycle(world, cx, EVENT_ON_DESTROY);
}

fn behaviour_lifecycle(
    mut world: DeferredWorld<'_>,
    cx: HookContext,
    event: u32,
) {
    let Some(deliver) = BEHAVIOUR_EVENT.get() else {
        return;
    };
    // SAFETY: the pointers are used only for this call; the callee records
    // structural changes in the queue (`cmd_*` go through
    // `ScriptState::current_queue`, set below) and never applies them.
    let (raw, queue) = unsafe { world.raw_parts() };
    // SAFETY: `raw` is the world the hook runs in; `world` is not used again.
    let world = unsafe { &mut *raw };
    if state(world).quiet_lifecycle {
        return;
    }
    let Some(cell) = world.get_by_id(cx.entity, cx.component) else {
        return;
    };
    // SAFETY: a managed column holds one u64 handle per row.
    let handle = unsafe { cell.cast::<u64>().read_unaligned() };
    if handle == 0 {
        return; // already freed, e.g. saved by a reload
    }
    // Commands from inside the callback (`Spawn`, `Insert`, `Despawn`) go
    // to the world's deferred buffer, which the sync point flushes.
    let previous = std::mem::replace(&mut state(world).current_queue, queue);
    // SAFETY: the callback was handed out by the script runtime.
    unsafe { deliver(raw, handle, cx.entity.to_bits(), event) };
    state(world).current_queue = previous;
}

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
    /// Query slots. A slot's state is `None` while the query is checked out
    /// by `query_next`, or after it was freed.
    pub queries: Vec<QuerySlot>,
    /// Indices of freed slots in `queries`, reused by `query_create`.
    pub free_queries: Vec<u32>,
    /// The assembly whose code is running (or loading); queries created
    /// meanwhile belong to it. [`NO_OWNER`] outside of any assembly.
    pub current_owner: u32,
    /// Exposed resources the running system asked for: (id, written).
    /// Becomes the system's `Access` after its first run.
    pub touched: Vec<(u32, bool)>,
    /// While set, behaviour hooks deliver no events: the reload is putting
    /// saved instances back and reports `OnAfterReload` itself.
    pub quiet_lifecycle: bool,
    /// The world the host first ran a script in. It must never move.
    pub pinned_world: *const World,
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

/// Owner of queries created outside of any assembly's code.
pub(crate) const NO_OWNER: u32 = u32::MAX;

/// More live queries than this from one assembly is almost certainly a
/// `Build()` inside `OnRun`; it is reported once.
pub(crate) const QUERY_WARN_LIMIT: usize = 256;

pub(crate) struct QuerySlot {
    pub state: Option<QueryState>,
    pub owner: u32,
    pub live: bool,
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
            free_queries: Vec::new(),
            current_owner: NO_OWNER,
            touched: Vec::new(),
            quiet_lifecycle: false,
            pinned_world: std::ptr::null(),
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

impl ScriptState {
    /// Frees every query an assembly created, so that reloading does not
    /// accumulate slots.
    pub(crate) fn free_queries_of(&mut self, owner: u32) {
        for (i, slot) in self.queries.iter_mut().enumerate() {
            if slot.live && slot.owner == owner {
                slot.live = false;
                slot.state = None;
                self.free_queries.push(i as u32);
            }
        }
    }

    /// (live queries, slots ever allocated)
    pub(crate) fn query_counts(&self) -> (usize, usize) {
        (
            self.queries.iter().filter(|s| s.live).count(),
            self.queries.len(),
        )
    }
}

/// Runs a host function body so that a panic can never unwind into native
/// code (which would abort the process): the panic becomes `last_error` and
/// the function's error value.
fn guard<R>(
    world: *mut World,
    on_error: R,
    f: impl FnOnce(&mut World) -> R,
) -> R {
    if world.is_null() {
        return on_error;
    }
    // SAFETY: the host passes the world it is running in.
    let result = catch_unwind(AssertUnwindSafe(|| f(unsafe { &mut *world })));
    match result {
        Ok(r) => r,
        Err(payload) => {
            // SAFETY: as above; the closure's borrow ended with the unwind.
            let world = unsafe { &mut *world };
            if let Some(st) = world.resource_mut::<ScriptState>() {
                st.last_error = format!(
                    "host function panicked: {}",
                    panic_message(payload.as_ref())
                );
            }
            on_error
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Sets `last_error` without assuming the state exists.
fn set_error(world: &mut World, message: impl Into<String>) {
    if let Some(st) = world.resource_mut::<ScriptState>() {
        st.last_error = message.into();
    }
}

fn check_component(world: &World, c: u32, what: &str) -> Result<(), String> {
    if (c as usize) < world.components().len() {
        Ok(())
    } else {
        Err(format!("{what}: unknown component id {c}"))
    }
}

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

fn field_type(
    registry: &TypeRegistry,
    f: &FieldDescFfi,
) -> Result<FieldType, String> {
    scalar_or_struct(registry, f.ty, f.aux, Some((f.elem_ty, f.elem_aux)))
}

fn scalar_or_struct(
    registry: &TypeRegistry,
    code: u32,
    aux: u32,
    elem: Option<(u32, u32)>,
) -> Result<FieldType, String> {
    Ok(match code {
        FT_ARRAY => {
            let Some((elem_ty, elem_aux)) = elem else {
                return Err("arrays of arrays are not supported".into());
            };
            let elem = scalar_or_struct(registry, elem_ty, elem_aux, None)?;
            let stride = elem.size(registry);
            FieldType::Array {
                elem: Box::new(elem),
                len: aux as usize,
                stride,
            }
        }
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
        FT_STRUCT => {
            if aux as usize >= registry.len() {
                return Err(format!("unknown struct reflect id {aux}"));
            }
            FieldType::Struct(ReflectId::from_index(aux))
        }
        FT_VEC2 | FT_VEC3 | FT_VEC4 | FT_QUAT => {
            let name = match code {
                FT_VEC2 => "vortex::Vec2",
                FT_VEC3 => "vortex::Vec3",
                FT_VEC4 => "vortex::Vec4",
                _ => "vortex::Quat",
            };
            match registry.id_by_name(name) {
                Some(id) => FieldType::Struct(id),
                None => {
                    return Err(format!(
                        "`{name}` is not registered; the engine must \
                         register its script types before scripts load"
                    ));
                }
            }
        }
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
        let tooltip = if f.tooltip.is_null() || f.tooltip_len == 0 {
            None
        } else {
            // SAFETY: per contract.
            Some(Box::from(unsafe { str_from(f.tooltip, f.tooltip_len)? }))
        };
        let attrs = FieldAttrs {
            range: (f.flags & FF_HAS_RANGE != 0)
                .then_some((f.range_lo, f.range_hi)),
            tooltip,
            hidden: f.flags & FF_HIDDEN != 0,
            read_only: f.flags & FF_READ_ONLY != 0,
        };
        reflect_fields.push(FieldReflect::new(
            fname,
            f.offset as usize,
            field_type(world.reflection(), f)?,
            attrs,
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

    let requires = {
        // SAFETY: per contract.
        let ids = unsafe { ids(world, &desc.requires, "requires")? };
        for &r in &ids {
            let info = world.components().info(r);
            if info.kind == Kind::ScriptManaged || info.drop.is_some() {
                return Err(format!(
                    "`{name}` requires `{}`, which cannot be constructed \
                     zeroed: only data and tag components may be required",
                    info.name
                ));
            }
        }
        ids
    };

    let mut existing = world.components().id_by_name(name);
    if let Some(old) = existing {
        let old_kind = world.components().info(old).kind;
        if old_kind != kind {
            if !reloading {
                return Err(format!(
                    "component `{name}` is already registered as {old_kind:?}, \
                     not {kind:?} (only a reload may change what it is)"
                ));
            }
            // A component that changed kind is a different component that
            // happens to share a name. Retire the old one (the reload removes
            // it from its entities) and register a fresh id; patching the
            // kind in place would reinterpret live columns.
            world.components_mut().retire(old);
            existing = None;
        }
    }

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

    let id = if let Some(existing) = existing {
        let info = world.components().info(existing);
        if info.layout != layout {
            if !reloading {
                return Err(format!(
                    "component `{name}` is already registered with a different \
                     layout (only a reload may redefine it)"
                ));
            }
            // SAFETY: the reload migrates every column of this component
            // before the world runs again (see `ScriptHost::reload`). The
            // kind is unchanged here, so the drop function stays valid.
            let info = unsafe { world.components_mut().info_mut(existing) };
            info.layout = layout;
        }
        if reloading {
            // SAFETY: only the reflection id changes here; the layout case
            // above is the one that needs migration.
            unsafe { world.components_mut().info_mut(existing) }.reflect =
                reflect;
        }
        existing
    } else {
        let mut hooks = delta::ComponentHooks::default();
        if managed {
            hooks.on_add = Some(behaviour_added);
            hooks.on_remove = Some(behaviour_removed);
        }
        world.components_mut().register_raw(ComponentInfo {
            name: name.into(),
            layout,
            drop: managed.then_some(drop_gc_handle as delta::blob_vec::DropFn),
            clone: None,
            kind,
            storage: Storage::Table,
            hooks,
            reflect,
            requires: Box::default(),
            external: managed.then_some(crate::managed::SERDE),
        })
    };
    // Declared on every registration, so a reload picks up changes.
    world.components_mut().set_requires(id, &requires);
    if managed {
        // SAFETY: only the serialization table changes; the layout and
        // storage of the component are untouched.
        unsafe { world.components_mut().info_mut(id) }.external =
            Some(crate::managed::SERDE);
    }
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
    guard(world, ERR_ID, |world| {
        if desc.is_null() {
            return fail(world, "register_component: null descriptor");
        }
        // SAFETY: the host passes a valid desc.
        let desc = unsafe { &*desc };
        // SAFETY: as above.
        match unsafe { register_component_impl(world, desc) } {
            Ok(id) => id,
            Err(message) => fail(world, message),
        }
    })
}

pub(crate) unsafe extern "C" fn component_id_by_name(
    world: *mut World,
    name: *const u8,
    len: usize,
) -> u32 {
    guard(world, ERR_ID, |world| {
        // SAFETY: valid UTF-8 name per the contract.
        match unsafe { str_from(name, len) } {
            Ok(name) => match world.components().id_by_name(name) {
                Some(id) => id.index() as u32,
                None => fail(world, format!("no component named `{name}`")),
            },
            Err(m) => fail(world, m),
        }
    })
}

pub(crate) unsafe extern "C" fn register_system(
    world: *mut World,
    desc: *const SystemDescFfi,
) -> u32 {
    guard(world, ERR_ID, |world| {
        if desc.is_null() {
            return fail(world, "register_system: null descriptor");
        }
        // SAFETY: the host passes a valid desc.
        let desc = unsafe { &*desc };
        // SAFETY: as above.
        let name = match unsafe { str_from(desc.name, desc.name_len) } {
            Ok(n) => n.to_owned(),
            Err(m) => return fail(world, m),
        };
        let stage = match Stage::ALL.get(desc.stage as usize) {
            Some(&s) => s,
            None => {
                return fail(world, format!("unknown stage {}", desc.stage));
            }
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
    })
}

/// Reads an id list, rejecting ids that name no component.
///
/// # Safety
/// `list` points at `len` valid ids.
unsafe fn ids(
    world: &World,
    list: &IdList,
    what: &str,
) -> Result<Vec<ComponentId>, String> {
    if list.len == 0 {
        return Ok(Vec::new());
    }
    if list.ptr.is_null() {
        return Err(format!("query_create: null `{what}` list"));
    }
    // SAFETY: per contract.
    let raw = unsafe { slice::from_raw_parts(list.ptr, list.len) };
    raw.iter()
        .map(|&i| {
            check_component(world, i, what)?;
            Ok(ComponentId::from_index(i))
        })
        .collect()
}

/// # Safety
/// `d` and the lists it holds are valid.
unsafe fn query_desc(
    world: &World,
    d: &QueryDescFfi,
) -> Result<QueryDesc, String> {
    // SAFETY: per contract.
    unsafe {
        Ok(QueryDesc {
            read: ids(world, &d.read, "read")?,
            write: ids(world, &d.write, "write")?,
            with: ids(world, &d.with, "with")?,
            without: ids(world, &d.without, "without")?,
            optional: ids(world, &d.optional, "optional")?,
            added: ids(world, &d.added, "added")?,
            changed: ids(world, &d.changed, "changed")?,
        })
    }
}

pub(crate) unsafe extern "C" fn query_create(
    world: *mut World,
    desc: *const QueryDescFfi,
) -> u32 {
    guard(world, ERR_ID, |world| {
        if desc.is_null() {
            return fail(world, "query_create: null descriptor");
        }
        // SAFETY: the host passes a valid desc.
        let desc = match unsafe { query_desc(world, &*desc) } {
            Ok(d) => d,
            Err(m) => return fail(world, m),
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
        let owner = st.current_owner;
        let slot = QuerySlot {
            state: Some(QueryState::new(desc)),
            owner,
            live: true,
        };
        let id = match st.free_queries.pop() {
            Some(i) => {
                st.queries[i as usize] = slot;
                i
            }
            None => {
                st.queries.push(slot);
                (st.queries.len() - 1) as u32
            }
        };
        let owned = st
            .queries
            .iter()
            .filter(|s| s.live && s.owner == owner)
            .count();
        if owned == QUERY_WARN_LIMIT {
            eprintln!(
                "[script warn] one script assembly now holds {owned} live \
                 queries; is a query being built on every run instead of \
                 once in OnInitialize?"
            );
        }
        id
    })
}

pub(crate) unsafe extern "C" fn query_free(world: *mut World, query: u32) {
    guard(world, (), |world| {
        let st = state(world);
        let Some(slot) = st.queries.get_mut(query as usize) else {
            return set_error(world, format!("query_free: no query {query}"));
        };
        if slot.live && slot.state.is_some() {
            slot.live = false;
            slot.state = None;
            st.free_queries.push(query);
        }
    });
}

pub(crate) unsafe extern "C" fn query_next(
    world: *mut World,
    query: u32,
    cursor: *mut Cursor,
    out: *mut ChunkFfi,
) -> u8 {
    guard(world, 0, |world| {
        if cursor.is_null() || out.is_null() {
            set_error(world, "query_next: null cursor or output");
            return 0;
        }
        let st = state(world);
        let last_run = st.current_last_run;
        // Check the query out so it can borrow the world alongside the state.
        let Some(mut q) = st
            .queries
            .get_mut(query as usize)
            .filter(|s| s.live)
            .and_then(|s| s.state.take())
        else {
            set_error(world, format!("query_next: no query {query}"));
            return 0;
        };
        // SAFETY: the cursor lives in the host's memory for the whole
        // iteration.
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
        state(world).queries[query as usize].state = Some(q);
        found
    })
}

pub(crate) unsafe extern "C" fn get_ptr(
    world: *mut World,
    entity: u64,
    component: u32,
    write: u8,
) -> *mut u8 {
    guard(world, std::ptr::null_mut(), |world| {
        if let Err(m) = check_component(world, component, "get_ptr") {
            set_error(world, m);
            return std::ptr::null_mut();
        }
        let e = Entity::from_bits(entity);
        let c = ComponentId::from_index(component);
        // A dead entity or one without the component is an ordinary null.
        let Some(loc) = world.location(e) else {
            return std::ptr::null_mut();
        };
        let Some(column) = world.archetypes().get(loc.archetype).column(c)
        else {
            return std::ptr::null_mut();
        };
        if write != 0 {
            // SAFETY: a script system has exclusive access while it runs.
            unsafe { column.changed_cell(loc.row).write(world.change_tick()) };
        }
        column.get_ptr(loc.row)
    })
}

pub(crate) unsafe extern "C" fn is_alive(world: *mut World, entity: u64) -> u8 {
    guard(world, 0, |world| {
        u8::from(world.is_alive(Entity::from_bits(entity)))
    })
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
    guard(world, Entity::PLACEHOLDER.to_bits(), |world| {
        world.entities().reserve().to_bits()
    })
}

pub(crate) unsafe extern "C" fn cmd_despawn(world: *mut World, entity: u64) {
    guard(world, (), |world| {
        // SAFETY: called from a running script system.
        match unsafe { queue(world) } {
            Some(q) => q.despawn(Entity::from_bits(entity)),
            None => set_error(world, "cmd_despawn outside a system"),
        }
    });
}

pub(crate) unsafe extern "C" fn cmd_insert(
    world: *mut World,
    entity: u64,
    component: u32,
    bytes: *const u8,
    len: usize,
) {
    guard(world, (), |world| {
        // Everything is checked now, while the script that made the mistake
        // is still on the stack; a bad command would otherwise panic when the
        // queue is applied at the sync point.
        let c = ComponentId::from_index(component);
        let Some(info) = world.components().get(c) else {
            return set_error(
                world,
                format!("cmd_insert: unknown component id {component}"),
            );
        };
        let (layout, drop) = (info.layout, info.drop);
        if len != layout.size() {
            return set_error(
                world,
                format!(
                    "cmd_insert: expected {} bytes, got {len}",
                    layout.size()
                ),
            );
        }
        if bytes.is_null() && len != 0 {
            return set_error(world, "cmd_insert: null value");
        }
        // SAFETY: called from a running script system.
        match unsafe { queue(world) } {
            // SAFETY: `bytes` holds a value of the component's layout; for a
            // managed component its ownership (the handle) moves into the
            // queue.
            Some(q) => unsafe {
                q.insert_by_id(
                    Entity::from_bits(entity),
                    c,
                    layout,
                    drop,
                    bytes,
                );
            },
            None => set_error(world, "cmd_insert outside a system"),
        }
    });
}

pub(crate) unsafe extern "C" fn cmd_remove(
    world: *mut World,
    entity: u64,
    component: u32,
) {
    guard(world, (), |world| {
        if let Err(m) = check_component(world, component, "cmd_remove") {
            return set_error(world, m);
        }
        // SAFETY: called from a running script system.
        match unsafe { queue(world) } {
            Some(q) => q.remove_by_id(
                Entity::from_bits(entity),
                ComponentId::from_index(component),
            ),
            None => set_error(world, "cmd_remove outside a system"),
        }
    });
}

pub(crate) unsafe extern "C" fn log(level: u32, text: *const u8, len: usize) {
    // A panic must not unwind into the runtime; logging has no world to
    // report to, so it is dropped.
    let _ = catch_unwind(|| {
        // SAFETY: valid UTF-8 per the contract.
        let text = unsafe { str_from(text, len) }.unwrap_or("<invalid utf-8>");
        let tag = ["trace", "info", "warn", "error"]
            .get(level as usize)
            .unwrap_or(&"log");
        eprintln!("[script {tag}] {text}");
    });
}

pub(crate) unsafe extern "C" fn last_error(
    world: *mut World,
    buffer: *mut u8,
    capacity: usize,
) -> usize {
    guard(world, 0, |world| {
        let message = &state(world).last_error;
        let n = message.len().min(capacity);
        if n > 0 {
            // SAFETY: `buffer` has room for `capacity` bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(message.as_ptr(), buffer, n);
            }
        }
        message.len()
    })
}

pub(crate) unsafe extern "C" fn resource_id_by_name(
    world: *mut World,
    name: *const u8,
    len: usize,
) -> u32 {
    guard(world, ERR_ID, |world| {
        // SAFETY: valid UTF-8 name per the contract.
        let name = match unsafe { str_from(name, len) } {
            Ok(n) => n,
            Err(m) => return fail(world, m),
        };
        match world.exposed_resource_by_name(name) {
            Some(id) => id,
            None => fail(world, format!("no resource `{name}` is exposed")),
        }
    })
}

pub(crate) unsafe extern "C" fn resource_ptr(
    world: *mut World,
    id: u32,
    write: u8,
) -> *mut u8 {
    guard(world, std::ptr::null_mut(), |world| {
        match world.exposed_resource_ptr(id) {
            Some(p) => {
                state(world).touched.push((id, write != 0));
                p
            }
            None => {
                set_error(
                    world,
                    format!("resource {id} is unknown or was not inserted"),
                );
                std::ptr::null_mut()
            }
        }
    })
}

pub(crate) unsafe extern "C" fn input_axis_id(
    world: *mut World,
    name: *const u8,
    len: usize,
) -> u32 {
    guard(world, ERR_ID, |world| {
        // SAFETY: valid UTF-8 name per the contract.
        let name = match unsafe { str_from(name, len) } {
            Ok(n) => n,
            Err(m) => return fail(world, m),
        };
        match world
            .resource::<delta::InputMap>()
            .and_then(|m| m.slot(name))
        {
            Some(slot) => slot,
            None => fail(world, format!("no input axis named `{name}`")),
        }
    })
}

/// Field names compare without case or underscores, since Rust writes
/// `fixed_delta` where C# writes `FixedDelta`.
fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| *c != '_')
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    norm(a) == norm(b)
}

/// Like [`types_equal`], but field names match loosely (see [`same_name`]).
fn layouts_match(
    ra: &TypeRegistry,
    a: &FieldType,
    rb: &TypeRegistry,
    b: &FieldType,
) -> bool {
    match (a, b) {
        (FieldType::Struct(x), FieldType::Struct(y)) => {
            let (ta, tb) = (ra.get(*x), rb.get(*y));
            ta.size == tb.size
                && ta.align == tb.align
                && ta.fields.len() == tb.fields.len()
                && ta.fields.iter().zip(tb.fields.iter()).all(|(fa, fb)| {
                    same_name(&fa.name, &fb.name)
                        && fa.offset == fb.offset
                        && layouts_match(ra, &fa.ty, rb, &fb.ty)
                })
        }
        (
            FieldType::Array {
                elem: ea,
                len: la,
                stride: sa,
            },
            FieldType::Array {
                elem: eb,
                len: lb,
                stride: sb,
            },
        ) => la == lb && sa == sb && layouts_match(ra, ea, rb, eb),
        _ => types_equal(ra, a, rb, b),
    }
}

fn type_text(registry: &TypeRegistry, ty: &FieldType) -> String {
    match ty {
        FieldType::Array { elem, len, .. } => {
            format!("[{}; {len}]", type_text(registry, elem))
        }
        FieldType::Struct(id) => registry.get(*id).name.to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn layout_text(registry: &TypeRegistry, id: ReflectId) -> String {
    let t = registry.get(id);
    let fields: Vec<_> = t
        .fields
        .iter()
        .map(|f| {
            format!("{}@{}:{}", f.name, f.offset, type_text(registry, &f.ty))
        })
        .collect();
    format!("size {} align {} [{}]", t.size, t.align, fields.join(", "))
}

/// Says what differs between two layouts, naming the first field that does.
fn layout_difference(
    rust: (&TypeRegistry, ReflectId),
    cs: (&TypeRegistry, ReflectId),
) -> String {
    let (a, b) = (rust.0.get(rust.1), cs.0.get(cs.1));
    let first = a.fields.iter().zip(b.fields.iter()).enumerate().find_map(
        |(i, (fa, fb))| {
            if !same_name(&fa.name, &fb.name) {
                Some(format!(
                    "field {i} is `{}` in Rust but `{}` in C# (renamed or \
                     out of order)",
                    fa.name, fb.name
                ))
            } else if fa.offset != fb.offset {
                Some(format!(
                    "field `{}` is at offset {} in Rust but {} in C#",
                    fa.name, fa.offset, fb.offset
                ))
            } else if !layouts_match(rust.0, &fa.ty, cs.0, &fb.ty) {
                Some(format!(
                    "field `{}` is {} in Rust but {} in C#",
                    fa.name,
                    type_text(rust.0, &fa.ty),
                    type_text(cs.0, &fb.ty)
                ))
            } else {
                None
            }
        },
    );
    if let Some(m) = first {
        return m;
    }
    if a.fields.len() != b.fields.len() {
        let (more, side) = if a.fields.len() > b.fields.len() {
            (&a.fields[b.fields.len()], "Rust")
        } else {
            (&b.fields[a.fields.len()], "C#")
        };
        return format!("field `{}` exists only in {side}", more.name);
    }
    format!("size/alignment differ ({} vs {})", a.size, b.size)
}

pub(crate) unsafe extern "C" fn check_layout(
    world: *mut World,
    name: *const u8,
    name_len: usize,
    size: usize,
    align: usize,
    fields: *const FieldDescFfi,
    field_count: usize,
) -> u32 {
    guard(world, ERR_ID, |world| {
        // SAFETY: valid UTF-8 name per the contract.
        let name = match unsafe { str_from(name, name_len) } {
            Ok(n) => n,
            Err(m) => return fail(world, m),
        };
        let Some(rust_id) = world
            .exposed_resource_by_name(name)
            .and_then(|r| world.exposed_resource_reflect(r))
            .or_else(|| {
                let c = world.components().id_by_name(name)?;
                world.components().info(c).reflect
            })
        else {
            return fail(
                world,
                format!("C# mirrors `{name}`, which Rust does not expose"),
            );
        };
        let descs = if field_count == 0 {
            &[]
        } else {
            // SAFETY: per contract.
            unsafe { slice::from_raw_parts(fields, field_count) }
        };
        let mut scratch = world.reflection().clone();
        let mut cs_fields = Vec::with_capacity(descs.len());
        for f in descs {
            // SAFETY: per contract.
            let fname = match unsafe { str_from(f.name, f.name_len) } {
                Ok(n) => n,
                Err(m) => return fail(world, m),
            };
            let ty = match field_type(&scratch, f) {
                Ok(t) => t,
                Err(m) => return fail(world, format!("`{name}.{fname}`: {m}")),
            };
            cs_fields.push(FieldReflect::new(
                fname,
                f.offset as usize,
                ty,
                FieldAttrs::default(),
            ));
        }
        let mirror = format!("{name}#csharp");
        let cs_id =
            match scratch.register_dynamic(&mirror, size, align, cs_fields) {
                Ok(id) => id,
                Err(m) => return fail(world, m),
            };
        let (rust, cs) = (FieldType::Struct(rust_id), FieldType::Struct(cs_id));
        if layouts_match(world.reflection(), &rust, &scratch, &cs) {
            return 0;
        }
        let difference =
            layout_difference((world.reflection(), rust_id), (&scratch, cs_id));
        let message = format!(
            "the C# mirror of `{name}` does not match Rust: {difference}\n  \
             Rust: {}\n  C#:   {}",
            layout_text(world.reflection(), rust_id),
            layout_text(&scratch, cs_id),
        );
        fail(world, message)
    })
}

pub(crate) unsafe extern "C" fn entity_to_scene_id(
    world: *mut World,
    entity: u64,
    buffer: *mut u8,
    capacity: usize,
) -> usize {
    guard(world, 0, |world| {
        let Some(id) = world.scene_id_for_reference(Entity::from_bits(entity))
        else {
            return 0;
        };
        let text = id.to_string();
        let n = text.len().min(capacity);
        if n > 0 {
            // SAFETY: `buffer` has room for `capacity` bytes.
            unsafe { std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, n) };
        }
        text.len()
    })
}

pub(crate) unsafe extern "C" fn scene_id_to_entity(
    world: *mut World,
    text: *const u8,
    len: usize,
) -> u64 {
    guard(world, Entity::PLACEHOLDER.to_bits(), |world| {
        // SAFETY: valid UTF-8 per the contract.
        let Ok(text) = (unsafe { str_from(text, len) }) else {
            return Entity::PLACEHOLDER.to_bits();
        };
        delta::SceneId::parse(text)
            .and_then(|id| world.entity_for_reference(id))
            .unwrap_or(Entity::PLACEHOLDER)
            .to_bits()
    })
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
        query_free,
        resource_id_by_name,
        resource_ptr,
        check_layout,
        input_axis_id,
        entity_to_scene_id,
        scene_id_to_entity,
    }
}

#[cfg(test)]
mod fuzz {
    use std::mem::MaybeUninit;

    use proptest::prelude::*;

    use super::*;

    /// A host call with arbitrary arguments. Buffers really are as long as
    /// the length they are passed with: a wrong length is undefined
    /// behaviour no guard can catch, and C# never produces one.
    #[derive(Clone, Debug)]
    enum Call {
        GetPtr(u64, u32, u8),
        IsAlive(u64),
        CmdSpawn,
        CmdDespawn(u64),
        CmdInsert(u64, u32, Vec<u8>),
        CmdRemove(u64, u32),
        ComponentIdByName(Vec<u8>),
        RegisterComponent(Vec<u8>, usize, usize, u32),
        QueryCreate([Vec<u32>; 7]),
        QueryNext(u32, u8),
        QueryFree(u32),
        LastError,
    }

    fn id() -> impl Strategy<Value = u32> {
        prop_oneof![
            8 => 0u32..12, // some real component ids, some not
            2 => any::<u32>(),
            1 => Just(u32::MAX),
        ]
    }

    fn entity(live: Vec<u64>) -> impl Strategy<Value = u64> {
        prop_oneof![
            3 => proptest::sample::select(live),
            2 => any::<u64>(),
            1 => Just(u64::MAX),
            1 => Just(0u64),
        ]
    }

    fn call(live: Vec<u64>) -> impl Strategy<Value = Call> {
        let e = || entity(live.clone());
        prop_oneof![
            (e(), id(), 0u8..3).prop_map(|(e, c, w)| Call::GetPtr(e, c, w)),
            e().prop_map(Call::IsAlive),
            Just(Call::CmdSpawn),
            e().prop_map(Call::CmdDespawn),
            (e(), id(), proptest::collection::vec(any::<u8>(), 0..12))
                .prop_map(|(e, c, b)| Call::CmdInsert(e, c, b)),
            (e(), id()).prop_map(|(e, c)| Call::CmdRemove(e, c)),
            proptest::collection::vec(any::<u8>(), 0..16)
                .prop_map(Call::ComponentIdByName),
            (
                proptest::collection::vec(any::<u8>(), 0..16),
                0usize..64,
                0usize..64,
                0u32..6,
            )
                .prop_map(|(n, s, a, k)| Call::RegisterComponent(n, s, a, k)),
            [
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
                proptest::collection::vec(id(), 0..3),
            ]
            .prop_map(Call::QueryCreate),
            (0u32..8, 1u8..3).prop_map(|(q, n)| Call::QueryNext(q, n)),
            (0u32..8).prop_map(Call::QueryFree),
            Just(Call::LastError),
        ]
    }

    fn world() -> (World, Vec<u64>) {
        let mut world = World::new();
        world.insert_resource(ScriptState::new());
        let mut ids = Vec::new();
        for i in 0..4 {
            ids.push(world.components_mut().register_raw(ComponentInfo {
                name: format!("fuzz{i}").into(),
                layout: Layout::from_size_align(4 * (i + 1), 4).unwrap(),
                drop: None,
                clone: None,
                kind: Kind::ScriptUnmanaged,
                storage: Storage::Table,
                hooks: delta::ComponentHooks::default(),
                reflect: None,
                requires: Box::default(),
                external: None,
            }));
        }
        let live = (0..4)
            .map(|_| {
                let e = world.spawn();
                // SAFETY: zeroed plain data of the registered layout.
                unsafe { world.insert_by_id(e, ids[0], [0u8; 4].as_ptr()) }
                    .unwrap();
                e.to_bits()
            })
            .collect();
        (world, live)
    }

    fn run(world: &mut World, call: &Call, queue: &mut CommandQueue) {
        let w: *mut World = world;
        let known = world.components().len();
        state(world).last_error.clear();
        state(world).current_queue = queue;
        // SAFETY: every pointer passed is valid for the length passed.
        unsafe {
            match call {
                Call::GetPtr(e, c, wr) => {
                    let p = get_ptr(w, *e, *c, *wr);
                    if p.is_null() && *c as usize >= known {
                        assert!(!state(world).last_error.is_empty());
                    }
                }
                Call::IsAlive(e) => {
                    is_alive(w, *e);
                }
                Call::CmdSpawn => {
                    cmd_spawn(w);
                }
                Call::CmdDespawn(e) => cmd_despawn(w, *e),
                Call::CmdInsert(e, c, bytes) => {
                    cmd_insert(w, *e, *c, bytes.as_ptr(), bytes.len());
                    let bad = world
                        .components()
                        .get(ComponentId::from_index(*c))
                        .is_none_or(|i| i.layout.size() != bytes.len());
                    if bad {
                        assert!(
                            !state(world).last_error.is_empty(),
                            "insert {c} len {}",
                            bytes.len()
                        );
                    }
                }
                Call::CmdRemove(e, c) => {
                    cmd_remove(w, *e, *c);
                    if *c as usize >= known {
                        assert!(!state(world).last_error.is_empty());
                    }
                }
                Call::ComponentIdByName(n) => {
                    component_id_by_name(w, n.as_ptr(), n.len());
                }
                Call::RegisterComponent(n, size, align, kind) => {
                    let d = ComponentDescFfi {
                        name: n.as_ptr(),
                        name_len: n.len(),
                        size: *size,
                        align: *align,
                        kind: *kind,
                        field_count: 0,
                        fields: std::ptr::null(),
                        requires: IdList {
                            ptr: std::ptr::null(),
                            len: 0,
                        },
                    };
                    register_component(w, &d);
                }
                Call::QueryCreate(lists) => {
                    let l = |i: usize| IdList {
                        ptr: lists[i].as_ptr(),
                        len: lists[i].len(),
                    };
                    let d = QueryDescFfi {
                        read: l(0),
                        write: l(1),
                        with: l(2),
                        without: l(3),
                        optional: l(4),
                        added: l(5),
                        changed: l(6),
                    };
                    query_create(w, &d);
                }
                Call::QueryNext(q, rounds) => {
                    let mut cursor = Cursor::default();
                    let mut out = MaybeUninit::<ChunkFfi>::zeroed();
                    for _ in 0..*rounds {
                        query_next(w, *q, &mut cursor, out.as_mut_ptr());
                    }
                }
                Call::QueryFree(q) => query_free(w, *q),
                Call::LastError => {
                    let mut buf = [0u8; 8];
                    last_error(w, buf.as_mut_ptr(), buf.len());
                }
            }
        }
        state(world).current_queue = std::ptr::null_mut();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// No host function panics or corrupts anything whatever it is
        /// handed, and the commands it queues apply cleanly.
        #[test]
        fn host_functions_survive_arbitrary_arguments(
            calls in {
                let (_, live) = world();
                proptest::collection::vec(call(live), 1..40)
            }
        ) {
            let (mut world, _) = world();
            let mut queue = CommandQueue::default();
            for c in &calls {
                run(&mut world, c, &mut queue);
            }
            queue.apply(&mut world);
        }
    }

    #[test]
    fn freed_queries_are_reused() {
        let (mut world, _) = world();
        let w: *mut World = &mut world;
        let d = QueryDescFfi {
            read: IdList {
                ptr: [0u32].as_ptr(),
                len: 1,
            },
            write: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
            with: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
            without: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
            optional: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
            added: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
            changed: IdList {
                ptr: std::ptr::null(),
                len: 0,
            },
        };
        state(&mut world).current_owner = 7;
        // SAFETY: valid world and descriptor.
        let a = unsafe { query_create(w, &d) };
        // SAFETY: as above.
        let b = unsafe { query_create(w, &d) };
        assert_ne!(a, b);
        // SAFETY: as above.
        unsafe { query_free(w, a) };
        // SAFETY: as above.
        assert_eq!(unsafe { query_create(w, &d) }, a, "slot reused");
        state(&mut world).free_queries_of(7);
        assert_eq!(state(&mut world).query_counts(), (0, 2));
    }
}
