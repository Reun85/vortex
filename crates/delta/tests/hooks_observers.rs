//! Hooks, observers and messages, exercised through the public API only.

use std::collections::{BTreeMap, HashSet};

use delta::{
    Commands,
    DeferredWorld,
    Entity,
    Event,
    Local,
    MessageReader,
    MessageWriter,
    Query,
    Res,
    ResMut,
    Schedule,
    Stage,
    Trigger,
    With,
    World,
};

#[derive(Default)]
struct Log(Vec<String>);

fn log(w: &mut DeferredWorld<'_>, line: String) {
    w.resource_mut::<Log>().unwrap().0.push(line);
}

fn lines(world: &World) -> Vec<String> {
    world.resource::<Log>().unwrap().0.clone()
}

fn clear(world: &mut World) {
    world.resource_mut::<Log>().unwrap().0.clear();
}

// --- hooks -----------------------------------------------------------------

struct Tracked(u32);
struct Companion;

fn tracked_world() -> World {
    let mut world = World::new();
    world.insert_resource(Log::default());
    world
        .register_hooks::<Tracked>()
        .on_add(|mut w, e| {
            let v = w.get::<Tracked>(e).unwrap().0;
            log(&mut w, format!("add {v}"));
        })
        .on_insert(|mut w, e| {
            let v = w.get::<Tracked>(e).unwrap().0;
            log(&mut w, format!("insert {v}"));
        })
        .on_replace(|mut w, e| {
            let v = w.get::<Tracked>(e).unwrap().0;
            log(&mut w, format!("replace {v}"));
        })
        .on_remove(|mut w, e| {
            let v = w.get::<Tracked>(e).unwrap().0;
            log(&mut w, format!("remove {v}"));
        });
    world
}

#[test]
fn hook_order_and_values() {
    let mut world = tracked_world();
    let e = world.spawn();

    world.insert(e, Tracked(1)).unwrap();
    assert_eq!(lines(&world), ["add 1", "insert 1"]);

    // Overwrite: the old value is visible to on_replace, the new to on_insert.
    clear(&mut world);
    world.insert(e, Tracked(2)).unwrap();
    assert_eq!(lines(&world), ["replace 1", "insert 2"]);

    clear(&mut world);
    world.remove::<Tracked>(e).unwrap();
    assert_eq!(lines(&world), ["replace 2", "remove 2"]);

    // Removing what is not there fires nothing.
    clear(&mut world);
    assert_eq!(world.remove::<Tracked>(e), Ok(false));
    assert!(lines(&world).is_empty());

    // Despawning counts as removal, with the value still readable.
    world.insert(e, Tracked(3)).unwrap();
    clear(&mut world);
    world.despawn(e).unwrap();
    assert_eq!(lines(&world), ["replace 3", "remove 3"]);
}

#[test]
fn hooks_do_not_fire_for_other_components() {
    let mut world = tracked_world();
    let e = world.spawn();
    world.insert(e, Companion).unwrap();
    world.despawn(e).unwrap();
    assert!(lines(&world).is_empty());
}

#[test]
fn batched_inserts_are_all_in_place_before_on_add() {
    struct Needs;
    let mut world = World::new();
    world.insert_resource(Log::default());
    world.register_hooks::<Needs>().on_add(|mut w, e| {
        let companion = w.has::<Companion>(e);
        log(&mut w, format!("needs sees companion: {companion}"));
    });

    fn spawner(mut commands: Commands) {
        // Needs first, so a one-by-one insert would run its hook too early.
        commands.spawn().insert(Needs).insert(Companion);
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, spawner);
    schedule.run(&mut world);
    assert_eq!(lines(&world), ["needs sees companion: true"]);
    assert_eq!(world.archetype_moves(), 1);
}

#[test]
#[should_panic(expected = "already registered")]
fn registering_a_hook_twice_panics() {
    let mut world = World::new();
    world.register_hooks::<Tracked>().on_add(|_, _| {});
    world.register_hooks::<Tracked>().on_add(|_, _| {});
}

struct Child;

