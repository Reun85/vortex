//! Exercises the typed API exactly as a user would: only public items.

use delta::{
    Added,
    Changed,
    Commands,
    Entity,
    Local,
    Query,
    Res,
    ResMut,
    Schedule,
    Stage,
    With,
    Without,
    World,
};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Position(f32, f32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Velocity(f32, f32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Health(i32);
struct Frozen;
struct Player;

struct Time {
    dt: f32,
}

#[derive(Default)]
struct Stats {
    moved: usize,
    seen_added: usize,
    seen_changed: usize,
}

fn spawn_one(world: &mut World, p: Position, v: Velocity) -> Entity {
    let e = world.spawn();
    world.insert(e, p).unwrap();
    world.insert(e, v).unwrap();
    e
}

#[test]
fn tuple_order_and_mutation() {
    let mut world = World::new();
    world.insert_resource(Time { dt: 0.5 });
    let a = spawn_one(&mut world, Position(0.0, 0.0), Velocity(2.0, 4.0));
    let b = spawn_one(&mut world, Position(1.0, 1.0), Velocity(2.0, 0.0));
    world.insert(b, Frozen).unwrap();

    // Mutable term first, read term second: tuple order must not matter.
    fn movement(
        time: Res<Time>,
        mut q: Query<(&mut Position, &Velocity), Without<Frozen>>,
        mut stats: ResMut<Stats>,
    ) {
        for (mut p, v) in &mut q {
            p.0 += v.0 * time.dt;
            p.1 += v.1 * time.dt;
            stats.moved += 1;
        }
    }
    world.insert_resource(Stats::default());

    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, movement);
    schedule.run(&mut world);
    schedule.run(&mut world);

    assert_eq!(world.get::<Position>(a), Some(&Position(2.0, 4.0)));
    assert_eq!(world.get::<Position>(b), Some(&Position(1.0, 1.0)));
    assert_eq!(world.resource::<Stats>().unwrap().moved, 2);
}

#[test]
fn entity_option_and_single_access() {
    let mut world = World::new();
    let a = spawn_one(&mut world, Position(1.0, 0.0), Velocity(0.0, 0.0));
    let b = world.spawn();
    world.insert(b, Position(2.0, 0.0)).unwrap();
    world.insert_resource(Stats::default());

    fn inspect(
        q: Query<(Entity, &Position, Option<&Velocity>)>,
        mut stats: ResMut<Stats>,
        mut out: Local<Vec<(Entity, bool)>>,
    ) {
        for (e, _, v) in &q {
            out.push((e, v.is_some()));
        }
        stats.moved = out.len();
        let (e, p, _) = q.iter().next().unwrap();
        assert_eq!(q.get(e).map(|(_, p, _)| *p), Some(*p));
        assert!(q.contains(e));
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, inspect);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Stats>().unwrap().moved, 2);
    let _ = (a, b);
}

#[test]
fn query_get_mut_and_missing() {
    let mut world = World::new();
    let a = spawn_one(&mut world, Position(0.0, 0.0), Velocity(0.0, 0.0));
    let b = world.spawn();
    world.insert(b, Velocity(1.0, 1.0)).unwrap();

    #[derive(Default)]
    struct Targets(Vec<Entity>);
    world.insert_resource(Targets(vec![a, b]));

    fn poke(t: Res<Targets>, mut q: Query<&mut Position>) {
        let [a, b] = t.0[..] else { unreachable!() };
        q.get_mut(a).unwrap().0 = 7.0;
        assert!(q.get_mut(b).is_none()); // exists but has no Position
    }
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, poke);
    schedule.run(&mut world);
    assert_eq!(world.get::<Position>(a), Some(&Position(7.0, 0.0)));
}

