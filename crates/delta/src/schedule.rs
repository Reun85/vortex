use crate::{
    engine::Time,
    system::{IntoSystem, System},
    world::World,
};

/// The fixed list of frame stages. Each ends with a sync point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    /// Input, time.
    PreUpdate,
    /// Physics and fixed-step scripts; may run several times per frame.
    FixedUpdate,
    /// Gameplay.
    Update,
    /// Cameras and anything that follows gameplay.
    LateUpdate,
    /// Transform propagation.
    PostUpdate,
    /// Copies render data out of the world.
    Extract,
}

impl Stage {
    pub const ALL: [Stage; 6] = [
        Stage::PreUpdate,
        Stage::FixedUpdate,
        Stage::Update,
        Stage::LateUpdate,
        Stage::PostUpdate,
        Stage::Extract,
    ];
}

fn set_in_fixed(world: &mut World, value: u8) {
    if let Some(time) = world.resource_mut::<Time>() {
        time.in_fixed = value;
    }
}

/// Runs systems stage by stage, in registration order within a stage, and
/// applies their deferred commands at each stage's sync point.
pub struct Schedule {
    stages: [Vec<Box<dyn System>>; 6],
    /// Manual override of the number of `FixedUpdate` runs per frame.
    fixed_steps: Option<u32>,
    fixed_delta: f32,
    accumulator: f64,
}

/// Seconds per `FixedUpdate` step unless changed.
pub const DEFAULT_FIXED_DELTA: f32 = 1.0 / 50.0;