#[test]
fn hook_commands_run_before_the_operation_returns() {
    let mut world = World::new();
    world.register_hooks::<Companion>().on_add(|mut w, _| {
        w.commands().spawn().insert(Child);
    });
    let e = world.spawn();
    world.insert(e, Companion).unwrap();
    // The child spawned by the hook exists once `insert` has returned.
    assert_eq!(world.entities().len(), 2);
}

#[test]
fn hook_commands_run_before_the_sync_point_ends() {
    let mut world = World::new();
    world.register_hooks::<Companion>().on_add(|mut w, _| {
        w.commands().spawn().insert(Child);
    });
    world.insert_resource(Log::default());

    fn make(mut commands: Commands) {
        commands.spawn().insert(Companion);
    }
    fn count(q: Query<Entity, With<Child>>, mut stats: ResMut<Log>) {
        stats.0.push(format!("children {}", q.iter().count()));
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, make);
    schedule.add_system(Stage::LateUpdate, count);
    schedule.run(&mut world);
    assert_eq!(lines(&world), ["children 1"]);
}

#[test]
#[should_panic(expected = "recursion limit")]
fn runaway_hooks_hit_the_recursion_limit() {
    struct Loop;
    let mut world = World::new();
    world.register_hooks::<Loop>().on_add(|mut w, _| {
        w.commands().spawn().insert(Loop);
    });
    let e = world.spawn();
    world.insert(e, Loop).unwrap();
}

// --- observers -------------------------------------------------------------

struct Ping(u32);
impl Event for Ping {}
struct Pong;
impl Event for Pong {}

#[test]
fn targeted_and_global_observers() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let (a, b) = (world.spawn(), world.spawn());

    world.observe(a, |ev: Trigger<Ping>, mut log: ResMut<Log>| {
        log.0.push(format!("a sees {}", ev.0));
    });
    world.observe(b, |ev: Trigger<Ping>, mut log: ResMut<Log>| {
        log.0.push(format!("b sees {}", ev.0));
    });
    world.observe_global(|ev: Trigger<Ping>, mut log: ResMut<Log>| {
        log.0.push(format!(
            "global sees {} at {:?}",
            ev.0,
            ev.target().is_some()
        ));
    });
    world.observe_global(|_: Trigger<Pong>, mut log: ResMut<Log>| {
        log.0.push("pong".into());
    });

    // Outside a system, emit delivers before returning. The observer of the
    // target runs first, then global ones; other targets and events don't.
    world.emit(a, Ping(1));
    assert_eq!(lines(&world), ["a sees 1", "global sees 1 at true"]);

    clear(&mut world);
    world.emit_global(Ping(2));
    assert_eq!(lines(&world), ["global sees 2 at false"]);

    clear(&mut world);
    world.emit_global(Pong);
    assert_eq!(lines(&world), ["pong"]);
}

#[test]
fn emitting_from_a_system_waits_for_the_sync_point() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let target = world.spawn();
    world.observe(target, |_: Trigger<Ping>, mut log: ResMut<Log>| {
        log.0.push("observer".into());
    });

    struct Target(Entity);
    world.insert_resource(Target(target));

    fn emitter(t: Res<Target>, mut commands: Commands, mut log: ResMut<Log>) {
        commands.emit(t.0, Ping(1));
        log.0.push("emitted".into());
    }
    fn later(mut log: ResMut<Log>) {
        log.0.push("later system".into());
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, emitter);
    schedule.add_system(Stage::Update, later);
    schedule.run(&mut world);
    // Both systems finish before the observer runs at the stage's sync point.
    assert_eq!(lines(&world), ["emitted", "later system", "observer"]);
}

#[test]
fn observers_never_nest() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let e = world.spawn();
    world.observe(
        e,
        |_: Trigger<Ping>, mut log: ResMut<Log>, mut c: Commands| {
            log.0.push("ping start".into());
            c.emit_global(Pong);
            log.0.push("ping end".into());
        },
    );
    world.observe_global(|_: Trigger<Pong>, mut log: ResMut<Log>| {
        log.0.push("pong".into());
    });
    world.emit(e, Ping(0));
    // The pong waits until the ping observer is completely done.
    assert_eq!(lines(&world), ["ping start", "ping end", "pong"]);
}

