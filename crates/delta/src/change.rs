use std::ops::{Deref, DerefMut};

use crate::tick::{Tick, TickCell};

/// Mutable access to a component that marks it changed when written through.
///
/// Reading through a `Mut` (including via `Deref`) costs nothing; only
/// `DerefMut` stamps the row's `changed` tick, so code that takes `&mut` but
/// does not write does not wake `Changed<T>` filters.
pub struct Mut<'w, T: ?Sized> {
    value: &'w mut T,
    changed: &'w TickCell,
    this_run: Tick,
}

impl<'w, T: ?Sized> Mut<'w, T> {
    pub(crate) fn new(
        value: &'w mut T,
        changed: &'w TickCell,
        this_run: Tick,
    ) -> Self {
        Self {
            value,
            changed,
            this_run,
        }
    }

    /// Whether this component was changed since `last_run`.
    pub fn is_changed_since(&self, last_run: Tick) -> bool {
        // SAFETY: this `Mut` has exclusive access to the row.
        let changed = unsafe { self.changed.read() };
        changed.is_newer_than(last_run, self.this_run)
    }

    /// Marks the component changed without touching it.
    pub fn set_changed(&mut self) {
        // SAFETY: this `Mut` has exclusive access to the row.
        unsafe { self.changed.write(self.this_run) };
    }

    /// Mutable access that does not mark the component changed.
    pub fn bypass_change_detection(&mut self) -> &mut T {
        self.value
    }
}

impl<T: ?Sized> Deref for Mut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> DerefMut for Mut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.set_changed();
        self.value
    }
}
