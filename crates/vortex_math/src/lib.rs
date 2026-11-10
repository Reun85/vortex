//! Plain `#[repr(C)]` scalar math types for components that scripts see.
//!
//! These are storage types: no SIMD alignment, so their layout is exactly
//! what C# (`System.Numerics`) has. Convert to [`glam`] for the maths.

use delta::Reflect;

macro_rules! vector {
    ($name:ident, $glam:ty, $reflect:literal, [$($f:ident),+]) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
        #[repr(C)]
        #[reflect(name = $reflect, script)]
        pub struct $name {
            $(pub $f: f32),+
        }

        impl $name {
            pub const fn new($($f: f32),+) -> Self {
                Self { $($f),+ }
            }
        }

        impl From<$glam> for $name {
            fn from(v: $glam) -> Self {
                Self { $($f: v.$f),+ }
            }
        }

        impl From<$name> for $glam {
            fn from(v: $name) -> Self {
                <$glam>::new($(v.$f),+)
            }
        }
    };
}

vector!(Vec2, glam::Vec2, "vortex::Vec2", [x, y]);
vector!(Vec3, glam::Vec3, "vortex::Vec3", [x, y, z]);
vector!(Vec4, glam::Vec4, "vortex::Vec4", [x, y, z, w]);

/// A rotation. Unlike `glam::Quat` this is not 16-byte aligned.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "vortex::Quat", script)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl From<glam::Quat> for Quat {
    fn from(q: glam::Quat) -> Self {
        Self {
            x: q.x,
            y: q.y,
            z: q.z,
            w: q.w,
        }
    }
}

impl From<Quat> for glam::Quat {
    fn from(q: Quat) -> Self {
        glam::Quat::from_xyzw(q.x, q.y, q.z, q.w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_plain_scalars() {
        assert_eq!((size_of::<Vec3>(), align_of::<Vec3>()), (12, 4));
        assert_eq!((size_of::<Quat>(), align_of::<Quat>()), (16, 4));
    }

    #[test]
    fn glam_round_trips() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(Vec3::from(glam::Vec3::from(v)), v);
        let q = Quat::from(glam::Quat::from_rotation_y(0.5));
        assert_eq!(Quat::from(glam::Quat::from(q)), q);
    }
}
