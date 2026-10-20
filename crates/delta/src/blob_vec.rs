use std::{
    alloc::{self, Layout},
    ptr::{self, NonNull},
};

/// Type-erased drop function for one element.
pub type DropFn = unsafe fn(*mut u8);

/// A growable, type-erased vector of equally laid out elements.
///
/// Zero-sized elements are supported and never allocate; they model tags.
pub struct BlobVec {
    item: Layout,
    drop: Option<DropFn>,
    ptr: NonNull<u8>,
    len: usize,
    cap: usize,
}

// SAFETY: the vec owns its elements; thread-safety of the element type is the
// responsibility of whoever registers the component (see `Components`).
unsafe impl Send for BlobVec {}
unsafe impl Sync for BlobVec {}

impl BlobVec {
    /// Creates an empty vec of elements with `item` layout.
    ///
    /// # Safety
    /// `drop`, when given, must be valid to call on any initialized element
    /// pushed into this vec.
    pub unsafe fn new(item: Layout, drop: Option<DropFn>) -> Self {
        let cap = if item.size() == 0 { usize::MAX } else { 0 };
        Self {
            item,
            drop,
            ptr: dangling(item),
            len: 0,
            cap,
        }
    }

    pub fn item_layout(&self) -> Layout {
        self.item
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Ensures room for `additional` more elements, growing by doubling.
    pub fn reserve(&mut self, additional: usize) {
        let required = self
            .len
            .checked_add(additional)
            .expect("BlobVec capacity overflow");
        if required <= self.cap {
            return;
        }
        // Only reachable for non-zero-sized items (zst cap is usize::MAX).
        let new_cap = required.max(self.cap.saturating_mul(2)).max(4);
        let new_layout = self.array_layout(new_cap);
        let new_ptr = if self.cap == 0 {
            // SAFETY: the layout has non-zero size because item size != 0.
            unsafe { alloc::alloc(new_layout) }
        } else {
            // SAFETY: `ptr` was allocated with `array_layout(self.cap)`, and
            // the new size is non-zero and does not overflow `isize`.
            unsafe {
                alloc::realloc(
                    self.ptr.as_ptr(),
                    self.array_layout(self.cap),
                    new_layout.size(),
                )
            }
        };
        self.ptr = NonNull::new(new_ptr)
            .unwrap_or_else(|| alloc::handle_alloc_error(new_layout));
        self.cap = new_cap;
    }

    /// Moves the element at `src` into the end of the vec.
    ///
    /// # Safety
    /// `src` must point to an initialized element of the vec's layout that
    /// does not overlap the vec's buffer. Ownership moves into the vec.
    pub unsafe fn push(&mut self, src: *const u8) {
        self.reserve(1);
        // SAFETY: capacity was reserved; src is valid per the contract.
        unsafe {
            ptr::copy_nonoverlapping(
                src,
                self.slot(self.len),
                self.item.size(),
            );
        }
        self.len += 1;
    }

    /// Pointer to element `index`.
    ///
    /// # Panics
    /// If `index >= len`.
    pub fn get_ptr(&self, index: usize) -> *mut u8 {
        assert!(index < self.len, "BlobVec index out of bounds");
        self.slot(index)
    }

    /// Overwrites element `index` with the value at `src`, dropping the old.
    ///
    /// # Safety
    /// Same contract as [`push`](Self::push) for `src`.
    pub unsafe fn replace(&mut self, index: usize, src: *const u8) {
        let dst = self.get_ptr(index);
        // SAFETY: dst holds an initialized element; src is valid and disjoint.
        unsafe {
            if let Some(drop) = self.drop {
                drop(dst);
            }
            ptr::copy_nonoverlapping(src, dst, self.item.size());
        }
    }

    /// Removes element `index`, moving the last element into its place, and
    /// drops the removed element.
    pub fn swap_remove_and_drop(&mut self, index: usize) {
        let ptr = self.get_ptr(index);
        // SAFETY: ptr holds an initialized element that is being removed.
        unsafe {
            if let Some(drop) = self.drop {
                drop(ptr);
            }
            self.remove_slot(index);
        }
    }

    /// Removes element `index` like [`swap_remove_and_drop`] but moves its
    /// bytes to `dst` instead of dropping them.
    ///
    /// # Safety
    /// `dst` must be valid for writes of one element and not overlap the vec.
    ///
    /// [`swap_remove_and_drop`]: Self::swap_remove_and_drop
    pub unsafe fn swap_remove_into(&mut self, index: usize, dst: *mut u8) {
        let ptr = self.get_ptr(index);
        // SAFETY: ptr is a valid element, dst valid per the contract.
        unsafe {
            ptr::copy_nonoverlapping(ptr, dst, self.item.size());
            self.remove_slot(index);
        }
    }

    /// Removes element `index` without dropping it; the caller already moved
    /// its value out.
    ///
    /// # Safety
    /// The element's ownership must have been transferred elsewhere.
    pub unsafe fn swap_remove_and_forget(&mut self, index: usize) {
        assert!(index < self.len, "BlobVec index out of bounds");
        // SAFETY: index is in bounds.
        unsafe { self.remove_slot(index) }
    }

    /// Drops every element, keeping the allocation.
    pub fn clear(&mut self) {
        let len = self.len;
        // Set first so a panicking drop cannot cause a double drop.
        self.len = 0;
        if let Some(drop) = self.drop {
            for i in 0..len {
                // SAFETY: elements 0..len were initialized.
                unsafe { drop(self.slot(i)) };
            }
        }
    }

    /// Moves the last element into `index`'s slot and shrinks by one.
    ///
    /// # Safety
    /// `index < len`, and the element at `index` is already moved out/dropped.
    unsafe fn remove_slot(&mut self, index: usize) {
        let last = self.len - 1;
        if index != last {
            // SAFETY: distinct in-bounds slots never overlap.
            unsafe {
                ptr::copy_nonoverlapping(
                    self.slot(last),
                    self.slot(index),
                    self.item.size(),
                );
            }
        }
        self.len = last;
    }

    fn slot(&self, index: usize) -> *mut u8 {
        // SAFETY: callers keep index <= cap, so the offset stays inside (or one
        // past) the allocation; for zsts the offset is 0.
        unsafe { self.ptr.as_ptr().add(index * self.item.size()) }
    }

    fn array_layout(&self, cap: usize) -> Layout {
        let size = self
            .item
            .size()
            .checked_mul(cap)
            .expect("BlobVec capacity overflow");
        Layout::from_size_align(size, self.item.align())
            .expect("BlobVec capacity overflow")
    }
}

impl Drop for BlobVec {
    fn drop(&mut self) {
        self.clear();
        if self.item.size() != 0 && self.cap != 0 {
            // SAFETY: allocated in `reserve` with exactly this layout.
            unsafe {
                alloc::dealloc(self.ptr.as_ptr(), self.array_layout(self.cap))
            };
        }
    }
}

fn dangling(item: Layout) -> NonNull<u8> {
    // Non-null and aligned; never dereferenced while there is no allocation.
    NonNull::new(ptr::without_provenance_mut(item.align()))
        .expect("alignment is non-zero")
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use proptest::prelude::*;

    use super::*;

    unsafe fn drop_t<T>(p: *mut u8) {
        // SAFETY: test-only; p points to an initialized T.
        unsafe { ptr::drop_in_place(p.cast::<T>()) }
    }

    fn vec_of<T>() -> BlobVec {
        let drop = std::mem::needs_drop::<T>().then_some(drop_t::<T> as DropFn);
        // SAFETY: drop_t::<T> matches the layout.
        unsafe { BlobVec::new(Layout::new::<T>(), drop) }
    }

    fn push<T>(v: &mut BlobVec, value: T) {
        let value = std::mem::ManuallyDrop::new(value);
        // SAFETY: value is a valid T, ownership moves into the vec.
        unsafe { v.push(ptr::from_ref(&*value).cast()) }
    }

    fn get<T: Copy>(v: &BlobVec, i: usize) -> T {
        // SAFETY: element i is an initialized T.
        unsafe { *v.get_ptr(i).cast::<T>() }
    }

    #[test]
    fn push_get_grow() {
        let mut v = vec_of::<u64>();
        for i in 0..100u64 {
            push(&mut v, i);
        }
        assert_eq!(v.len(), 100);
        assert!((0..100u64).all(|i| get::<u64>(&v, i as usize) == i));
    }

    #[test]
    fn over_aligned() {
        #[repr(align(64))]
        #[derive(Clone, Copy)]
        struct A(u8);
        let mut v = vec_of::<A>();
        for i in 0..10 {
            push(&mut v, A(i));
        }
        for i in 0..10 {
            assert_eq!(v.get_ptr(i).addr() % 64, 0);
            assert_eq!(get::<A>(&v, i).0 as usize, i);
        }
    }

    #[test]
    fn zst_never_allocates() {
        let mut v = vec_of::<()>();
        for _ in 0..1000 {
            push(&mut v, ());
        }
        assert_eq!(v.len(), 1000);
        v.swap_remove_and_drop(0);
        assert_eq!(v.len(), 999);
    }

    struct Counter(Rc<Cell<usize>>);
    impl Drop for Counter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn drops_exactly_once() {
        let drops = Rc::new(Cell::new(0));
        let mut v = vec_of::<Counter>();
        for _ in 0..5 {
            push(&mut v, Counter(drops.clone()));
        }
        v.swap_remove_and_drop(4); // last row
        assert_eq!(drops.get(), 1);
        v.swap_remove_and_drop(0); // swaps last into 0
        assert_eq!(drops.get(), 2);
        let new = std::mem::ManuallyDrop::new(Counter(drops.clone()));
        // SAFETY: valid Counter, moved into the vec.
        unsafe { v.replace(1, ptr::from_ref(&*new).cast()) };
        assert_eq!(drops.get(), 3);
        drop(v);
        assert_eq!(drops.get(), 6);
    }

    #[test]
    fn swap_remove_into_moves_without_drop() {
        let drops = Rc::new(Cell::new(0));
        let mut v = vec_of::<Counter>();
        push(&mut v, Counter(drops.clone()));
        push(&mut v, Counter(drops.clone()));
        let mut out = std::mem::MaybeUninit::<Counter>::uninit();
        // SAFETY: out is valid for one Counter.
        unsafe { v.swap_remove_into(0, out.as_mut_ptr().cast()) };
        assert_eq!(drops.get(), 0);
        // SAFETY: just initialized by swap_remove_into.
        drop(unsafe { out.assume_init() });
        assert_eq!(drops.get(), 1);
        drop(v);
        assert_eq!(drops.get(), 2);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Push(u32),
        SwapRemove(usize),
        Replace(usize, u32),
        Clear,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => any::<u32>().prop_map(Op::Push),
            3 => any::<usize>().prop_map(Op::SwapRemove),
            2 => (any::<usize>(), any::<u32>())
                .prop_map(|(i, x)| Op::Replace(i, x)),
            1 => Just(Op::Clear),
        ]
    }

    proptest! {
        #[test]
        fn matches_vec_model(ops in proptest::collection::vec(op(), 0..200)) {
            let mut v = vec_of::<u32>();
            let mut model: Vec<u32> = Vec::new();
            for op in ops {
                match op {
                    Op::Push(x) => {
                        push(&mut v, x);
                        model.push(x);
                    }
                    Op::SwapRemove(i) if !model.is_empty() => {
                        let i = i % model.len();
                        v.swap_remove_and_drop(i);
                        model.swap_remove(i);
                    }
                    Op::Replace(i, x) if !model.is_empty() => {
                        let i = i % model.len();
                        let x = std::mem::ManuallyDrop::new(x);
                        // SAFETY: valid u32.
                        unsafe { v.replace(i, ptr::from_ref(&*x).cast()) };
                        model[i] = *x;
                    }
                    Op::Clear => {
                        v.clear();
                        model.clear();
                    }
                    _ => {}
                }
                prop_assert_eq!(v.len(), model.len());
                for (i, m) in model.iter().enumerate() {
                    prop_assert_eq!(get::<u32>(&v, i), *m);
                }
            }
        }
    }
}