#[test]
fn observer_commands_apply_and_can_despawn_the_target() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let victim = world.spawn();
    let observer =
        world.observe(victim, |ev: Trigger<Ping>, mut c: Commands| {
            c.despawn(ev.target().unwrap());
        });
    assert!(world.is_alive(observer));
    world.emit(victim, Ping(0));
    assert!(!world.is_alive(victim));
    // The observer watching a despawned target goes away with it.
    assert!(!world.is_alive(observer));
}

#[test]
fn despawning_the_observer_entity_stops_it() {
    let mut world = World::new();
    world.insert_resource(Log::default());
    let observer =
        world.observe_global(|_: Trigger<Pong>, mut log: ResMut<Log>| {
            log.0.push("pong".into());
        });
    world.emit_global(Pong);
    world.despawn(observer).unwrap();
    world.emit_global(Pong);
    assert_eq!(lines(&world), ["pong"]);
}

#[test]
fn event_names_are_registered_for_scripts() {
    let mut world = World::new();
    let id = world.event_id::<Ping>();
    let name = world.events().name(id).to_owned();
    assert_eq!(world.events().id_by_name(&name), Some(id));
}

// --- messages --------------------------------------------------------------

struct LevelLoaded(u32);

#[test]
fn messages_reach_readers_before_and_after_the_writer() {
    #[derive(Default)]
    struct Seen {
        early: Vec<u32>,
        late: Vec<u32>,
    }
    fn early(mut r: MessageReader<LevelLoaded>, mut seen: ResMut<Seen>) {
        seen.early.extend(r.read().map(|m| m.0));
    }
    fn write(mut w: MessageWriter<LevelLoaded>, mut frame: Local<u32>) {
        *frame += 1;
        if *frame == 1 {
            w.send(LevelLoaded(7));
        }
    }
    fn late(mut r: MessageReader<LevelLoaded>, mut seen: ResMut<Seen>) {
        seen.late.extend(r.read().map(|m| m.0));
    }

    let mut world = World::new();
    world.insert_resource(Seen::default());
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::PreUpdate, early);
    schedule.add_system(Stage::Update, write);
    schedule.add_system(Stage::LateUpdate, late);
    for _ in 0..4 {
        schedule.run(&mut world);
    }
    let seen = world.resource::<Seen>().unwrap();
    // The reader that ran before the writer still gets it, next frame, and
    // every reader sees it exactly once.
    assert_eq!(seen.early, [7]);
    assert_eq!(seen.late, [7]);
    // Two frame boundaries later it is gone.
    assert!(
        world
            .resource::<delta::Messages<LevelLoaded>>()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_reader_added_late_does_not_see_expired_messages() {
    let mut world = World::new();
    world.send_message(LevelLoaded(1));
    world.update_messages();
    world.update_messages();
    world.send_message(LevelLoaded(2));

    #[derive(Default)]
    struct Seen(Vec<u32>);
    fn read(mut r: MessageReader<LevelLoaded>, mut s: ResMut<Seen>) {
        s.0.extend(r.read().map(|m| m.0));
    }
    world.insert_resource(Seen::default());
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, read);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Seen>().unwrap().0, [2]);
}

// --- the roadmap exit test: a physics stub ---------------------------------

