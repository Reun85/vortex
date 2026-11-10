# Vortex Scripting: Unity-Style Behaviours Plan

Oct 22, 2026 · @reun

Six phases take `Delta.Scripting` from today's minimal `Behaviour` to a script you could write the way you write a Unity `MonoBehaviour`. Phase 0 hardens the bridge first, and each phase ends in an exit test, in the style of the existing roadmap.

## Goal and Acceptance Test

The target is one script: the Unity `PlayerMovement` example, ported with only mechanical renames. It should compile, run, survive a hot reload, and be editable with undo. When that test passes, the plan is done.

```csharp
[Requires(typeof(Transform), typeof(CharacterBody))]
public sealed class PlayerMovement : Behaviour
{
    [SerializeField, Range(0, 20)] float walkSpeed = 5f;   // private, inspector-editable
    [SerializeField] float sprintSpeed = 8f, rotationSpeed = 720f, acceleration = 20f;
    [SerializeField] float jumpHeight = 1.2f, gravity = -20f, groundedStickForce = -2f;
    [SerializeField] Entity cameraEntity;                   // saved in scenes as a SceneId

    ComponentRef<Transform> cameraTransform;                // cached once, validated per use
    Vector3 horizontalVelocity;                             // private runtime state
    float verticalVelocity;

    protected override void Awake() => cameraTransform = World.Ref<Transform>(cameraEntity);

    public override void Update()
    {
        float dt = Time.DeltaTime;                          // engine resource
        Vector2 input = Input.Axis("Horizontal", "Vertical");
        ref var self = ref Get<Transform>();                // Rust-native component
        ref var body = ref Get<CharacterBody>();
        // ... the Unity maths, unchanged ...
        body.Velocity = horizontalVelocity + Vector3.Up * verticalVelocity;
    }
}
```

The test (`delta_script/tests/player.rs`) drives a world with fake input. It must check that:

1. **Lifecycle:** spawning the entity inserts `Transform` and `CharacterBody` if missing, then runs `Awake` once, `Start` once before the first `Update`, and `OnDestroy` once on despawn.
2. **Engine data:** holding "Vertical" for 60 frames at a fixed `DeltaTime` moves the body the expected distance (computed in Rust from the same formula).
3. **Hot reload:** after a reload, private `horizontalVelocity` and `verticalVelocity` keep their values, and the frame after the reload continues smoothly from them.
4. **Editor:** the inspector shows `walkSpeed` with its range. Setting it through `Editor::set_field` changes the speed, and undo restores it. Play mode restores it on stop.
5. **Scenes:** saving and loading the scene keeps `cameraEntity` pointing at the camera's `SceneId`.

Non-goals: physics (a stub system that integrates `CharacterBody.Velocity` stands in for `controller.Move`), C# observers and events, parallel execution, and Windows hosting.

## Order of Work

&#91;embedded content: order of work · 6 phases and the acceptance test\]

After Phase 0, three tracks are independent: lifecycle leading to editor data, engine data leading to engine types, and cached references. Phase 4 depends on Phase 1 because restoring a behaviour from JSON has to decide which lifecycle calls run. Phase 3 depends on Phase 2 because it reuses `check_layout` and generates the `Time`, `Input` and `KeyCode` mirrors. Each phase bumps `API_VERSION` (2 through 7) and lands with its own exit test.

## Phase 0: Bridge Hardening

This phase fixes the soft spots found in the walkthrough before anything new is added on top of them. Every item is small and local, and none changes the script-facing API.

