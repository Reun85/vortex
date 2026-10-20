# ECS Architecture & Development Plan

Oct 20, 2026 · @reun

## Goals and constraints

One archetype ECS is the single source of truth for the engine, the editor and C# scripts. There is no second object model the way Unity has GameObjects beside DOTS.

- **Fast native path.** Rust systems iterate dense, cache-friendly columns with no per-entity indirection. Physics and rendering extraction live here.
- **Event-driven layer on top.** Component lifecycle hooks, entity-targeted observers and buffered messages give the Unity-style `OnCollisionEnter` feel without leaving the ECS.
- **Scripts are components.** Every C# `Behaviour` class and every C# `IComponent` struct is a real component type with its own `ComponentId`, so queries, the inspector and serialization treat it like a native one.
- **Editor-grade.** Reflection on every component makes undo, the inspector, scene files and hot reload one shared mechanism.
- **Single-threaded first, parallel-ready.** The design keeps access sets explicit from day one, so the scheduler can go parallel later without API changes.

Non-goals for v1: relations and entity hierarchies as first-class pairs (flecs-style), networking and determinism guarantees.

## Core identity model

Everything is keyed by small integer IDs assigned at runtime, never by Rust's `TypeId`. C# defines components at load time, so the registry must accept a component described only by data.

```rust
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity(u64); // low 32 bits: index, high 32 bits: generation

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

pub enum Storage { Table, SparseSet }
pub enum Kind { Native, ScriptUnmanaged, ScriptManaged, Tag }

pub struct ComponentInfo {
    pub name: Box<str>,          // stable name, used by serialization and C# binding
    pub layout: Layout,          // size + align; zero-sized for tags
    pub drop: Option<unsafe fn(*mut u8)>,
    pub clone: Option<unsafe fn(*const u8, *mut u8)>,
    pub kind: Kind,
    pub storage: Storage,
    pub reflect: ReflectId,      // field list for inspector, undo, serialization
    pub hooks: ComponentHooks,   // on_add / on_insert / on_replace / on_remove
}

pub struct Components {
    infos: Vec<ComponentInfo>,
    by_name: HashMap<Box<str>, ComponentId>,
    by_type: HashMap<TypeId, ComponentId>, // convenience lookup for Rust types only
}
```

- **Entity allocator:** a free list of indices plus a generation per slot. A stale `Entity` fails `is_alive` instead of aliasing a new one. Reserving IDs must be atomic, because command buffers hand out entities before they exist.
- **Entity location:** a dense `Vec<(ArchetypeId, row)>` indexed by entity index, updated on every move.
- **Rust convenience:** `world.component_id::<T>()` maps `TypeId` to `ComponentId` once, so typed Rust code stays ergonomic while the storage only sees IDs.
- **Names are stable, IDs are not.** IDs depend on registration order and may differ between runs. Scene files and the C# binding always use names.

## Storage

Each unique set of table components is one archetype, stored as structure-of-arrays: one type-erased column per component, plus a column of entities. Rows are densely packed, and removal is swap-remove.

```rust
pub struct Column {
    data: BlobVec,           // type-erased, aligned, grows by doubling
    added: Vec<Tick>,        // per-row tick when the component was added
    changed: Vec<Tick>,      // per-row tick of the last write
}

pub struct Archetype {
    id: ArchetypeId,
    components: Box<[ComponentId]>,      // sorted, the archetype's identity
    columns: Box<[Column]>,              // same order as components
    entities: Vec<Entity>,
    edges: ArchetypeEdges,               // add/remove transition cache
    managed_mask: FixedBitSet,           // which columns hold GC handles
}

pub struct ArchetypeEdges {
    add: HashMap<ComponentId, ArchetypeId>,
    remove: HashMap<ComponentId, ArchetypeId>,
}
```

