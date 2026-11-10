# Vortex: status and next steps

Nov 10, 2026

## Where things stand

Both plans that were in `Plan/` are implemented and now live in
`docs/completed-plans/`. Checked on this date: `cargo clippy --workspace
--all-targets` is clean, and the `delta`, `delta_editor`, `vortex_math`,
`vortex_engine` and `delta_script` tests all pass (the C# integration tests
included, `API_VERSION` 6).

| Plan | Result |
| --- | --- |
| ECS Architecture & Development Plan (roadmap 1–11) | Done. Storage, archetypes, dynamic and typed queries, change ticks, command buffers, hooks/observers/messages, reflection and scenes, CoreCLR bridge, hot reload, editor (undo, inspector, play mode). |
| Scripting: Unity-Style Behaviours (phases 0–5) | Done. Acceptance test is `delta_script/tests/player.rs`. Phase 5 shipped `ComponentRef<T>` as a thin wrapper over `Get<T>()` (see `phase5-measurements.md`). |

### What exists

| Crate / folder | What it holds |
| --- | --- |
| `crates/delta` | ECS core, schedule with 6 stages and a fixed-step accumulator, `Time` resource, scenes, inspection. Example `bouncing`, bench `iterate`. |
| `crates/delta_derive` | `#[derive(Reflect)]` |
| `crates/delta_editor` | Undo/redo, selection, picking, play mode, inspector (no UI) |
| `crates/delta_script` | CoreCLR host, FFI, hot reload, lifecycle (`Awake`/`Start`/`OnDestroy`), managed components as editor data |
| `crates/vortex_math` | `Vec2/3/4`, `Quat` storage types |
| `crates/vortex_engine` | `Transform`, `CharacterBody`, `register_script_types`, a stand-in `integrate_bodies` system |
| `crates/xtask` | `gen-csharp` generator for `Engine.g.cs`, with a golden test |
| `scripting/Delta.Scripting` + `samples/*` | C# half and the sample assemblies the tests build |

### Departures from the plans, and gaps

- **Sparse-set storage is declared, not built.** `Storage::SparseSet` exists as
  an enum value; no separate sparse-set storage was found. Everything is table
  storage.
- **No hierarchy.** `Parent` appears only as a test-local component. The plan
  leaned towards `Parent`/`Children` components.
- **No physics.** `integrate_bodies` is a stub with a flat ground at y = 0.
- **No renderer, window or input source.** `Extract` is a stage with nothing to
  extract; `Input` is only filled by tests.
- **Phase 5 caching skipped** on purpose: `Get<T>` costs about 19 ns, chunk
  tier about 1 ns. The merge bar was set after measuring, not before.
- **Execution is sequential.** Access sets are declared for Rust systems;
  script systems declare resource access only. No `[ExecuteAlways]`,
  `[Parallel]` or `[ExecutionOrder]` beyond what `Attributes.cs` has.
- **Housekeeping.** Syncthing had left about 80 `*.sync-conflict-*` files
  (older copies of sources, tests and plans). They broke `cargo test` and
  `dotnet build` (duplicate definitions). They are now in `.sync-conflicts/`,
  untouched, so nothing is lost. Delete that folder once you have looked.
  `Cargo.lock` is also listed as gitignored, yet there are two conflict
  copies of it.

## What to do next

Ordered by how much each unblocks. The first three turn the engine from "a
tested core" into "something you can see".

1. **Window, input and main loop.** `winit` loop that fills `InputState` in
   `PreUpdate` and calls `Schedule::run_frame`. Small, and it makes every later
   step observable. Add the `InputMap` defaults ("Horizontal"/"Vertical").
2. **Renderer on the `Extract` boundary.** `wgpu`, a render world built from
   `Changed<Transform>` plus mesh/material handles, and an ID buffer for
   picking (the editor already maps ids back to `SceneId`). Needs asset handles
   (`AssetHandle<T>`, `StrId`), which the plan mentions but nothing implements.
3. **Hierarchy.** `Parent`/`Children` components kept consistent by hooks,
   `GlobalTransform` propagation in `PostUpdate`, scene format support. Do this
   before the renderer gets many entities. Keep `ComponentId` widening
   (64-bit) in mind only if relations are still wanted.
4. **Physics behind a `PhysicsBackend` boundary.** Rapier, registered through
   hooks and synced by change ticks, replacing `integrate_bodies`. Gives real
   `OnCollisionEnter` observers and a proper `CharacterBody`.
5. **C# observers and events.** The scripting plan listed this as a non-goal.
   `[Observe]` and magic methods (`OnCollisionEnter`, `OnClick`) are the
   Unity-feel feature still missing; it needs step 4 to be worth testing.
6. **Editor UI.** `delta_editor` is logic only. Pick an immediate-mode UI
   (egui) over the inspector schema, hierarchy and undo stack. Decide the open
   question on `[ExecuteAlways]` and edit-mode script execution first.
7. **Several worlds.** Play mode currently snapshots and restores. The plan
   leaned towards separate worlds sharing one component registry; revisit when
   the editor UI exists and prefab preview becomes a need.
8. **Parallel execution.** Native systems first, driven by declared access;
   `[Parallel]` script systems after. Make script access sets non-empty and
   turn the version counters to atomics when this starts.
9. **Smaller items.** Sparse-set storage for `Selected`/`Hovered`-style tags;
   required *managed* components; `ComponentRef<T>` caching if a real profile
   shows `Get<T>` hot; Windows/macOS hosting for the scripting layer; CI that
   runs the golden generator test, clippy and miri.

### Suggested next plan

Write `Plan/Window, Renderer and Hierarchy.md` covering steps 1–3 with an
exit test of its own: a script-driven `PlayerMovement` entity visibly moving
in a window with a child camera, surviving a hot reload. Steps 4–5 follow as a
second plan.