#[test]
fn physics_stub_with_hooks_and_collision_observers() {
    struct RigidBody {
        mass: f32,
    }
    struct Pos(f32);
    struct Health(i32);
    struct Enemy;

    #[derive(Default)]
    struct PhysicsBackend {
        bodies: BTreeMap<Entity, f32>,
        removed: u32,
    }
    struct CollisionEnter {
        other: Entity,
        impulse: f32,
    }
    impl Event for CollisionEnter {}

    #[derive(Default)]
    struct Stats {
        collisions: u32,
        hit_by: Vec<Entity>,
    }

    let mut world = World::new();
    world.insert_resource(PhysicsBackend::default());
    world.insert_resource(Stats::default());

    // Hooks keep the backend in sync with the components, whoever spawns or
    // despawns the body and however they do it.
    world
        .register_hooks::<RigidBody>()
        .on_add(|mut w, e| {
            let mass = w.get::<RigidBody>(e).unwrap().mass;
            w.resource_mut::<PhysicsBackend>()
                .unwrap()
                .bodies
                .insert(e, mass);
        })
        .on_remove(|mut w, e| {
            let backend = w.resource_mut::<PhysicsBackend>().unwrap();
            backend.bodies.remove(&e);
            backend.removed += 1;
        });

    let player = world.spawn();
    world.insert(player, RigidBody { mass: 10.0 }).unwrap();
    world.insert(player, Pos(0.0)).unwrap();
    world.insert(player, Health(100)).unwrap();
    assert_eq!(world.resource::<PhysicsBackend>().unwrap().bodies.len(), 1);

    // The player reacts to being hit; a global observer keeps statistics.
    world.observe(
        player,
        |ev: Trigger<CollisionEnter>, mut health: Query<&mut Health>| {
            let target = ev.target().unwrap();
            health.get_mut(target).unwrap().0 -= ev.impulse as i32;
        },
    );
    world.observe_global(
        |ev: Trigger<CollisionEnter>, mut stats: ResMut<Stats>| {
            stats.collisions += 1;
            stats.hit_by.push(ev.other);
        },
    );

    // Spawn enemies through commands: their hooks fire at the sync point.
    fn spawn_enemies(mut commands: Commands, mut frame: Local<u32>) {
        *frame += 1;
        if *frame == 1 {
            for x in [4.0, 8.0] {
                commands
                    .spawn()
                    .insert(RigidBody { mass: 1.0 })
                    .insert(Pos(x))
                    .insert(Enemy);
            }
        }
    }
    // Enemies walk towards the player.
    fn walk(mut q: Query<&mut Pos, With<Enemy>>) {
        for mut p in &mut q {
            p.0 -= 1.0;
        }
    }
    // Contact detection: emits CollisionEnter once when a pair starts
    // touching, not on every frame it stays in contact.
    fn physics_step(
        bodies: Query<(Entity, &Pos, &RigidBody)>,
        mut touching: Local<HashSet<(Entity, Entity)>>,
        mut commands: Commands,
    ) {
        let all: Vec<_> = bodies.iter().collect();
        for (i, (a, pa, ma)) in all.iter().enumerate() {
            for (b, pb, mb) in &all[i + 1..] {
                let pair = (*a.min(b), *a.max(b));
                if (pa.0 - pb.0).abs() < 1.0 {
                    if touching.insert(pair) {
                        let impulse = ma.mass.min(mb.mass) * 5.0;
                        commands
                            .emit(*a, CollisionEnter { other: *b, impulse });
                        commands
                            .emit(*b, CollisionEnter { other: *a, impulse });
                    }
                } else {
                    touching.remove(&pair);
                }
            }
        }
    }
    // Enemies that touched the player are removed.
    fn reap(
        mut commands: Commands,
        stats: Res<Stats>,
        enemies: Query<Entity, With<Enemy>>,
    ) {
        for e in &enemies {
            if stats.hit_by.contains(&e) {
                commands.despawn(e);
            }
        }
    }

    let mut schedule = Schedule::new();
    schedule
        .add_system(Stage::PreUpdate, spawn_enemies)
        .add_system(Stage::Update, walk)
        .add_system(Stage::FixedUpdate, physics_step)
        .add_system(Stage::LateUpdate, reap);

    schedule.run(&mut world);
    // Hooks registered both enemies as soon as the spawn commands applied.
    assert_eq!(world.resource::<PhysicsBackend>().unwrap().bodies.len(), 3);

    for _ in 0..12 {
        schedule.run(&mut world);
    }

    let stats = world.resource::<Stats>().unwrap();
    // Two enemies each touched the player once: the player sees two hits of
    // 5 damage and the global observer one event per touched entity (the
    // player's and the enemy's side).
    assert_eq!(world.get::<Health>(player).unwrap().0, 100 - 2 * 5);
    assert_eq!(stats.collisions, 4);
    // Both enemies were despawned; the hook took them out of the backend.
    let backend = world.resource::<PhysicsBackend>().unwrap();
    assert_eq!(backend.bodies.len(), 1);
    assert!(backend.bodies.contains_key(&player));
    assert_eq!(backend.removed, 2);
    // What remains: the player and the two observer entities.
    assert_eq!(world.entities().len(), 3);
}