| Change                            | Where                                                              | Detail                                                                                                                                                                                                                                                                                                                                                                                       |
| --------------------------------- | ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Range-check component ids from C# | `bridge.rs`: `cmd_insert`, `cmd_remove`, `get_ptr`, `query_create` | Use `components().get(c)` and return `u32::MAX` or null with `last_error`, so a bad id can never panic inside `extern "C"` and abort the process. Add a `catch_unwind` guard around every host function as a backstop.                                                                                                                                                                       |
| Give queries an owner             | `ScriptState.queries`, `Scripts.Unload`, `Query`                   | Record the owning assembly handle in each `QueryState` slot. `unload_assembly` frees that assembly's queries and puts the ids on a free list. Add `HostApi.query_free` and call it from a finalizer-free `Query.Dispose`. Log a warning when one system creates more than N queries, to catch `Build()` inside `OnRun`.                                                                      |
| Reject kind changes on reload     | `register_component_impl`                                          | If a name exists with a different `Kind` while `reloading`, return an error naming both kinds. `reload` then treats it as "removed, then added": snapshot or remove the old component, register a new id under a new internal name, and restore by class name. Never patch `Kind` in place.                                                                                                  |
| `bool` fields                     | `Components.cs` `Describe`, `ComponentRegistry`                    | The real blocker is that layout comes from `Marshal.SizeOf`/`OffsetOf`, which treat `bool` as a 4-byte Win32 `BOOL`. Switch to the managed layout (`Unsafe.SizeOf<T>()` and offsets measured on a default instance), then map `bool` to `FT_BOOL` at 1 byte. Rust reads must treat any nonzero byte as true, because a raw `bool` outside 0/1 is undefined behaviour. `char` stays rejected. |
| Field attributes from C#          | `FieldDescFfi`, `Components.cs`, `register_component_impl`         | Add `range_lo`, `range_hi`, `flags` (hidden, read\_only, has\_range) and a tooltip string to `FieldDescFfi`, filled from `[Range]`, `[Tooltip]`, `[HideInInspector]` and `[ReadOnly]`. Pass them through into `FieldAttrs`.                                                                                                                                                                  |
| Pin the world                     | `ScriptHost::start` docs, `World.For`                              | C# caches `World` objects by pointer, and Phase 5 caches more. Require the host to keep the `World` at a fixed address (a `Box<World>`), say so in `ScriptHost::start`, and assert in debug that the pointer passed to `run_system` never changes.                                                                                                                                           |
| Fix the `host.rs` comment         | `DotnetHost::function`                                             | Say the delegate loads each assembly into an isolated load context, which is why `ScriptLoadContext` hands back the bridge assembly by identity.                                                                                                                                                                                                                                             |

Bump `API_VERSION` to 2, since `FieldDescFfi` grows and `HostApi` gains `query_free`.

**Exit test:**

- A fuzz test calls each host function with random ids, entity bits and lengths. Nothing aborts, and `last_error` is set.
- Reloading `Reloadable` 50 times leaves `ScriptState.queries` at a constant size.
- A V4 variant that turns `Counter` from a `Behaviour` into a struct reloads with a clear report and no leaked handles (`live_handles` returns to its pre-reload count).

## Phase 1: Lifecycle and Required Components

`Awake`, `Start` and `OnDestroy` are driven by component hooks on the Rust side, so they fire at the same moments as every other component event. They never run in the middle of a system's iteration.

### 1a. Hooks need to know which component fired

Today `Hook = fn(DeferredWorld, Entity)`, so one shared function cannot tell which behaviour class it is running for. Change the signature to Bevy's shape:

```rust
pub struct HookContext { pub entity: Entity, pub component: ComponentId }
pub type Hook = fn(DeferredWorld<'_>, HookContext);
```

This is a breaking change inside `delta` only. The `SceneId` and `Observer` hooks and the tests update mechanically.

### 1b. Lifecycle callbacks into C#