- **Lookup:** `HashMap<Box<[ComponentId]>, ArchetypeId>` finds an archetype by its sorted component set. Edges cache single add/remove transitions, so the common case `insert::<Velocity>()` is one hash lookup on a tiny map.
- **Moves:** moving an entity copies each shared column's bytes from the old row to the new row, drops columns that were removed, and swap-removes the old row. The swapped entity's location gets patched.
- **Sparse-set storage** is opt-in per component, for data that is added and removed often (`Selected`, `Hovered`, temporary status effects). It avoids archetype moves at the cost of slower iteration.
- **Tags** are zero-sized: they change the archetype but allocate no column bytes.
- **No fixed-size chunks in v1.** One growable table per archetype is simpler. Chunking (Unity uses 16 KB chunks) matters mainly for parallel splitting and memory reuse; the query API below already exposes iteration as "chunks" (row ranges), so it can be added later without changing callers.
- **BlobVec** is the one piece of unsafe code worth fuzzing hard: alignment, drop on overwrite, and swap-remove of the last row.

## Component kinds

Four kinds share the same storage and query machinery. They differ only in who declares them and what the column bytes mean.

| Kind | Declared in | Column holds | Iterable from | Hot reload |
| --- | --- | --- | --- | --- |
| Native | Rust, `#[derive(Component)]` | the struct, `#[repr(C)]` if script-visible | Rust and C# | engine rebuild |
| Script unmanaged | C#, `struct X : IComponent` | blittable bytes | Rust (by name, via reflection) and C# | column migration |
| Script managed | C#, `class X : Behaviour` | a `GCHandle` (8 bytes) | C# only; Rust sees an opaque handle | serialize and restore |
| Tag | either side | nothing | both | trivial |

**Why a script class is its own component type.** In Unity a script is a component, and users expect the same here. Giving every `Behaviour` class its own `ComponentId` means:

- `Query<Player>` and "all entities with `Player`" are ordinary archetype queries, not a scan over a generic script list.
- The editor's "Add Component" menu, inspector and scene files need no special case for scripts.
- Magic-method dispatch is per type: the scheduler runs `Update` on the `Player` column, then the next class, in a stable order.

**Rules for script-visible data:**

- No `String`, `Vec` or `Box` in a script-visible component. Use handles: `StrId`, `AssetHandle<T>`, `Entity` as `u64`, or a `BufferHandle` into a side arena for variable-length data.
- Managed state (lists, references to other objects) lives only on `Behaviour` objects, never in columns.
- Dropping a managed column row frees its `GCHandle`; this is the `drop` function registered in `ComponentInfo`.

## Change detection

The world keeps a global `change_tick: u32` that increments once per system run. Every column row stores an `added` and a `changed` tick, and every system remembers the tick at which it last ran.

- `Added<T>` matches rows where `added > last_run`; `Changed<T>` matches rows where `changed > last_run`.
- **Rust writes:** `Mut<T>` sets the row's `changed` tick on `DerefMut`, so reading through `&mut` without writing costs nothing.
- **C# writes:** taking a `Span<T>` for writing stamps the whole range at once. It is coarser than per-row tracking but free in the inner loop. `ref T Get<T>()` on a `Behaviour` stamps one row.
- **Wraparound:** compare with wrapping arithmetic, and every \~1 billion ticks clamp old ticks so no stored tick is more than half the range behind.
- **Consumers:** render extraction (only re-upload changed transforms), physics sync, the editor inspector (refresh only what changed), and network replication later.

## Queries and iteration

A query is compiled once into a `QueryState` that caches the list of matching archetypes and updates incrementally as new archetypes appear. Iteration is then a loop over cached archetypes, with no per-frame matching.

```rust
pub struct QueryDesc {
    read: Vec<ComponentId>,
    write: Vec<ComponentId>,
    with: Vec<ComponentId>,      // filter only, no data access
    without: Vec<ComponentId>,
    optional: Vec<ComponentId>,
    changed: Vec<ComponentId>,   // Changed<T> / Added<T> filters
}

pub struct QueryState {
    desc: QueryDesc,
    matched: Vec<(ArchetypeId, Box<[usize]>)>, // archetype + column index per term
    seen_archetypes: usize,                    // archetypes checked so far
}
```

