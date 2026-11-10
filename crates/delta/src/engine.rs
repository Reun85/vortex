//! Engine data that scripts read: time, input, and the mechanism that
//! exposes any reflected resource to them by name.

use std::{any::TypeId, collections::HashMap, mem};

use delta_derive::Reflect;

use crate::{
    reflect::{Reflect as ReflectTrait, ReflectId},
    world::World,
};

/// A resource that is exposed to scripts, as plain data.
#[derive(Clone, Debug)]
pub(crate) struct ExposedResource {
    pub name: Box<str>,
    pub type_id: TypeId,
    pub reflect: ReflectId,
}

impl World {
    /// Makes the reflected resource `T` available to scripts as `name`,
    /// which is how `Time` and `Input` reach C#. `T` is plain data like a
    /// component. Returns the id scripts use (stable only within a run).
    ///
    /// The resource itself is inserted separately; scripts see a missing
    /// resource as an error when they ask for its address.
    ///
    /// # Panics
    /// If `T` has drop glue, or `name` is already exposed for another type.
    pub fn expose_resource<T: ReflectTrait + Send + Sync>(
        &mut self,
        name: &str,
    ) -> u32 {
        assert!(
            !mem::needs_drop::<T>(),
            "an exposed resource must be plain data (no drop glue): {name}"
        );
        let reflect = T::reflect_id(&mut self.reflection);
        if let Some(i) = self.exposed_resource_by_name(name) {
            assert!(
                self.exposed[i as usize].type_id == TypeId::of::<T>(),
                "resource name `{name}` is already exposed for another type"
            );
            return i;
        }
        self.exposed.push(ExposedResource {
            name: name.into(),
            type_id: TypeId::of::<T>(),
            reflect,
        });
        (self.exposed.len() - 1) as u32
    }

    /// Every exposed resource: its script-visible name and layout.
    pub fn exposed_resources(&self) -> impl Iterator<Item = (&str, ReflectId)> {
        self.exposed.iter().map(|r| (&*r.name, r.reflect))
    }

    pub fn exposed_resource_by_name(&self, name: &str) -> Option<u32> {
        self.exposed
            .iter()
            .position(|r| &*r.name == name)
            .map(|i| i as u32)
    }

    /// The layout an exposed resource was registered with.
    pub fn exposed_resource_reflect(&self, id: u32) -> Option<ReflectId> {
        self.exposed.get(id as usize).map(|r| r.reflect)
    }

    /// The type of an exposed resource, for declaring access.
    pub fn exposed_resource_type(&self, id: u32) -> Option<TypeId> {
        self.exposed.get(id as usize).map(|r| r.type_id)
    }

    /// The address of an exposed resource's value, or `None` if the id is
    /// unknown or the resource was not inserted. Valid until the resource is
    /// inserted or removed: nothing can do that while a system runs, because
    /// `Commands::insert_resource` is deferred.
    pub fn exposed_resource_ptr(&mut self, id: u32) -> Option<*mut u8> {
        let type_id = self.exposed.get(id as usize)?.type_id;
        let boxed = self.resources.get_mut(&type_id)?;
        let value: *mut (dyn std::any::Any + Send + Sync) = &raw mut **boxed;
        Some(value.cast::<u8>())
    }
}

/// Time as scripts see it. Owned and advanced by [`Schedule`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "vortex::Time", script)]
pub struct Time {
    /// Seconds the current frame took (the simulated frame time).
    pub delta: f32,
    /// Seconds per `FixedUpdate` step.
    pub fixed_delta: f32,
    /// Seconds since the first frame.
    pub elapsed: f64,
    /// Frames started so far.
    pub frame: u64,
    /// Nonzero while `FixedUpdate` runs; scripts then read `fixed_delta`.
    pub in_fixed: u8,
}

macro_rules! key_codes {
    ($($name:ident),* $(,)?) => {
        /// Keys and mouse buttons, numbered in `InputState` bit order.
        /// The C# `KeyCode` enum has exactly these names and numbers (the
        /// generator emits it from [`KeyCode::ALL`]).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum KeyCode {
            $($name),*
        }

        impl KeyCode {
            /// Every key with its name, in numeric order.
            pub const ALL: &'static [(&'static str, u16)] = &[
                $((stringify!($name), KeyCode::$name as u16)),*
            ];
        }
    };
}

key_codes!(
    None,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftAlt,
    RightAlt,
    Up,
    Down,
    Left,
    Right,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    MouseLeft,
    MouseRight,
    MouseMiddle,
);

/// Keys are numbered by the script-visible `KeyCode`; 256 bits each.
pub const KEY_BITS: usize = 256;
/// Axis slots.
pub const AXES: usize = 32;

/// Raw input state for the frame. The platform layer fills it during
/// `PreUpdate`; tests fill it directly.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "vortex::Input", script)]
pub struct InputState {
    pub down: [u64; 4],
    pub pressed: [u64; 4],
    pub released: [u64; 4],
    pub mouse_delta: [f32; 2],
    pub axes: [f32; AXES],
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            down: [0; 4],
            pressed: [0; 4],
            released: [0; 4],
            mouse_delta: [0.0; 2],
            axes: [0.0; AXES],
        }
    }
}

impl InputState {
    /// Forgets what only lasts one frame: presses, releases, mouse motion.
    pub fn begin_frame(&mut self) {
        self.pressed = [0; 4];
        self.released = [0; 4];
        self.mouse_delta = [0.0; 2];
    }

    fn bit(key: usize) -> (usize, u64) {
        assert!(key < KEY_BITS, "key code {key} is out of range");
        (key / 64, 1 << (key % 64))
    }

    pub fn press(&mut self, key: usize) {
        let (w, b) = Self::bit(key);
        if self.down[w] & b == 0 {
            self.pressed[w] |= b;
        }
        self.down[w] |= b;
    }

    pub fn release(&mut self, key: usize) {
        let (w, b) = Self::bit(key);
        if self.down[w] & b != 0 {
            self.released[w] |= b;
        }
        self.down[w] &= !b;
    }

    pub fn is_down(&self, key: usize) -> bool {
        let (w, b) = Self::bit(key);
        self.down[w] & b != 0
    }

    pub fn set_axis(&mut self, slot: usize, value: f32) {
        self.axes[slot] = value;
    }
}

/// Names of axes ("Horizontal") to slots in [`InputState::axes`]. Rust-only:
/// reflected data cannot hold strings.
#[derive(Clone, Debug, Default)]
pub struct InputMap {
    slots: HashMap<String, u32>,
}

impl InputMap {
    /// Binds `name` to `slot`, replacing an earlier binding.
    ///
    /// # Panics
    /// If `slot` is not below [`AXES`].
    pub fn bind(&mut self, name: &str, slot: usize) {
        assert!(slot < AXES, "axis slot {slot} is out of range");
        self.slots.insert(name.to_owned(), slot as u32);
    }

    pub fn slot(&self, name: &str) -> Option<u32> {
        self.slots.get(name).copied()
    }
}

/// Registers the engine's script-visible resources and inserts their
/// defaults. Called by `World::new`.
pub(crate) fn init(world: &mut World) {
    world.insert_resource(Time::default());
    world.insert_resource(InputState::default());
    world.insert_resource(InputMap::default());
    world.expose_resource::<Time>("vortex::Time");
    world.expose_resource::<InputState>("vortex::Input");
}