- Add `ScriptApi.behaviour_event(handle: u64, event: u32)`. The events are `AWAKE`, `ON_DESTROY` and `ON_ENABLE`/`ON_DISABLE`, the last two reserved for later.
- At registration, every `ScriptManaged` component gets the same two hooks. `on_add` reads the handle from the column and calls `behaviour_event(h, AWAKE)`. `on_remove` calls `behaviour_event(h, ON_DESTROY)` while the value is still in place, which the hook contract already guarantees. The handle is freed after that, by the drop fn as today.
- **Commands from inside a lifecycle callback**: before calling into C#, set `ScriptState.current_queue` to `&mut world.deferred`, and restore it afterwards. `World.Spawn`/`Insert`/`Despawn` then work in `Awake` and `OnDestroy`, and their effects apply before the sync point ends, under the existing 100-round recursion limit.
- **`Start`** is not a hook. Each `Behaviour` gets an internal `started` flag. A start pass runs at the front of `FixedUpdate`, `Update` and `LateUpdate`: for every class that overrides `Start`, it queries that class with `Added<Class>` and calls `Start` on each instance whose flag is clear. The `Added` filter makes it free in steady state, and the flag stops the three passes from repeating it. That gives Unity's rule, "`Start` runs before the first magic method of that instance", even for behaviours inserted at the `PreUpdate` sync point.
- `C#`: `Behaviour` gains `protected virtual void Awake()`, `Start()` and `OnDestroy()`. `Entity` and `World` are set before `Awake`.

**Hot reload semantics:** restored instances do **not** get `Awake` or `Start` again; they get a new `OnAfterReload()` virtual instead. The `started` flag is saved with the instance. `OnUnload` stays as the "before reload" hook. Despawning during play still calls `OnDestroy`, but `stop_play` despawning the scene does too, and that has to be decided (see Open decisions).

### 1c. Required components

- `[Requires(typeof(A), typeof(B))]` on a `Behaviour` or `IComponent` struct is sent at registration as a list of component ids (`ComponentDescFfi.requires`). `delta` stores it as `ComponentInfo.requires: Box<[ComponentId]>`.
- `insert_many_by_id` extends the inserted set with every missing required component, **zero-initialized from reflection**, inside the same single move, before any hook runs. That matches Bevy's required components. Removing a required component is still allowed; the requirement is about construction, not an invariant.
- Requiring a managed component means constructing a `Behaviour`, which needs a call into C#. Allow requirements on data and tag components only in this phase.

Bump `API_VERSION` to 3.

**Exit test:**

- A `Probe : Behaviour` logs `Awake`/`Start`/`Update`/`OnDestroy` with the frame number. The log matches Unity's order for: spawning in `PreUpdate`, spawning in `Update`, despawning in `LateUpdate`, and adding and then removing the component in the same buffer, where both `Awake` and `OnDestroy` run, in order.
- `Awake` spawning another entity works, and that entity exists by the end of the same sync point.
- `world.Add<PlayerMovement>(e)` on a bare entity gives it zeroed `Transform` and `CharacterBody`, with exactly one archetype move.

## Phase 2: Engine Data in C#

Scripts need `Time` and `Input`. Rather than writing two special cases, this phase adds one mechanism: **a Rust resource that is reflected can be exposed to C# by name**, the same way components are.

### 2a. Exposed resources

- **Rust.** `world.expose_resource::<T: Reflect + Send + Sync>(name)` records `name -> (TypeId, ReflectId)` in a `ScriptResources` table. `T` must be plain reflected data, as components are.
- **FFI.** `HostApi.resource_id_by_name(world, name) -> u32` and `resource_ptr(world, id, write: u8) -> *mut u8`. The pointer is the boxed value's address. It stays valid for the system run, because nothing can insert or remove a resource while a system runs (`Commands::insert_resource` is deferred). C# must not cache it across runs, because `insert_resource` replaces the box.
- **Layout check.** C# declares a mirror (`[EngineResource("vortex::Time")] public struct TimeData { ... }`). At load the bridge sends its field description through a new `HostApi.check_layout(world, name, desc)`, which compares it with the Rust reflection using `types_equal`. A mismatch fails the load with both layouts in the error, so a mismatch can never become silent memory corruption. Phase 3 generates these mirrors, so the check becomes a safety net instead of the main defence.
- **C# API.** `Resource<T>.Read` returns `ref readonly T` and `Resource<T>.Write` returns `ref T`, each one host call. `RunSystem` sets a `World.Current` static (one world per process), so static facades like `Time.DeltaTime` work inside any system or behaviour.
- **Access.** Each script system records the resource ids it touched on its first run as resource reads and writes in its `Access`, the first step toward non-empty script access sets.