- **Typed Rust layer:** `Query<(&Transform, &mut Velocity), (With<Player>, Without<Frozen>)>` builds a `QueryDesc` at system registration through a `WorldQuery` trait, like Bevy's. Write the dynamic layer first; the typed layer is a thin wrapper over it.
- **Chunk view:** iteration yields `(archetype, row range)` pairs with raw column pointers. Rust wraps them as slices; C# wraps them as spans. This is the one shared primitive.
- **Matching:** keep a bitset of component IDs per archetype, so matching is `required ⊆ archetype && excluded ∩ archetype = ∅`. New archetypes are checked against existing `QueryState`s lazily, on their next use.
- **Single-entity access:** `world.get::<T>(entity)` goes through the entity location and the archetype's column index map. It is fine for gameplay code and editor tools, but slower than iteration.

## Structural changes and command buffers

Spawn, despawn, insert and remove never happen while any system iterates. They are recorded into command buffers and applied at sync points between schedule stages. This one rule keeps every column pointer, Rust slice and C# span valid for the whole system run.

```rust
pub struct Commands<'w> { queue: &'w mut CommandQueue, entities: &'w Entities }

impl Commands<'_> {
    pub fn spawn(&mut self) -> EntityCommands;     // reserves an Entity atomically
    pub fn despawn(&mut self, e: Entity);
    pub fn insert_by_id(&mut self, e: Entity, c: ComponentId, bytes: &[u8]);
    pub fn remove_by_id(&mut self, e: Entity, c: ComponentId);
}
```

- **Encoding:** a flat byte queue of `(opcode, entity, component, payload bytes)`. Applying it is a single pass; C# writes into the same format through a `HostApi` call or directly into a shared buffer.
- **Batching:** when applying, group consecutive inserts on the same entity, so `spawn().insert(A).insert(B).insert(C)` moves the entity once, not three times.
- **Reserved entities:** `spawn` returns a real `Entity` immediately, usable in other commands in the same buffer, though it has no components until the flush.
- **Hooks and observers fire during apply,** in command order. Their own structural changes go into a fresh buffer that is applied before the sync point ends, with a recursion limit to catch infinite loops.
- **Exclusive systems** (`fn(&mut World)`) may make structural changes directly. The editor and the asset loader use these.

## Systems and scheduling

A frame is a fixed list of stages. Each stage runs its systems, then hits a sync point that applies command buffers and dispatches deferred events. Systems inside a stage are ordered by explicit `before`/`after` constraints, then by registration order.

&#91;embedded content: frame schedule · 6 stages, 5 sync points\]

No system ever sees a structural change mid-iteration; spans and slices stay valid until the next dot.

- **Stages:** `PreUpdate` (input, time) → `FixedUpdate` ×N (physics, fixed-step scripts) → `Update` (gameplay, `Behaviour.Update`) → `LateUpdate` (cameras, `Behaviour.LateUpdate`) → `PostUpdate` (transform propagation) → `Extract` (copy render data out).
- **Access sets:** every system declares the components it reads and writes, from its queries. The v1 executor ignores them and runs systems sequentially, but a debug build checks two systems never alias `&mut` access. That makes going parallel later a scheduler change, not an API change.
- **Script systems** register through the same path. A C# class gets one generated system per magic method it defines (`Update`, `FixedUpdate`, `LateUpdate`), each with a query over its own `ComponentId` and the access it declares via attributes.
- **Run order for behaviours:** by `[ExecutionOrder(n)]`, then by class name, so it is deterministic across runs.
- **Extract is the renderer boundary.** The render world is a separate, plain data snapshot built from changed components. The renderer never queries the main world, so editor, gameplay and rendering can later run on separate threads.

## Events

The event-driven layer has three mechanisms. Each answers a different question, and all three are reachable from Rust and C#.

| Mechanism | Answers | Fires | Example |
| --- | --- | --- | --- |
| Component hooks | "what must always happen when this component appears or disappears?" | during command apply, per entity | register a rigid body with physics on `on_add<RigidBody>` |
| Observers | "who wants to react to this event on this entity?" | during command apply or at a sync point, targeted | `OnCollisionEnter`, `OnClick`, `OnDamaged` |
| Buffered messages | "what happened this frame, for whoever reads it?" | double-buffered, read by systems | `LevelLoaded`, audio cues, analytics |