#[test]
fn change_detection_filters() {
    let mut world = World::new();
    world.insert_resource(Stats::default());
    let a = spawn_one(&mut world, Position(0.0, 0.0), Velocity(0.0, 0.0));

    fn observe(
        added: Query<Entity, Added<Position>>,
        changed: Query<Entity, Changed<Position>>,
        mut stats: ResMut<Stats>,
    ) {
        stats.seen_added += added.iter().count();
        stats.seen_changed += changed.iter().count();
    }
    fn write(mut q: Query<&mut Position, With<Player>>) {
        for mut p in &mut q {
            p.0 += 1.0;
        }
    }

    let mut schedule = Schedule::new();
    schedule.add_system(Stage::PreUpdate, observe);
    schedule.add_system(Stage::Update, write);

    // Frame 1: the entity predates the first run, so it counts as added and
    // changed. `write` matches nothing (no Player).
    schedule.run(&mut world);
    let s = world.resource::<Stats>().unwrap();
    assert_eq!((s.seen_added, s.seen_changed), (1, 1));

    // Frame 2: nothing happened.
    schedule.run(&mut world);
    let s = world.resource::<Stats>().unwrap();
    assert_eq!((s.seen_added, s.seen_changed), (1, 1));

    // Frame 3: make it a player; `write` now changes it every frame, and
    // `observe` (which runs first) sees the change on the next frame.
    world.insert(a, Player).unwrap();
    schedule.run(&mut world);
    schedule.run(&mut world);
    let s = world.resource::<Stats>().unwrap();
    assert_eq!(s.seen_added, 1);
    assert_eq!(s.seen_changed, 2);
}

#[test]
fn commands_are_deferred_to_the_sync_point() {
    let mut world = World::new();
    world.insert_resource(Stats::default());

    fn spawner(mut commands: Commands, mut stats: ResMut<Stats>) {
        let e = commands.spawn().insert(Position(1.0, 1.0)).id();
        commands.entity(e).insert(Velocity(0.0, 1.0));
        stats.moved += 1;
    }
    fn counter(q: Query<&Position>, mut stats: ResMut<Stats>) {
        stats.seen_added = q.iter().count();
    }

    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, spawner);
    schedule.add_system(Stage::Update, counter);
    schedule.run(&mut world);
    // `counter` ran in the same stage as `spawner`, before the sync point.
    assert_eq!(world.resource::<Stats>().unwrap().seen_added, 0);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Stats>().unwrap().seen_added, 1);
    assert_eq!(world.entities().len(), 2);
}

#[test]
#[should_panic(expected = "conflicting access")]
fn conflicting_params_are_rejected() {
    fn bad(_a: Query<&mut Position>, _b: Query<&Position>) {}
    let mut world = World::new();
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, bad);
    schedule.run(&mut world);
}

#[test]
#[should_panic(expected = "resource")]
fn missing_resource_panics_with_its_name() {
    fn needs(_t: Res<Time>) {}
    let mut world = World::new();
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, needs);
    schedule.run(&mut world);
}

#[test]
fn fixed_update_runs_n_times() {
    let mut world = World::new();
    world.insert_resource(Stats::default());
    fn tick(mut stats: ResMut<Stats>) {
        stats.moved += 1;
    }
    let mut schedule = Schedule::new();
    schedule
        .add_system(Stage::FixedUpdate, tick)
        .set_fixed_steps(3);
    schedule.run(&mut world);
    assert_eq!(world.resource::<Stats>().unwrap().moved, 3);
}

/// The roadmap's exit test: a small game (spawning, moving, despawning)
/// written only through the typed API.
#[test]
fn sample_game() {
    struct Lifetime(u32);
    struct Spawner {
        every: u32,
        frame: u32,
    }
    #[derive(Default)]
    struct Score {
        despawned: u32,
    }

    fn spawn_enemies(mut commands: Commands, mut spawner: ResMut<Spawner>) {
        spawner.frame += 1;
        if spawner.frame.is_multiple_of(spawner.every) {
            let x = spawner.frame as f32;
            commands
                .spawn()
                .insert(Position(x, 0.0))
                .insert(Velocity(0.0, 1.0))
                .insert(Lifetime(5));
        }
    }
    fn movement(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
        for (mut p, v) in &mut q {
            p.0 += v.0 * time.dt;
            p.1 += v.1 * time.dt;
        }
    }
    fn expire(
        mut commands: Commands,
        mut q: Query<(Entity, &mut Lifetime)>,
        mut score: ResMut<Score>,
    ) {
        for (e, mut life) in &mut q {
            life.0 -= 1;
            if life.0 == 0 {
                commands.despawn(e);
                score.despawned += 1;
            }
        }
    }
    fn damage_low_health(mut q: Query<&mut Health, Without<Player>>) {
        for mut h in &mut q {
            h.0 -= 1;
        }
    }

    let mut world = World::new();
    world.insert_resource(Time { dt: 1.0 });
    world.insert_resource(Spawner { every: 2, frame: 0 });
    world.insert_resource(Score::default());
    let hero = world.spawn();
    world.insert(hero, Player).unwrap();
    world.insert(hero, Health(100)).unwrap();

    let mut schedule = Schedule::new();
    schedule
        .add_system(Stage::PreUpdate, spawn_enemies)
        .add_system(Stage::Update, movement)
        .add_system(Stage::LateUpdate, expire)
        .add_system(Stage::PostUpdate, damage_low_health);

    for _ in 0..20 {
        schedule.run(&mut world);
    }

    // 10 spawns over 20 frames; each lives 5 frames after its first update
    // (spawned at the sync point after PreUpdate), so the last two or so are
    // still alive.
    let score = world.resource::<Score>().unwrap();
    let alive = world.entities().len() - 1; // minus the hero
    assert_eq!(score.despawned as usize + alive, 10);
    assert!(score.despawned >= 7, "despawned {}", score.despawned);
    assert!((1..=3).contains(&alive), "alive {alive}");
    // The hero (a Player) is untouched by `damage_low_health`.
    assert_eq!(world.get::<Health>(hero), Some(&Health(100)));
}

