//! Starting the scripting runtime for a world, loading and hot-reloading
//! script assemblies, and the systems they provide.

use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    mem::MaybeUninit,
    path::{Path, PathBuf},
};

use delta::{
    Access,
    ComponentId,
    Entity,
    Kind,
    Schedule,
    System,
    Tick,
    World,
    reflect::{FieldType, RegistryPair, migrate_value, types_equal},
};

use crate::{
    bridge::{
        self,
        FREE_HANDLE,
        PendingSystem,
        RemovedRecord,
        ScriptState,
        state,
    },
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

/// The function table the managed side filled in, with no holes.
#[derive(Clone, Copy)]
struct Api {
    load_assembly:
        unsafe extern "C" fn(*mut World, *const u8, usize, *mut u32) -> i32,
    run_system: unsafe extern "C" fn(u32, *mut World),
    live_handles: unsafe extern "C" fn() -> u64,
    last_error: unsafe extern "C" fn(*mut u8, usize) -> usize,
    unload_assembly: unsafe extern "C" fn(u32),
    assembly_alive: unsafe extern "C" fn(u32) -> u8,
    serialize_behaviour: unsafe extern "C" fn(u64, *mut u8, usize) -> usize,
    deserialize_behaviour: unsafe extern "C" fn(u32, *const u8, usize) -> u64,
    take_warnings: unsafe extern "C" fn(*mut u8, usize) -> usize,
    free_handle: unsafe extern "C" fn(u64),
}

/// One loaded script assembly and what it registered.
#[derive(Clone, Debug)]
pub struct ScriptAssembly {
    pub handle: u32,
    pub path: PathBuf,
    /// The systems it declared, as installed by [`ScriptHost::install`].
    pub systems: Vec<PendingSystem>,
    /// Every script component (struct, tag or behaviour class) it declares.
    pub components: Vec<ComponentId>,
}

/// What a hot reload did.
#[derive(Debug, Default)]
pub struct ReloadReport {
    /// Whether the old assembly's load context was verifiably collected.
    /// `false` means something still references the old code, typically a
    /// static event subscription that was not undone in `OnUnload`.
    pub unloaded: bool,
    /// Components whose columns were rebuilt for a new layout.
    pub migrated: Vec<String>,
    /// Behaviours carried over into instances of their new class.
    pub restored_behaviours: usize,
    /// Classes that no longer exist, with the entities that lost them. Their
    /// state is kept and comes back if the class does.
    pub removed_behaviours: Vec<(String, Vec<Entity>)>,
    /// Behaviours restored from records of an earlier reload.
    pub restored_from_records: usize,
    /// Fields that were dropped or reset, state that did not fit, and the
    /// like.
    pub warnings: Vec<String>,
}

/// The scripting runtime attached to one world.
///
/// The C# side keeps per-world registries in statics, so for now one world
/// per process may use scripting.
pub struct ScriptHost {
    api: Api,
}

struct Snapshot {
    entity: Entity,
    component: ComponentId,
    bytes: Vec<u8>,
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
        let api = Api {
            load_assembly: api
                .load_assembly
                .ok_or_else(|| missing("load_assembly"))?,
            run_system: api.run_system.ok_or_else(|| missing("run_system"))?,
            live_handles: api
                .live_handles
                .ok_or_else(|| missing("live_handles"))?,
            last_error: api.last_error.ok_or_else(|| missing("last_error"))?,
            unload_assembly: api
                .unload_assembly
                .ok_or_else(|| missing("unload_assembly"))?,
            assembly_alive: api
                .assembly_alive
                .ok_or_else(|| missing("assembly_alive"))?,
            serialize_behaviour: api
                .serialize_behaviour
                .ok_or_else(|| missing("serialize_behaviour"))?,
            deserialize_behaviour: api
                .deserialize_behaviour
                .ok_or_else(|| missing("deserialize_behaviour"))?,
            take_warnings: api
                .take_warnings
                .ok_or_else(|| missing("take_warnings"))?,
            free_handle: api
                .free_handle
                .ok_or_else(|| missing("free_handle"))?,
        };
        let _ = FREE_HANDLE.set(api.free_handle);
        Ok(Self { api })
    }

    fn managed_error_text(&self) -> String {
        let mut buffer = vec![0u8; 8192];
        // SAFETY: the pointer and capacity describe `buffer`.
        let len =
            unsafe { (self.api.last_error)(buffer.as_mut_ptr(), buffer.len()) };
        buffer.truncate(len.min(buffer.len()));
        String::from_utf8_lossy(&buffer).into_owned()
    }

    /// The warnings the managed side collected since the last call.
    fn take_warnings(&self) -> Vec<String> {
        let mut buffer = vec![0u8; 64 * 1024];
        // SAFETY: the pointer and capacity describe `buffer`.
        let len = unsafe {
            (self.api.take_warnings)(buffer.as_mut_ptr(), buffer.len())
        };
        buffer.truncate(len.min(buffer.len()));
        String::from_utf8_lossy(&buffer)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn load_raw(
        &self,
        world: &mut World,
        assembly: &Path,
    ) -> Result<ScriptAssembly, ScriptError> {
        state(world).load_components.clear();
        let path = assembly.to_string_lossy();
        let mut handle = 0u32;
        // SAFETY: valid world pointer, UTF-8 path bytes and out-pointer.
        let status = unsafe {
            (self.api.load_assembly)(
                world,
                path.as_ptr(),
                path.len(),
                &mut handle,
            )
        };
        if status < 0 {
            return Err(ScriptError::Script(self.managed_error_text()));
        }
        let st = state(world);
        Ok(ScriptAssembly {
            handle,
            path: assembly.to_owned(),
            systems: std::mem::take(&mut st.pending_systems),
            components: std::mem::take(&mut st.load_components),
        })
    }

    /// Loads a script assembly: its components are registered in `world`,
    /// behaviours saved by an earlier failed or class-removing reload are
    /// restored, and the systems it declares are available for
    /// [`install`](Self::install).
    pub fn load_assembly(
        &self,
        world: &mut World,
        assembly: &Path,
    ) -> Result<ScriptAssembly, ScriptError> {
        let loaded = self.load_raw(world, assembly)?;
        self.restore_records(world, &loaded, &mut ReloadReport::default());
        Ok(loaded)
    }

    /// Adds the assembly's systems to `schedule`: by stage, then by their
    /// declared order, then by name, so the run order is deterministic.
    pub fn install(&self, schedule: &mut Schedule, assembly: &ScriptAssembly) {
        let mut systems = assembly.systems.clone();
        systems.sort_by(|a, b| {
            (a.stage, a.order, &a.name).cmp(&(b.stage, b.order, &b.name))
        });
        for s in systems {
            schedule.add_boxed_system(
                s.stage,
                Box::new(ScriptSystem {
                    id: s.id,
                    assembly: assembly.handle,
                    name: s.name,
                    run: self.api.run_system,
                    queue: delta::CommandQueue::default(),
                    last_run: Tick::new(0),
                    access: Access::default(),
                }),
            );
        }
    }

    /// How many managed component values currently hold a `GCHandle`.
    pub fn live_handles(&self) -> u64 {
        // SAFETY: a plain query function of the managed side.
        unsafe { (self.api.live_handles)() }
    }

    fn serialize(&self, handle: u64) -> Option<Vec<u8>> {
        // First ask for the size, then fetch; only the second call tears the
        // object down.
        // SAFETY: a null buffer with capacity 0 is a size query.
        let n = unsafe {
            (self.api.serialize_behaviour)(handle, std::ptr::null_mut(), 0)
        };
        if n == usize::MAX {
            return None;
        }
        let mut bytes = vec![0u8; n];
        // SAFETY: the pointer and capacity describe `bytes`.
        let m = unsafe {
            (self.api.serialize_behaviour)(handle, bytes.as_mut_ptr(), n)
        };
        (m == n).then_some(bytes)
    }

    fn deserialize(&self, component: ComponentId, bytes: &[u8]) -> u64 {
        // SAFETY: the pointer and length describe `bytes`.
        unsafe {
            (self.api.deserialize_behaviour)(
                component.index() as u32,
                bytes.as_ptr(),
                bytes.len(),
            )
        }
    }

    /// Step 2 of a reload: saves every behaviour of the given classes and
    /// frees its `GCHandle`, leaving a null handle in the column (which the
    /// column's drop function ignores).
    fn snapshot_behaviours(
        &self,
        world: &World,
        components: &[ComponentId],
        report: &mut ReloadReport,
    ) -> Vec<Snapshot> {
        let mut out = Vec::new();
        for &c in components {
            if world.components().info(c).kind != Kind::ScriptManaged {
                continue;
            }
            for arch in world.archetypes().iter() {
                let Some(column) = arch.column(c) else {
                    continue;
                };
                for (row, &entity) in arch.entities().iter().enumerate() {
                    let cell = column.get_ptr(row).cast::<u64>();
                    // SAFETY: a managed column holds one u64 handle per row.
                    let handle = unsafe { cell.read_unaligned() };
                    if handle == 0 {
                        continue;
                    }
                    match self.serialize(handle) {
                        Some(bytes) => out.push(Snapshot {
                            entity,
                            component: c,
                            bytes,
                        }),
                        None => report.warnings.push(format!(
                            "could not save the {} on {entity:?}; it is lost",
                            world.components().info(c).name
                        )),
                    }
                    // SAFETY: the handle is live and owned by this cell; the
                    // cell is nulled so nothing frees it twice.
                    unsafe {
                        (self.api.free_handle)(handle);
                        cell.write_unaligned(0);
                    }
                }
            }
        }
        out
    }

    /// Step 6: rebuilds the columns of script structs whose layout or field
    /// types changed, converting each value by field name.
    fn migrate_layouts(
        &self,
        world: &mut World,
        old_registry: &delta::reflect::TypeRegistry,
        old_infos: &HashMap<ComponentId, (std::alloc::Layout, Kind)>,
        new_components: &[ComponentId],
        report: &mut ReloadReport,
    ) {
        let new_registry = world.reflection().clone();
        let registries = RegistryPair {
            old: old_registry,
            new: &new_registry,
        };
        for &c in new_components {
            let info = world.components().info(c);
            if !matches!(info.kind, Kind::ScriptUnmanaged | Kind::Tag) {
                continue;
            }
            let (Some(reflect), Some(&(old_layout, old_kind))) =
                (info.reflect, old_infos.get(&c))
            else {
                continue; // a component that did not exist before
            };
            let ty = FieldType::Struct(reflect);
            let unchanged = old_layout == info.layout
                && old_kind == info.kind
                && types_equal(old_registry, &ty, &new_registry, &ty);
            if unchanged {
                continue;
            }
            let name = info.name.to_string();
            let mut warnings = BTreeSet::new();
            let mut one = Vec::new();
            // SAFETY: the component's info already describes the new layout;
            // `convert` fully initializes the (plain-data) new value.
            unsafe {
                world.rebuild_columns(c, &mut |old, new| {
                    one.clear();
                    migrate_value(
                        registries, &ty, old, &ty, new, &name, &mut one,
                    );
                    warnings.extend(one.drain(..));
                });
            }
            report.warnings.extend(warnings);
            report.migrated.push(name);
        }
    }

    /// Step 7 (and step 8 for classes that are gone): puts saved behaviours
    /// back into the columns of their new class, in place.
    fn restore_behaviours(
        &self,
        world: &mut World,
        snapshots: Vec<Snapshot>,
        new: &ScriptAssembly,
        report: &mut ReloadReport,
    ) {
        let mut removed: HashMap<ComponentId, Vec<Entity>> = HashMap::new();
        for snap in snapshots {
            let present = new.components.contains(&snap.component);
            let handle = if present {
                self.deserialize(snap.component, &snap.bytes)
            } else {
                0
            };
            if handle != 0 {
                let cell = world
                    .get_by_id(snap.entity, snap.component)
                    .expect("the entity still has the component")
                    .cast_mut()
                    .cast::<u64>();
                // SAFETY: the cell holds the null handle left by the
                // snapshot; the new object's handle takes its place.
                unsafe { cell.write_unaligned(handle) };
                report.restored_behaviours += 1;
                continue;
            }
            // The class is gone (or its state could not be rebuilt): take
            // the component off the entity, but keep the state.
            let name = world.components().info(snap.component).name.to_string();
            let _ = world.remove_by_id(snap.entity, snap.component);
            removed.entry(snap.component).or_default().push(snap.entity);
            state(world).removed_records.push(RemovedRecord {
                class: name,
                entity: snap.entity,
                bytes: snap.bytes,
            });
        }
        for (c, entities) in removed {
            let name = world.components().info(c).name.to_string();
            report.warnings.push(format!(
                "{name} no longer exists: removed from {} entities (state kept)",
                entities.len()
            ));
            report.removed_behaviours.push((name, entities));
        }
    }

    /// Brings back behaviours whose class returned: records left by an
    /// earlier reload that removed the class or failed.
    fn restore_records(
        &self,
        world: &mut World,
        new: &ScriptAssembly,
        report: &mut ReloadReport,
    ) {
        let records = std::mem::take(&mut state(world).removed_records);
        let mut keep = Vec::new();
        for record in records {
            let class = world
                .components()
                .id_by_name(&record.class)
                .filter(|c| new.components.contains(c));
            let Some(c) = class else {
                keep.push(record);
                continue;
            };
            if !world.is_alive(record.entity) {
                report.warnings.push(format!(
                    "{}: dropped saved state of a despawned entity",
                    record.class
                ));
                continue;
            }
            if world.has_by_id(record.entity, c) {
                continue;
            }
            let handle = self.deserialize(c, &record.bytes);
            if handle == 0 {
                keep.push(record);
                continue;
            }
            // SAFETY: a managed value is one u64 handle; the world takes
            // ownership of it.
            let inserted = unsafe {
                world.insert_by_id(
                    record.entity,
                    c,
                    std::ptr::from_ref(&handle).cast(),
                )
            };
            if inserted.is_ok() {
                report.restored_from_records += 1;
            }
        }
        state(world).removed_records.extend(keep);
    }

    /// Replaces a loaded script assembly with a new build of it, keeping the
    /// world running:
    ///
    /// 1. Behaviours are saved and their `GCHandle`s freed.
    /// 2. The old systems leave the schedule and the old assembly is
    ///    unloaded (and checked: [`ReloadReport::unloaded`]).
    /// 3. The new assembly is loaded; components keep their ids by name.
    /// 4. Columns of structs whose layout changed are rebuilt, converting by
    ///    field name.
    /// 5. Behaviours are rebuilt from their saved state in place; classes
    ///    that no longer exist are removed from their entities, with their
    ///    state kept for if they return.
    /// 6. The new systems are installed.
    ///
    /// Call between frames, when no command buffers hold script values.
    /// Systems restart with a fresh `last_run`, so `Added`/`Changed` filters
    /// see everything once.
    pub fn reload(
        &self,
        world: &mut World,
        schedule: &mut Schedule,
        old: &ScriptAssembly,
        new_path: &Path,
    ) -> Result<(ScriptAssembly, ReloadReport), ScriptError> {
        let mut report = ReloadReport::default();
        let old_registry = world.reflection().clone();
        let old_infos: HashMap<_, _> = old
            .components
            .iter()
            .map(|&c| {
                let i = world.components().info(c);
                (c, (i.layout, i.kind))
            })
            .collect();

        let snapshots =
            self.snapshot_behaviours(world, &old.components, &mut report);

        schedule.remove_systems(|s| {
            s.as_any()
                .downcast_ref::<ScriptSystem>()
                .is_some_and(|s| s.assembly == old.handle)
        });
        // SAFETY: plain calls into the managed side; every handle to the old
        // assembly's objects was freed above.
        report.unloaded = unsafe {
            (self.api.unload_assembly)(old.handle);
            (self.api.assembly_alive)(old.handle) == 0
        };

        state(world).reloading = true;
        let loaded = self.load_raw(world, new_path);
        state(world).reloading = false;
        let new = match loaded {
            Ok(new) => new,
            Err(e) => {
                // Keep every saved behaviour for the next successful load.
                for snap in snapshots {
                    let class = world
                        .components()
                        .info(snap.component)
                        .name
                        .to_string();
                    let _ = world.remove_by_id(snap.entity, snap.component);
                    state(world).removed_records.push(RemovedRecord {
                        class,
                        entity: snap.entity,
                        bytes: snap.bytes,
                    });
                }
                return Err(e);
            }
        };

        self.migrate_layouts(
            world,
            &old_registry,
            &old_infos,
            &new.components,
            &mut report,
        );
        self.restore_behaviours(world, snapshots, &new, &mut report);
        self.restore_records(world, &new, &mut report);
        report.warnings.extend(self.take_warnings());
        // The same problem is reported once per object; say it once.
        let mut seen = BTreeSet::new();
        report.warnings.retain(|w| seen.insert(w.clone()));
        if !report.unloaded {
            report.warnings.push(
                "the old assembly is still loaded: something references its \
                 code (a static event subscription not undone in OnUnload?)"
                    .into(),
            );
        }
        self.install(schedule, &new);
        Ok((new, report))
    }
}

/// A system implemented in C#: the schedule runs it like any other, and it
/// calls back into the world through the host API.
pub struct ScriptSystem {
    id: u32,
    assembly: u32,
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

    fn as_any(&self) -> &dyn std::any::Any {
        self
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
