use std::alloc::Layout;

use crate::{
    blob_vec::{BlobVec, DropFn},
    tick::Tick,
};

/// One component's values for every row of an archetype, plus per-row ticks.
/// The three arrays always have the same length.
pub struct Column {
    data: BlobVec,
    added: Vec<Tick>,
    changed: Vec<Tick>,
}

impl Column {
    /// # Safety
    /// See [`BlobVec::new`].
    pub unsafe fn new(item: Layout, drop: Option<DropFn>) -> Self {
        Self {
            // SAFETY: forwarded contract.
            data: unsafe { BlobVec::new(item, drop) },
            added: Vec::new(),
            changed: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn item_layout(&self) -> Layout {
        self.data.item_layout()
    }

    /// # Safety
    /// See [`BlobVec::push`].
    pub unsafe fn push(&mut self, src: *const u8, tick: Tick) {
        // SAFETY: forwarded contract.
        unsafe { self.data.push(src) };
        self.added.push(tick);
        self.changed.push(tick);
    }

    /// Moves row `row` of `src` onto the end of this column, keeping its
    /// ticks. The source row must afterwards be removed with
    /// [`swap_remove_and_forget`](Self::swap_remove_and_forget).
    ///
    /// # Safety
    /// Both columns must hold the same component type, and `src`'s row must
    /// not be dropped or read again as an owner.
    pub unsafe fn push_moved_from(&mut self, src: &Column, row: usize) {
        // SAFETY: same layout; src row is valid and in a different buffer.
        unsafe { self.data.push(src.data.get_ptr(row)) };
        self.added.push(src.added[row]);
        self.changed.push(src.changed[row]);
    }

    pub fn get_ptr(&self, row: usize) -> *mut u8 {
        self.data.get_ptr(row)
    }

    pub fn added(&self, row: usize) -> Tick {
        self.added[row]
    }

    pub fn changed(&self, row: usize) -> Tick {
        self.changed[row]
    }

    /// Overwrites a row, dropping the old value, and stamps `changed`.
    ///
    /// # Safety
    /// See [`BlobVec::replace`].
    pub unsafe fn replace(&mut self, row: usize, src: *const u8, tick: Tick) {
        // SAFETY: forwarded contract.
        unsafe { self.data.replace(row, src) };
        self.changed[row] = tick;
    }

    pub fn set_changed(&mut self, row: usize, tick: Tick) {
        self.changed[row] = tick;
    }

    pub fn swap_remove_and_drop(&mut self, row: usize) {
        self.data.swap_remove_and_drop(row);
        self.added.swap_remove(row);
        self.changed.swap_remove(row);
    }

    /// # Safety
    /// See [`BlobVec::swap_remove_and_forget`].
    pub unsafe fn swap_remove_and_forget(&mut self, row: usize) {
        // SAFETY: forwarded contract.
        unsafe { self.data.swap_remove_and_forget(row) };
        self.added.swap_remove(row);
        self.changed.swap_remove(row);
    }

    /// # Safety
    /// See [`BlobVec::swap_remove_into`].
    pub unsafe fn swap_remove_into(&mut self, row: usize, dst: *mut u8) {
        // SAFETY: forwarded contract.
        unsafe { self.data.swap_remove_into(row, dst) };
        self.added.swap_remove(row);
        self.changed.swap_remove(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_follow_rows() {
        // SAFETY: u32 needs no drop.
        let mut c = unsafe { Column::new(Layout::new::<u32>(), None) };
        for i in 0..3u32 {
            // SAFETY: valid u32.
            unsafe { c.push((&raw const i).cast(), Tick::new(i)) };
        }
        c.swap_remove_and_drop(0);
        assert_eq!(c.len(), 2);
        // Row 0 is now what was row 2.
        assert_eq!(c.added(0), Tick::new(2));
        // SAFETY: row 0 holds a u32.
        assert_eq!(unsafe { *c.get_ptr(0).cast::<u32>() }, 2);

        let v = 9u32;
        // SAFETY: valid u32.
        unsafe { c.replace(1, (&raw const v).cast(), Tick::new(7)) };
        assert_eq!(c.changed(1), Tick::new(7));
        assert_eq!(c.added(1), Tick::new(1));
    }
}