### 2b. `Time`

```rust
#[derive(Reflect)] #[repr(C)] #[reflect(name = "vortex::Time")]
pub struct Time { pub delta: f32, pub fixed_delta: f32, pub elapsed: f64, pub frame: u64, pub in_fixed: u8 }
```

- `Schedule::run` gains a `dt` parameter (or reads it from the host loop), and owns the `Time` resource: it sets `delta`, `elapsed` and `frame` at frame start.
- Inside `FixedUpdate` it sets `in_fixed = 1`, and `Time.DeltaTime` in C# returns `fixed_delta` while that is set, which is Unity's rule. The number of `FixedUpdate` runs per frame comes from an accumulator (`floor((acc + dt) / fixed_delta)`, capped), replacing today's manual `set_fixed_steps`.

### 2c. `Input`

```rust
#[derive(Reflect)] #[repr(C)] #[reflect(name = "vortex::Input")]
pub struct InputState {
    pub down: [u64; 4], pub pressed: [u64; 4], pub released: [u64; 4],   // 256 key bits
    pub mouse_delta: [f32; 2], pub axes: [f32; 32],
}
```

- The platform layer (winit, later) fills it in `PreUpdate`. Tests fill it directly.
- Axis *names* ("Horizontal") cannot live in a reflected resource. An `InputMap` resource (Rust-only, it may hold strings) maps names to slot indices, and `HostApi.input_axis_id(world, name)` resolves a name once. The C# facade (`Input.GetKey(KeyCode)`, `GetKeyDown`, `Axis("Horizontal")`) caches name lookups in a dictionary that is cleared on unload.
- `KeyCode` is one enum with the same numbering on both sides, generated in Phase 3.

Bump `API_VERSION` to 4.

**Exit test:**

- With `dt = 1/60` and `fixed_delta = 1/50`, over 60 frames a behaviour sees `DeltaTime == 1/60` in `Update` and `1/50` in `FixedUpdate`, and `FixedUpdate` runs 50 times.
- Faked "Vertical" input reaches `Input.Axis` the same frame. `GetKeyDown` is true for exactly one frame.
- A C# mirror with a field in the wrong order fails `load_assembly` with a message that names the field.

## Phase 3: Rust Components in C#

This is the plan's `#[script]` item. A Rust component marked for scripting gets a generated C# mirror, binds to the same `ComponentId` by name, and has its layout verified at load. After this phase, `Get<Transform>()` in C# returns a `ref` into the *Rust* `Transform` column.

### 3a. Marking and layout rules

- `#[derive(Reflect)] #[reflect(script)]`. The derive emits a compile error unless the struct is `#[repr(C)]`, because C# must reproduce the layout exactly. Offsets are still taken from `offset_of!` and recorded, so the check below compares facts, not assumptions.
- `TypeReflect` gains `script: bool`. Only marked types, and the structs and enums they contain, are generated.
- Field types are limited to what reflection already supports, plus `bool` (Phase 0) and fieldless `#[repr(u8/u16/u32)]` enums, which become C# enums of the same underlying type. A new `FieldType::Enum { repr, variants }` carries the variant names for the inspector.

### 3b. The generator

- A `cargo xtask gen-csharp` binary links the engine crates, calls the engine's one `register_script_types(&mut World)` function, walks the `TypeRegistry` for `script` types and exposed resources (Phase 2), and writes `scripting/Delta.Scripting/Generated/Engine.g.cs`.
- Each type becomes `[StructLayout(LayoutKind.Explicit, Size = N)]` with a `[FieldOffset(k)]` per field, plus `[EngineComponent("vortex::Transform")]` or `[EngineResource(..)]`. Explicit offsets make padding and alignment identical by construction, not by matching the C# compiler's rules.
- `KeyCode` (Phase 2) is generated the same way.
- The file is committed. A test reruns the generator and fails if the output differs, so a Rust layout change without a regenerate fails CI instead of corrupting memory at runtime.

