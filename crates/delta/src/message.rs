//! Buffered messages: "what happened this frame, for whoever reads it".
//! Double-buffered so a message survives one full frame boundary, which
//! lets readers that run before or after the writer both see it.

use std::{any::TypeId, mem};

use crate::{
    system::{Access, SystemParam},
    tick::Tick,
    world::{World, WorldCell},
};

/// The resource holding all live messages of type `T`.
pub struct Messages<T> {
    previous: Vec<T>,
    current: Vec<T>,
    /// Id of `previous[0]`; ids grow forever, so readers can tell what they
    /// have already seen.
    base: usize,
}

impl<T> Default for Messages<T> {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            current: Vec::new(),
            base: 0,
        }
    }
}

impl<T> Messages<T> {
    pub fn send(&mut self, message: T) {
        self.current.push(message);
    }

    /// Ends the frame: messages older than one full update are dropped.
    pub fn update(&mut self) {
        self.base += self.previous.len();
        self.previous = mem::take(&mut self.current);
    }

    pub fn len(&self) -> usize {
        self.previous.len() + self.current.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Messages not yet seen through `cursor`, which then advances.
    pub fn read_since(&self, cursor: &mut usize) -> impl Iterator<Item = &T> {
        let start = (*cursor).max(self.base);
        *cursor = self.base + self.previous.len() + self.current.len();
        let into_previous = start - self.base;
        let into_current =
            start.saturating_sub(self.base + self.previous.len());
        self.previous
            .iter()
            .skip(into_previous)
            .chain(self.current.iter().skip(into_current))
    }
}

/// Sends messages of type `T`.
pub struct MessageWriter<'w, T: Send + Sync + 'static> {
    messages: &'w mut Messages<T>,
}

impl<T: Send + Sync + 'static> MessageWriter<'_, T> {
    pub fn send(&mut self, message: T) {
        self.messages.send(message);
    }
}

// SAFETY: declares write access to `Messages<T>`, the only thing it touches.
unsafe impl<T: Send + Sync + 'static> SystemParam for MessageWriter<'_, T> {
    type State = ();
    type Item<'w, 's> = MessageWriter<'w, T>;

    fn init_state(world: &mut World, access: &mut Access) {
        world.init_messages::<T>();
        access.add_resource_write::<Messages<T>>();
    }

    unsafe fn get_param<'w>(
        _: &mut (),
        _: Tick,
        world: WorldCell<'w>,
    ) -> MessageWriter<'w, T> {
        MessageWriter {
            // SAFETY: write access was declared.
            messages: unsafe { world.resource_mut::<Messages<T>>() }
                .expect("messages are initialized with the parameter"),
        }
    }
}

/// Reads each message of type `T` once per reader.
pub struct MessageReader<'w, 's, T: Send + Sync + 'static> {
    messages: &'w Messages<T>,
    cursor: &'s mut usize,
}

impl<T: Send + Sync + 'static> MessageReader<'_, '_, T> {
    /// The messages sent since this reader last read.
    pub fn read(&mut self) -> impl Iterator<Item = &T> {
        self.messages.read_since(self.cursor)
    }

    /// Whether `read` would yield anything.
    pub fn is_empty(&self) -> bool {
        let mut cursor = *self.cursor;
        self.messages.read_since(&mut cursor).next().is_none()
    }
}

// SAFETY: declares read access to `Messages<T>`, the only thing it touches.
unsafe impl<T: Send + Sync + 'static> SystemParam for MessageReader<'_, '_, T> {
    type State = usize;
    type Item<'w, 's> = MessageReader<'w, 's, T>;

    fn init_state(world: &mut World, access: &mut Access) -> usize {
        world.init_messages::<T>();
        access.add_resource_read::<Messages<T>>();
        0
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut usize,
        _: Tick,
        world: WorldCell<'w>,
    ) -> MessageReader<'w, 's, T> {
        MessageReader {
            // SAFETY: read access was declared.
            messages: unsafe { world.resource::<Messages<T>>() }
                .expect("messages are initialized with the parameter"),
            cursor: state,
        }
    }
}

impl World {
    /// Creates the `Messages<T>` resource and schedules its per-frame swap.
    pub fn init_messages<T: Send + Sync + 'static>(&mut self) {
        if self.resource::<Messages<T>>().is_some() {
            return;
        }
        self.insert_resource(Messages::<T>::default());
        self.message_updaters.push((TypeId::of::<T>(), |world| {
            if let Some(m) = world.resource_mut::<Messages<T>>() {
                m.update();
            }
        }));
    }

    pub fn send_message<T: Send + Sync + 'static>(&mut self, message: T) {
        self.init_messages::<T>();
        self.resource_mut::<Messages<T>>()
            .expect("just initialized")
            .send(message);
    }

    /// Swaps every message buffer; the schedule calls this once per frame.
    pub fn update_messages(&mut self) {
        let updaters: Vec<_> =
            self.message_updaters.iter().map(|&(_, f)| f).collect();
        for update in updaters {
            update(self);
        }
    }
}
