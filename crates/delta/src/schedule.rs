use crate::{
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

/// Runs systems stage by stage, in registration order within a stage, and
/// applies their deferred commands at each stage's sync point.
pub struct Schedule {
    stages: [Vec<Box<dyn System>>; 6],
    fixed_steps: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            stages: Default::default(),
            fixed_steps: 1,
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

    /// How many times `FixedUpdate` runs per [`run`](Self::run).
    pub fn set_fixed_steps(&mut self, steps: u32) -> &mut Self {
        self.fixed_steps = steps;
        self
    }

    /// Runs one frame.
    pub fn run(&mut self, world: &mut World) {
        for stage in Stage::ALL {
            let times = if stage == Stage::FixedUpdate {
                self.fixed_steps
            } else {
                1
            };
            for _ in 0..times {
                self.run_stage(stage, world);
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