```rust
// Hooks: one per component type, owned by the component's author
world.register_hooks::<RigidBody>()
    .on_add(|mut w, e| physics_add(&mut w, e))
    .on_remove(|mut w, e| physics_remove(&mut w, e));

// Observers: entity-targeted or global, many per event type
world.observe(player, |ev: Trigger<CollisionEnter>, mut cmd: Commands| { ... });
world.emit(target, CollisionEnter { other, impulse });   // deferred to sync point

// Buffered messages
fn read(mut r: MessageReader<LevelLoaded>) { for m in r.read() { ... } }
```

- **Observers are stored as entities** with an `Observer` component (event ID, optional target filter, callback). Adding or removing them uses the normal ECS, and they appear in the editor's hierarchy for debugging.
- **Propagation (later):** an event can bubble from a child to its parents, which UI picking and damage-on-parent need. Leave a `propagate: bool` field in the event header from the start.
- **C# mapping:** `Behaviour` methods marked `[Observe]` register an observer targeted at the owning entity when the component is added and unregister on removal. Magic methods like `OnCollisionEnter` are found by name, like in Unity, and become `[Observe]` implicitly.

## C# scripting bridge

The engine hosts CoreCLR through `hostfxr`. At startup Rust passes C# a `HostApi` table of `extern "C"` function pointers, and C# passes back a table of `[UnmanagedCallersOnly]` entry points. Every call between the two sides goes through these two tables.

```rust
#[repr(C)]
pub struct HostApi {
    register_component: extern "C" fn(*mut World, *const ComponentDescFfi) -> ComponentId,
    component_id_by_name: extern "C" fn(*const World, *const u8, usize) -> ComponentId,
    query_create: extern "C" fn(*mut World, *const QueryDescFfi) -> QueryId,
    query_next:   extern "C" fn(*mut World, QueryId, *mut Cursor, *mut ChunkFfi) -> bool,
    get_ptr:      extern "C" fn(*mut World, Entity, ComponentId, write: bool) -> *mut u8,
    cmd:          extern "C" fn(*mut World) -> *mut CommandQueue, // current stage's buffer
    emit:         extern "C" fn(*mut World, Entity, EventId, *const u8, usize),
}

#[repr(C)]
pub struct ScriptApi {
    load_assembly: extern "C" fn(*const u8, usize) -> u32,  // returns ALC handle
    unload_assembly: extern "C" fn(u32),
    run_system: extern "C" fn(SystemId, *mut World),
    invoke_observer: extern "C" fn(ObserverId, Entity, *const u8),
    serialize_behaviour: extern "C" fn(GcHandle, *mut ByteSink),
    deserialize_behaviour: extern "C" fn(ComponentId, *const u8, usize) -> GcHandle,
}
```

**Load sequence for a script assembly:**

1. C# loads the assembly into a collectible `AssemblyLoadContext`.
2. It scans the types: every `Behaviour` subclass and every `IComponent` struct calls `register_component` with name, size, alignment and field reflection.
3. For each `Behaviour`, it detects which magic methods exist and registers one generated system per method, plus observers for `[Observe]` methods.
4. Rust components marked `#[script]` are exposed as generated C# structs, produced at engine build time from the same reflection data, so both sides always agree on layout.

**Two C# tiers:**

- `Behaviour` (class): per-entity callbacks, managed fields, `ref T Get<T>()` into native columns. The Unity-like convenience layer.
- `ScriptSystem` with `w.Query<Write<A>, Read<B>>()`: a `ref struct` chunk view handing out `Span<T>` over columns. The fast path for many entities.

**GC safety:** managed objects are referenced from columns only through `GCHandle` (normal, not pinned). Columns never hold raw managed pointers, because CoreCLR's GC moves objects.

## Hot reload and layout migration

A script reload swaps one assembly while the world keeps running. Everything defined by the old assembly is torn down and rebuilt by name, with state carried across through reflection.