/// The most `FixedUpdate` steps one frame may owe, so that a long frame
/// cannot make the next ones ever longer.
pub const MAX_FIXED_STEPS: u32 = 8;

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            stages: Default::default(),
            fixed_steps: None,
            fixed_delta: DEFAULT_FIXED_DELTA,
            accumulator: 0.0,
        }
    }

    pub fn add_system<M>(
        &mut self,
        stage: Stage,
        system: impl IntoSystem<M> + 'static,
    ) -> &mut Self {
        self.stages[stage as usize].push(Box::new(system.into_system()));
        self
    }

    /// Adds a system that is not a plain function, such as one backed by a
    /// scripting runtime.
    pub fn add_boxed_system(
        &mut self,
        stage: Stage,
        system: Box<dyn System>,
    ) -> &mut Self {
        self.stages[stage as usize].push(system);
        self
    }

    /// Removes every system for which `remove` returns true, in all stages.
    /// Returns how many were removed.
    pub fn remove_systems(
        &mut self,
        mut remove: impl FnMut(&dyn System) -> bool,
    ) -> usize {
        let mut removed = 0;
        for stage in &mut self.stages {
            let before = stage.len();
            stage.retain(|s| !remove(s.as_ref()));
            removed += before - stage.len();
        }
        removed
    }

    /// Forces the number of `FixedUpdate` runs per frame, bypassing the
    /// time accumulator. For tests and tools that step by hand.
    pub fn set_fixed_steps(&mut self, steps: u32) -> &mut Self {
        self.fixed_steps = Some(steps);
        self
    }

    /// Sets the seconds per `FixedUpdate` step and returns to deriving the
    /// step count from frame time.
    ///
    /// # Panics
    /// If `seconds` is not positive and finite.
    pub fn set_fixed_delta(&mut self, seconds: f32) -> &mut Self {
        assert!(
            seconds.is_finite() && seconds > 0.0,
            "fixed delta must be positive"
        );
        self.fixed_delta = seconds;
        self.fixed_steps = None;
        self
    }

    /// Runs one frame of the same length as the last one (zero before any
    /// [`run_frame`](Self::run_frame)): `FixedUpdate` runs once unless
    /// [`set_fixed_steps`](Self::set_fixed_steps) says otherwise.
    pub fn run(&mut self, world: &mut World) {
        let dt = world.resource::<Time>().map_or(0.0, |t| t.delta);
        let steps = self.fixed_steps.unwrap_or(1);
        self.run_with(world, dt, steps);
    }

    /// Runs one frame that took `dt` seconds. This advances [`Time`]: it
    /// sets `delta`, `fixed_delta`, `elapsed` and `frame` first, and
    /// `FixedUpdate` runs `floor((accumulated + dt) / fixed_delta)` times,
    /// at most [`MAX_FIXED_STEPS`], with `in_fixed` set around each run.
    pub fn run_frame(&mut self, world: &mut World, dt: f32) {
        let steps = match self.fixed_steps {
            Some(n) => n,
            None => {
                // f64: a minute of f32 frame times must not lose a step.
                let step = f64::from(self.fixed_delta);
                self.accumulator += f64::from(dt);
                let n = ((self.accumulator / step).floor() as u32)
                    .min(MAX_FIXED_STEPS);
                self.accumulator =
                    (self.accumulator - f64::from(n) * step).max(0.0);
                if n == MAX_FIXED_STEPS {
                    // Drop the debt rather than spiral.
                    self.accumulator = self.accumulator.min(step);
                }
                n
            }
        };
        self.run_with(world, dt, steps);
    }

    fn run_with(&mut self, world: &mut World, dt: f32, steps: u32) {
        if let Some(time) = world.resource_mut::<Time>() {
            time.delta = dt;
            time.fixed_delta = self.fixed_delta;
            time.elapsed += f64::from(dt);
            time.frame += 1;
        }
        for stage in Stage::ALL {
            if stage != Stage::FixedUpdate {
                self.run_stage(stage, world);
                continue;
            }
            for _ in 0..steps {
                set_in_fixed(world, 1);
                self.run_stage(stage, world);
                set_in_fixed(world, 0);
            }
        }
        world.update_messages();
        if let Some(now) = world.check_change_ticks() {
            for system in self.stages.iter_mut().flatten() {
                system.check_change_tick(now);
            }
        }
    }

    /// Runs a single stage followed by its sync point.
    pub fn run_stage(&mut self, stage: Stage, world: &mut World) {
        let systems = &mut self.stages[stage as usize];
        for system in systems.iter_mut() {
            system.initialize(world);
            system.run(world);
        }
        for system in systems.iter_mut() {
            system.apply_deferred(world);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::InputState,
        tick::{CHECK_TICK_THRESHOLD, MAX_CHANGE_AGE, Tick},
    };

    struct A;

    #[test]
    fn frame_clamps_world_ticks_when_due() {
        fn noop() {}
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, A).unwrap();
        let mut schedule = Schedule::new();
        schedule.add_system(Stage::Update, noop);

        // Jump far enough that the write at tick 1 is stale and a check is due.
        let far = 1 + MAX_CHANGE_AGE.max(CHECK_TICK_THRESHOLD) + 10;
        world.set_change_tick(Tick::new(far));
        schedule.run(&mut world);

        let now = world.change_tick();
        let c = world.components().id_of::<A>().unwrap();
        let loc = world.location(e).unwrap();
        let col = world.archetypes().get(loc.archetype).column(c).unwrap();
        assert_eq!(now.relative_to(col.changed(loc.row)).get(), MAX_CHANGE_AGE);
    }

    #[derive(Default)]
    struct Runs {
        fixed: u32,
        updates: u32,
        fixed_dt: Vec<f32>,
    }

    #[test]
    fn time_and_the_fixed_step_accumulator() {
        use crate::system::{Res, ResMut};

        fn fixed(mut runs: ResMut<Runs>, time: Res<Time>) {
            runs.fixed += 1;
            assert_eq!(time.in_fixed, 1);
            runs.fixed_dt.push(time.fixed_delta);
        }
        fn update(mut runs: ResMut<Runs>, time: Res<Time>) {
            runs.updates += 1;
            assert_eq!(time.in_fixed, 0);
            assert!(time.delta > 0.0);
        }
        let mut world = World::new();
        world.insert_resource(Runs::default());
        let mut schedule = Schedule::new();
        schedule.set_fixed_delta(1.0 / 50.0);
        schedule.add_system(Stage::FixedUpdate, fixed);
        schedule.add_system(Stage::Update, update);

        for _ in 0..60 {
            schedule.run_frame(&mut world, 1.0 / 60.0);
        }
        let runs = world.resource::<Runs>().unwrap();
        // One simulated second at 50 Hz, whatever the frame rate.
        assert!((49..=50).contains(&runs.fixed), "{}", runs.fixed);
        assert_eq!(runs.updates, 60);
        assert!(runs.fixed_dt.iter().all(|&d| d == 1.0 / 50.0));
        let time = world.resource::<Time>().unwrap();
        assert_eq!(time.frame, 60);
        assert!((time.elapsed - 1.0).abs() < 1e-4);
        assert_eq!(time.in_fixed, 0);

        // A hitch cannot make the schedule owe unbounded steps.
        let before = world.resource::<Runs>().unwrap().fixed;
        schedule.run_frame(&mut world, 10.0);
        let ran = world.resource::<Runs>().unwrap().fixed - before;
        assert_eq!(ran, MAX_FIXED_STEPS);
    }

    #[test]
    fn input_edges_last_one_frame() {
        let mut input = InputState::default();
        input.press(70);
        assert!(input.is_down(70));
        assert_eq!(input.pressed[1], 1 << 6);
        input.begin_frame();
        input.press(70); // held: not a new press
        assert_eq!(input.pressed, [0; 4]);
        input.release(70);
        assert_eq!(input.released[1], 1 << 6);
        assert!(!input.is_down(70));
    }
}