### 3c. Binding at runtime

- `Delta.Scripting` binds engine types at `Bridge.Initialize`, not per script load. For each `[EngineComponent]` type it calls `component_id_by_name` (finally used) and `check_layout`, then sets `ComponentType<T>.Value`. The engine must register its script types before `ScriptHost::start`; binding fails loudly if a name is missing.
- Engine types live in the non-collectible `Delta.Scripting` assembly, so they survive script reloads, and `ScriptLoadContext` already shares that assembly with scripts.
- Writes from C# go through the normal paths (`Get<T>` stamps the row, `chunk.Write<T>` stamps the chunk), so Rust `Changed<Transform>` systems such as render extraction see script edits.

### 3d. Math types

- **Rust storage types** are plain `#[repr(C)]` scalar structs in a small `vortex_math` crate: `Vec2`, `Vec3`, `Vec4`, `Quat` (all `f32`, no SIMD alignment), with `From` conversions to glam for the actual maths. glam's own `Quat` and `Vec3A` are 16-byte aligned under SIMD, so they must not be stored in script-visible components.
- **C# mirrors map them to `System.Numerics`** (`Vector2`, `Vector3`, `Vector4`, `Quaternion`). The layouts are identical (sequential `float`s), and scripts get SIMD-accelerated maths with no conversions. `check_layout` gets an alias table that treats `vortex::Vec3 ↔ System.Numerics.Vector3` as equal, since their field names differ in case.
- `Delta.Scripting` adds the Unity-flavoured helpers the target script uses as static methods: `MoveTowards`, `RotateTowards`, `LookRotation`, `ProjectOnPlane`, `ClampMagnitude`, and a `Mathf`-style class.

Bump `API_VERSION` to 5.

**Exit test:**

- The generator's golden test passes. Reordering a field in Rust without regenerating fails that test, and a hand-edited stale mirror fails `Bridge.Initialize` with the field named.
- A C# chunk-tier system moves 10k entities' Rust `Transform`s, and a Rust `Changed<Transform>` system sees exactly the moved rows.
- `Get<Transform>()` from a `Behaviour` and a Rust `Query<&mut Transform>` alternate on the same entity over 100 frames, and both see each other's writes.

## Phase 4: Behaviours as Editor Data

Today a `Behaviour`'s fields are invisible to scenes, the inspector, undo and play mode, and private fields are lost on reload. This phase makes managed components first-class editor data **without teaching `delta` anything about .NET**, through one extension point on `ComponentInfo`.

### 4a. Two serialization modes in C#

| Mode | Used for | Members included | `Entity` written as |
| --- | --- | --- | --- |
| `Persist` | scenes, inspector, undo, play mode | public fields and properties, plus `[SerializeField]` private fields; minus `[NonSerialized]` | `SceneId` string (via a host lookup) |
| `Snapshot` | hot reload only | **every** instance field, public or private, including auto-property backing fields; minus `[NonSerialized]` and types that can't survive a reload (`ComponentRef<T>`, delegates, pointers) | raw bits (the world survives a reload) |

`Snapshot` fixes the private-state loss directly. The existing `BehaviourSerializer` gains a mode parameter. In `Persist` mode, `Entity` conversion goes through two new host functions, `entity_to_scene_id(world, bits) -> u128` and `scene_id_to_entity(world, lo, hi) -> u64`. They reuse `entity_ref_to_json`'s rules, retired entities included.

### 4b. The extension point in `delta`