1. **Pause** at the end of a frame, after all command buffers are applied.
2. **Snapshot managed state:** for every row in every managed column of the assembly, call `serialize_behaviour` and store the bytes keyed by `(Entity, component name)`.
3. **Snapshot unmanaged layouts:** record each script struct's old field list (name, type, offset).
4. **Unregister** the assembly's systems and observers, free every `GCHandle`, then unload the `AssemblyLoadContext`.
5. **Load** the new assembly and re-register its components by name. A component with the same name keeps its `ComponentId`.
6. **Migrate unmanaged columns** whose layout changed: build a new column per affected archetype, copy each field by name where the type matches, default-initialize new fields, drop removed ones. Archetype identity is unchanged, since the `ComponentId` is the same.
7. **Restore managed state:** `deserialize_behaviour` per snapshot row, writing the new `GCHandle` into the column. Missing fields take defaults; unknown fields are dropped with a warning.
8. **Handle removed types:** a class that no longer exists is removed from every entity through a command buffer, with a warning listing affected entities. Keep its serialized bytes in an undo-able record, so restoring the class restores the data.

**Gotchas to test early:** static fields (reset on reload, like Unity's domain reload), events subscribed by C# delegates (must be torn down in step 4 or the ALC never unloads), and any managed object referenced from Rust outside a column (forbidden by design).

## Editor integration

Reflection is the backbone: the inspector, undo, scene files, copy/paste and hot reload all read and write components through the same field descriptions.

```rust
pub struct TypeReflect {
    name: Box<str>,
    fields: Box<[FieldReflect]>,
}
pub struct FieldReflect {
    name: Box<str>,
    offset: usize,
    ty: FieldType,          // F32, Vec3, Entity, AssetHandle, Enum(ReflectId), Struct(ReflectId), ...
    attrs: FieldAttrs,      // range, tooltip, hidden, read_only
}
```

- **Stable IDs:** every editable entity carries a `SceneId(u128)` component. Scene files, undo records and cross-entity references store `SceneId`, never `Entity`. A map `SceneId → Entity` is rebuilt on load and patched on spawn and despawn.
- **Undo commands** operate on reflected data:
  - `SetField { id: SceneId, component, field_path, old: Bytes, new: Bytes }`
  - `AddComponent` / `RemoveComponent` storing the full serialized component
  - `Spawn` / `Despawn` storing the serialized entity, so undoing a delete restores the same `SceneId`
  - `Group(Vec<Command>)` for multi-step edits
- **Merging:** consecutive `SetField` on the same field within one drag gesture merge into one command, keeping the first `old` and the last `new`.
- **Play mode:** serialize the scene on entering play, and restore it on stop. The undo stack is suspended during play.
- **Inspector refresh** uses change ticks: only components with `changed > last_inspector_tick` re-read their fields.
- **Picking** writes `Entity` bits into an ID buffer during rendering; the editor maps it back to a `SceneId` for selection, which is itself an undo-able command.

## Development roadmap

Build bottom-up, each milestone with a test suite and a benchmark before moving on. The ECS crate has no dependency on the renderer or CoreCLR, so it can be tested in isolation.

1. **BlobVec and Column.** Type-erased storage with drop, clone and swap-remove. Exit: Miri-clean tests, a proptest against `Vec<T>` as the model.
2. **Entities and archetypes.** Allocator with generations, archetype table, edges, entity moves. Exit: random spawn/insert/remove/despawn sequences match a naive `HashMap<Entity, HashMap<ComponentId, Vec<u8>>>` model.
3. **Dynamic queries.** `QueryDesc`, `QueryState`, incremental matching, chunk iteration. Exit: iteration over 1M entities with 2 components runs within \~2× a plain `Vec` loop.
4. **Typed Rust API.** `WorldQuery`, `Query<…>`, `Res`, `Commands`, function systems. Exit: a small sample game (spawning, moving, despawning) written only through the typed API.
5. **Change ticks.** `Added`/`Changed` filters, `Mut<T>`, wraparound handling. Exit: tests that cross the clamp boundary.
6. **Command buffers and sync points.** Encoding, batching, reserved entities. Exit: `spawn().insert(A).insert(B)` performs exactly one archetype move.
7. **Hooks, observers, messages.** Exit: a physics stub that registers bodies through hooks and fires `CollisionEnter` observers.
8. **Reflection and serialization.** Derive macro, scene save/load by name, `SceneId`. Exit: save, reload with components registered in a different order, compare.
9. **CoreCLR bridge.** Host, `HostApi`, registration of C# components, `Behaviour` dispatch, chunk tier. Exit: a C# script moves 10k entities at frame rate through the chunk tier.
10. **Hot reload.** The 8-step sequence above. Exit: change a `Behaviour`'s fields and a struct's layout while running; no state loss, the old ALC verifiably unloads.
11. **Editor hooks.** Undo commands, inspector refresh, play-mode snapshot. Exit: undo of delete restores the same entity and its references.

Useful references while building: [Bevy ECS source](https://github.com/bevyengine/bevy/tree/main/crates/bevy_ecs) for the Rust typing tricks, [flecs](https://github.com/SanderMertens/flecs) for observers and hooks, and Unity's Entities docs for chunk design.

## Open questions

### Hierarchy: components or first-class relations?

| Option | Upsides | Downsides |
| --- | --- | --- |
| `Parent` / `Children` components | Fits the storage as designed; trivial to serialize; easy to debug | Both sides kept consistent by hooks; every other relationship ("owned by", "targets") is a hand-rolled component; relationship queries are manual |
| Relation pairs (flecs-style `(Rel, Target)` IDs) | One mechanism for hierarchy, ownership and attachments; queries like "all children of X" are native | Each distinct target can create its own archetype, fragmenting into many tiny tables; `ComponentId` encoding and cleanup on target despawn get much harder |

**Leaning:** components now, but make `ComponentId` 64-bit internally so pair encoding can be added later without breaking the format.

### Can Rust query C# structs by name?

| Option | Upsides | Downsides |
| --- | --- | --- |
| Full access by name | Native systems (physics, networking) can process script-defined data at full speed | Rust code depends on names a script reload can remove, so failures move to runtime; typed Rust access needs runtime layout checks |
| Reflection-only access | Clear ownership: Rust never hardcodes script types; editor and serialization still work | Generic field access is slow; no fast Rust system can iterate script data |

**Leaning:** reflection-only. If Rust needs fast access to some data, that component should be declared in Rust and exposed to C#.

### Physics: own solver or Rapier?

| Option | Upsides | Downsides |
| --- | --- | --- |
| Wrap Rapier | Mature solver, joints, CCD, good performance; weeks of work instead of months | Keeps its own world, so state is duplicated and synced every frame; you don't control its data layout |
| Own solver on ECS columns | No sync step; data layout and scheduling fully yours; strong learning and research value | Months of work; robust stacking, CCD and joints are hard to get right |

**Leaning:** Rapier behind a `PhysicsBackend` boundary, synced through hooks and change ticks. Replace it later only if the sync cost shows up in profiles.

### One world or several?

| Option | Upsides | Downsides |
| --- | --- | --- |
| Several worlds (editor, play, prefab preview) | Clean isolation: play mode runs on a copy, so stopping just drops it; the undo stack is never touched by gameplay | References across worlds are impossible; memory is duplicated; systems must be told which world they run on |
| One world with scene tags | Cheap; references between scenes just work | Every query must filter by scene; play-mode scripts can accidentally touch editor entities; restoring after play is harder |

**Leaning:** several worlds sharing one component registry, so `ComponentId`s match across them and copying between worlds is a memcpy per column.

### When to go parallel, and may C# run off the main thread?

| Option | Upsides | Downsides |
| --- | --- | --- |
| Parallel from the start | Aliasing bugs and order assumptions surface early | Harder debugging while core features are still moving; slows every milestone |
| Sequential first, parallel later | Simple and fast to build; access sets are already declared, so the switch is a scheduler change | Hidden reliance on run order accumulates and must be found at switch time |
| C# on worker threads | Script systems scale with cores | Unity-style user code assumes one thread; thread-safety bugs land in user scripts |

**Leaning:** sequential through roadmap step 9, then parallel native systems. `Behaviour` code stays on the main thread; only `ScriptSystem`s marked `[Parallel]` with declared access run on workers.
