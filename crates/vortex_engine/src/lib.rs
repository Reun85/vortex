//! Engine-level components and the one function that registers everything
//! scripts can see.

pub mod csharp;

use delta::{Reflect, Schedule, Stage, World};
use vortex_math::{Quat, Vec3};

/// Where an entity is.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "vortex::Transform", script)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::default(),
            rotation: Quat::IDENTITY,
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }
}

/// A character controlled by a script: it sets `velocity`, physics moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "vortex::CharacterBody", script)]
pub struct CharacterBody {
    pub velocity: Vec3,
    pub grounded: bool,
}

/// Registers every script-visible type, in a fixed order. Call before
/// `ScriptHost::start`; the C# side binds to these by name and refuses to
/// start if one is missing or laid out differently.
pub fn register_script_types(world: &mut World) {
    world.register_reflected::<Transform>();
    world.register_reflected::<CharacterBody>();
    // Math types scripts can use in their own components, not only inside
    // the engine's.
    let registry = world.reflection_mut();
    vortex_math::Vec2::reflect_id(registry);
    vortex_math::Vec4::reflect_id(registry);
}

/// Stand-in for physics: integrates `CharacterBody.velocity` into
/// `Transform.position` using the frame's `Time`.
pub fn integrate_bodies(
    time: delta::Res<delta::Time>,
    mut bodies: delta::Query<(&mut CharacterBody, &mut Transform)>,
) {
    let dt = if time.in_fixed != 0 {
        time.fixed_delta
    } else {
        time.delta
    };
    for (mut body, mut transform) in &mut bodies {
        transform.position.x += body.velocity.x * dt;
        transform.position.y += body.velocity.y * dt;
        transform.position.z += body.velocity.z * dt;
        // The ground is the plane y = 0.
        body.grounded = transform.position.y <= 0.0;
        if body.grounded {
            transform.position.y = 0.0;
        }
    }
}

/// Adds the engine's own systems to a schedule.
pub fn add_engine_systems(schedule: &mut Schedule) {
    schedule.add_system(Stage::PostUpdate, integrate_bodies);
}