```rust
pub struct ExternalSerde {                       // set only for managed components
    pub to_json:   fn(&World, *const u8) -> Value,                         // whole component
    pub from_json: fn(&mut World, ComponentId, &Value) -> Result<OwnedValue, String>,
    pub get_path:  fn(&World, *const u8, &str) -> Result<Value, String>,
    pub set_path:  fn(&mut World, *mut u8, &str, &Value) -> Result<Value, String>, // returns old
    pub schema:    fn(&World, ComponentId) -> Value,                         // member list for UI
}
// ComponentInfo gains: pub external: Option<ExternalSerde>
```

- `delta_script` fills it for every `ScriptManaged` component, with each function forwarding to a new `ScriptApi` slot: `behaviour_to_json(handle, mode)`, `behaviour_from_json(component, json) -> handle`, `behaviour_get_member(handle, path)`, `behaviour_set_member(handle, path, json) -> old` and `behaviour_schema(component)`.
- **`set_path` edits the live object in place.** It never replaces the instance, so other scripts holding a reference to it, and its private runtime state, are unaffected. This is the main reason not to implement field edits as "serialize, patch, deserialize".
- Every reflection-based entry point in `scene.rs` and `inspect.rs` (`entity_value`, `fill_entity`, `component_value`, `field_value`, `set_field_value`, `insert_component_value`, `default_component_value`, `reflected_components`) checks `external` first. Because the editor is built only on those functions, `SetField`, `Add`/`RemoveComponent`, `Spawn`/`Despawn` undo and play mode all start covering behaviours with **no editor changes**.
- `set_path` on a managed component cannot stamp a tick by itself, so `set_field_value` keeps calling `column.set_changed` as it does today.
- The scene format goes to version 2: managed components appear under `"components"` by class name, like any other component. Version 1 files still load.

### 4c. Lifecycle on restore

A behaviour created from JSON (scene load, undo of delete, play-mode stop) is a **new instance with restored state**, not a reload. Whether that runs `Awake` is an open decision. The leaning is: yes for scene load and undo of delete (Unity does the same), followed by a new `OnAfterDeserialize()` virtual.

### 4d. Inspector

- Field metadata for UI comes from `schema`: member names, types, `[Range]`, `[Tooltip]` and `[HideInInspector]`, read from C# attributes. It is the same `FieldAttrs` shape Phase 0 added for structs.
- Managed fields change without touching change ticks, so `Inspector::refresh` cannot rely on `changed` for them. It re-reads managed components of the inspected entity on every refresh, which is one entity and cheap, and reports them as changed only when the JSON differs from the last read.

Bump `API_VERSION` to 6.

**Exit test:**

- Acceptance checks 3–5: private velocity survives a reload; `walkSpeed` is editable with undo and restored by play mode; `cameraEntity` round-trips through a scene as a `SceneId`.
- Undo of deleting an entity with a `PlayerMovement` restores its serialized fields, and another entity's `Entity` field pointing at it is re-pointed.
- `set_field` on a behaviour keeps the same object: a C# test helper checks reference equality across the edit.

## Phase 5: Cached Component References

This gives "resolve once in `Awake`, use every frame" without a native call on the hot path. `ComponentRef<T>` caches the row pointer together with a version number. A single integer comparison proves the pointer still valid; otherwise it resolves again. **Measure before merging**: if `Get<T>()` overhead does not show up in a profile, ship `ComponentRef<T>` as a thin wrapper over `Get<T>()`, keep the API, and skip the caching.

### 5a. Archetype versions

- A cached pointer goes stale in exactly two situations: a row of the archetype *leaves* (swap-remove can move another row into its place, and the entity itself may have moved or died), or a column *reallocates* (growth, or `rebuild_columns` on reload). `Archetype` gains a `version: u32`, bumped:
  - on every row removal (`swap_remove_entity`, used by moves and despawn), and
  - on any push that grows capacity, plus every `rebuild_columns`.