#[test]
fn mut_marks_changed_only_when_written_through() {
    struct Write(bool);

    fn touch(mut q: Query<&mut Health>, write: Res<Write>) {
        for mut h in &mut q {
            if write.0 {
                h.0 += 1; // DerefMut: marks changed
            } else {
                let _ = h.0; // Deref only: must not
            }
        }
    }
    fn watch(q: Query<Entity, Changed<Health>>, mut stats: ResMut<Stats>) {
        stats.seen_changed += q.iter().count();
    }

    let mut world = World::new();
    world.insert_resource(Stats::default());
    world.insert_resource(Write(false));
    let e = world.spawn();
    world.insert(e, Health(0)).unwrap();

    let mut schedule = Schedule::new();
    schedule.add_system(Stage::PreUpdate, watch);
    schedule.add_system(Stage::Update, touch);

    let seen = |w: &World| w.resource::<Stats>().unwrap().seen_changed;
    schedule.run(&mut world); // watch sees the insert
    assert_eq!(seen(&world), 1);
    schedule.run(&mut world);
    schedule.run(&mut world); // touch only read
    assert_eq!(seen(&world), 1);

    world.resource_mut::<Write>().unwrap().0 = true;
    schedule.run(&mut world); // touch writes...
    assert_eq!(seen(&world), 1);
    schedule.run(&mut world); // ...watch sees it one frame later
    assert_eq!(seen(&world), 2);
    assert_eq!(world.get::<Health>(e), Some(&Health(2)));
}

/// Milestone 6 exit: `spawn().insert(A).insert(B)` performs exactly one
/// archetype move, through the real `Commands` parameter.
#[test]
fn spawn_insert_insert_moves_exactly_once() {
    fn spawner(mut commands: Commands) {
        commands
            .spawn()
            .insert(Position(1.0, 2.0))
            .insert(Velocity(3.0, 4.0))
            .insert(Health(5));
    }
    let mut world = World::new();
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, spawner);

    schedule.run(&mut world);
    assert_eq!(world.archetype_moves(), 1);
    assert_eq!(world.entities().len(), 1);

    // Ten more entities cost exactly ten more moves, not thirty.
    for _ in 0..10 {
        schedule.run(&mut world);
    }
    assert_eq!(world.archetype_moves(), 11);
    assert_eq!(world.entities().len(), 11);
}

#[test]
fn reserved_entity_is_usable_before_the_sync_point() {
    fn build(mut commands: Commands) {
        let parent = commands.spawn().insert(Health(1)).id();
        // The child refers to the parent before either exists.
        commands.spawn().insert(Parent(parent));
        commands.entity(parent).insert(Position(0.0, 0.0));
    }
    #[derive(Clone, Copy)]
    struct Parent(Entity);

    fn check(
        children: Query<&Parent>,
        parents: Query<(&Position, &Health)>,
        mut stats: ResMut<Stats>,
    ) {
        for child in &children {
            // The reserved id the child stored now names a real entity.
            assert!(parents.get(child.0).is_some());
            stats.moved += 1;
        }
    }
    let mut world = World::new();
    world.insert_resource(Stats::default());
    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, build);
    schedule.add_system(Stage::LateUpdate, check);
    schedule.run(&mut world);
    // `check` runs after the Update sync point, so both entities exist and
    // the parent has all of its components.
    assert_eq!(world.entities().len(), 2);
    assert_eq!(world.resource::<Stats>().unwrap().moved, 1);
}