- Pushes that fit in capacity do **not** bump. A spawn-heavy frame invalidates nothing until a table actually grows, so the hit rate is high outside of churn.
- **Stable address for the counter.** C# must read the version without a call, so the counters cannot live inside `Vec<Archetype>` (it reallocates as archetypes are added). Store them in `Archetypes.versions: Vec<Box<[u32; 256]>>`, chunks that never move, indexed by archetype id. Single-threaded today, so plain `u32` is enough; it becomes `AtomicU32` with the parallel executor.

### 5b. The FFI and the C# type

```rust
#[repr(C)] pub struct RefFfi {
    pub value: *mut u8, pub tick: *const TickCell,   // row's changed-tick cell
    pub version_cell: *const u32, pub version: u32,
}
// HostApi.resolve_ref(world, entity, component, *mut RefFfi) -> u8   (0 = dead or missing)
// HostApi.change_tick_ptr(world) -> *const u32                        (needs the pinned world)
```

```csharp
public struct ComponentRef<T> where T : unmanaged, IComponent
{
    Entity entity; RefFfi cache;
    public ref readonly T Read  => ref *(T*)Valid().value;
    public ref T Write { get { var c = Valid(); *c.tick = *ChangeTick; return ref *(T*)c.value; } }
    public bool TryResolve();                     // false once the entity is dead or lacks T
    RefFfi Valid() => *cache.version_cell == cache.version ? cache : Resolve();
}
```

- `Write` stamps the row's tick from the cached world tick pointer, matching what `get_ptr(write = 1)` does today, so change detection is unaffected.
- The tick cell pointer is covered by the same version: `Column.changed` reallocates exactly when the column grows.
- `ComponentRef<T>` is excluded from both serialization modes (Phase 4). After a reload or a scene load the cache is simply empty, and the first access resolves.
- `World.Ref<T>(entity)` creates one, and `Behaviour.Ref<T>()` is shorthand for its own entity.

**Exit test:**

- A proptest runs random spawns, inserts, removes, despawns and growth between frames, and after each frame checks that every live `ComponentRef` reads exactly what `world.get` returns and that refs to dead entities fail `TryResolve`.
- A benchmark of 10k behaviours, each reading and writing `Transform` per `Update`, compares `Get<T>()`, `ComponentRef<T>` in steady state, and the chunk tier. The numbers are recorded in the repo, and decide 5a/5b's fate as described above.

## Open Decisions

| Decision | Options | Leaning |
| --- | --- | --- |
| Do script systems run outside play mode? | (a) never: the editor runs only Rust systems until `enter_play`; (b) per class, opt-in with `[ExecuteAlways]` | (a) now; (b) later. It decides whether `Awake` on undo/scene load can have side effects in edit mode |
| `Awake` when a behaviour is rebuilt from JSON | (a) `Awake` + `OnAfterDeserialize`; (b) `OnAfterDeserialize` only | (a) for scene load and undo of delete, matching Unity; hot reload stays `OnAfterReload` only (Phase 1) |
| `OnDestroy` when play mode stops | (a) yes, every behaviour gets it, as in Unity; (b) no, the play world is just dropped | (a), so cleanup code is exercised the same way every time |
| Math types in C# | (a) map Rust `Vec3`/`Quat` to `System.Numerics`; (b) generate own `Vector3`/`Quaternion` structs | (a): free SIMD and no conversions, at the cost of an alias rule in `check_layout` |
| Generator form | (a) `xtask` + committed file + golden test; (b) `build.rs` emitting into the C# project; (c) runtime emission at `ScriptHost::start` | (a): reviewable diffs, no build-order coupling between cargo and dotnet |
| Required *managed* components (a `Behaviour` requiring another `Behaviour`) | (a) auto-construct through a C# call during insert; (b) refuse at registration | (b) in Phase 1; revisit after Phase 4 gives a JSON path for construction |
| Phase 5 merge bar | pick the speedup over `Get<T>()` that justifies the version machinery | set it before running the benchmark, not after |
| `API_VERSION` per phase | bump at every phase (2 → 7) vs once at the end | per phase: each phase lands and tests on its own, and mismatched builds fail at `Bridge.Initialize` |
