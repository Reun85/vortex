use std::num::NonZeroU32;

/// A type that is guaranteed to not be `T::max`.
/// This is achieved via the `NonZero + 1` wrapper. Allowing for optimizations when using `Option<NonMaxType<T>`
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NonMaxU32(NonZeroU32);

/// An index into a collection of type `T`.
#[must_use]
pub struct Index<T> {
    value: NonMaxU32,
    _marker: std::marker::PhantomData<T>,
}
/// A range of indices into a collection of type `T`.
#[must_use]
pub struct IndexRange<T> {
    start: NonMaxU32,

    exclusive_end: NonMaxU32,
    _marker: std::marker::PhantomData<[T]>,
}

impl NonMaxU32 {
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value.wrapping_add(1)) {
            Some(non_zero) => Some(Self(non_zero)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0.get().wrapping_sub(1)
    }
    /// # Safety
    /// The caller must ensure that `value` is not `u32::MAX`.
    pub const unsafe fn new_unchecked(value: u32) -> Self {
        Self(unsafe { NonZeroU32::new_unchecked(value.wrapping_add(1)) })
    }
}

impl<T> Index<T> {
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        match NonMaxU32::new(value) {
            Some(non_max) => Some(Self {
                value: non_max,
                _marker: std::marker::PhantomData,
            }),
            None => None,
        }
    }
    /// # Safety
    /// The caller must ensure that `value` is not `u32::MAX`.
    pub const unsafe fn new_unchecked(value: u32) -> Self {
        Self {
            value: unsafe { NonMaxU32::new_unchecked(value) },
            _marker: std::marker::PhantomData,
        }
    }

    #[must_use]
    pub const fn get(&self) -> u32 {
        self.value.get()
    }
    /// # Safety
    /// This method is only safe to call if the caller ensures that the resulting index is valid for the collection it will be used with.
    pub const fn transfer<Q>(&self) -> Index<Q> {
        Index {
            value: self.value,
            _marker: std::marker::PhantomData,
        }
    }
    #[must_use]
    pub const fn to_usize(&self) -> usize {
        self.get() as usize
    }
}

impl<T> From<Index<T>> for usize {
    fn from(value: Index<T>) -> Self {
        value.get() as Self
    }
}
impl<T> From<Index<T>> for u32 {
    fn from(value: Index<T>) -> Self {
        value.get()
    }
}
impl<T> IndexRange<T> {
    #[must_use]
    pub const fn new(start: u32, exclusive_end: u32) -> Option<Self> {
        match (NonMaxU32::new(start), NonMaxU32::new(exclusive_end)) {
            (Some(start), Some(exclusive_end)) => Some(Self {
                start,
                exclusive_end,
                _marker: std::marker::PhantomData,
            }),
            _ => None,
        }
    }
    #[must_use]
    pub const fn count(&self) -> usize {
        (self.exclusive_end.get() - self.start.get()) as usize
    }
    /// # Safety
    /// Neither `start` nor `exclusive_end` can be  `u32::MAX`.
    pub const unsafe fn new_unchecked(start: u32, exclusive_end: u32) -> Self {
        Self {
            start: unsafe { NonMaxU32::new_unchecked(start) },
            exclusive_end: unsafe { NonMaxU32::new_unchecked(exclusive_end) },
            _marker: std::marker::PhantomData,
        }
    }
    #[must_use]
    #[doc(alias = "from_count_and_start")]
    pub const fn from_start_and_length(
        start: u32,
        length: u32,
    ) -> Option<Self> {
        match start.checked_add(length) {
            Some(exclusive_end) => Self::new(start, exclusive_end),
            None => None,
        }
    }
    #[must_use]
    pub const fn new_inclusive(start: u32, inclusive_end: u32) -> Option<Self> {
        match inclusive_end.checked_add(1) {
            Some(exclusive_end) => Self::new(start, exclusive_end),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(&self) -> std::ops::Range<u32> {
        self.start.get()..self.exclusive_end.get()
    }
    #[must_use]
    pub const fn start(&self) -> u32 {
        self.start.get()
    }
    #[must_use]
    pub const fn exclusive_end(&self) -> u32 {
        self.exclusive_end.get()
    }
    #[must_use]
    pub const fn to_range(self) -> std::ops::Range<usize> {
        self.get().start as usize..self.get().end as usize
    }
    pub const fn transfer<Q>(&self) -> IndexRange<Q> {
        IndexRange {
            start: self.start,
            exclusive_end: self.exclusive_end,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> std::ops::Index<Index<T>> for Vec<T> {
    type Output = T;

    fn index(&self, index: Index<T>) -> &Self::Output {
        &self[index.get() as usize]
    }
}
impl<T> std::ops::IndexMut<Index<T>> for Vec<T> {
    fn index_mut(&mut self, index: Index<T>) -> &mut Self::Output {
        &mut self[index.get() as usize]
    }
}
impl<T> std::ops::Index<Index<T>> for [T] {
    type Output = T;

    fn index(&self, index: Index<T>) -> &Self::Output {
        &self[index.get() as usize]
    }
}
impl<T> std::ops::IndexMut<Index<T>> for [T] {
    fn index_mut(&mut self, index: Index<T>) -> &mut Self::Output {
        &mut self[index.get() as usize]
    }
}
impl<T> std::ops::Index<Index<T>> for std::collections::VecDeque<T> {
    type Output = T;

    fn index(&self, index: Index<T>) -> &Self::Output {
        &self[index.get() as usize]
    }
}
impl<T> std::ops::IndexMut<Index<T>> for std::collections::VecDeque<T> {
    fn index_mut(&mut self, index: Index<T>) -> &mut Self::Output {
        &mut self[index.get() as usize]
    }
}
impl<T> std::ops::Index<IndexRange<T>> for Vec<T> {
    type Output = [T];

    fn index(&self, index: IndexRange<T>) -> &Self::Output {
        &self[index.start.get() as usize..index.exclusive_end.get() as usize]
    }
}
impl<T> std::ops::IndexMut<IndexRange<T>> for Vec<T> {
    fn index_mut(&mut self, index: IndexRange<T>) -> &mut Self::Output {
        &mut self
            [index.start.get() as usize..index.exclusive_end.get() as usize]
    }
}
impl<T> std::ops::Index<IndexRange<T>> for [T] {
    type Output = [T];

    fn index(&self, index: IndexRange<T>) -> &Self::Output {
        &self[index.start.get() as usize..index.exclusive_end.get() as usize]
    }
}
impl<T> std::ops::IndexMut<IndexRange<T>> for [T] {
    fn index_mut(&mut self, index: IndexRange<T>) -> &mut Self::Output {
        &mut self
            [index.start.get() as usize..index.exclusive_end.get() as usize]
    }
}
impl<T> std::fmt::Debug for Index<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Index<{}>({})", std::any::type_name::<T>(), self.get())
    }
}

impl<T> std::fmt::Debug for IndexRange<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IndexRange<{}>({}..{})",
            std::any::type_name::<T>(),
            self.start.get(),
            self.exclusive_end.get()
        )
    }
}

impl<T> From<IndexRange<T>> for std::ops::Range<usize> {
    fn from(value: IndexRange<T>) -> Self {
        value.get().start as usize..value.get().end as usize
    }
}

// Common implementations
impl<T> Copy for Index<T> {}
impl<T> Clone for Index<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for Index<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T> Eq for Index<T> {}
impl<T> Ord for Index<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T> PartialOrd for Index<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> std::hash::Hash for Index<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> Copy for IndexRange<T> {}
impl<T> Clone for IndexRange<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for IndexRange<T> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.exclusive_end == other.exclusive_end
    }
}
impl<T> Eq for IndexRange<T> {}
impl<T> Ord for IndexRange<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.start
            .cmp(&other.start)
            .then_with(|| self.exclusive_end.cmp(&other.exclusive_end))
    }
}
impl<T> PartialOrd for IndexRange<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> std::hash::Hash for IndexRange<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.start.hash(state);
        self.exclusive_end.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_vec() {
        #[expect(clippy::useless_vec)]
        let mut vec = vec![10, 20, 30, 40, 50];
        let index = Index::<i32>::new(2).unwrap();
        assert_eq!(vec[index], 30);
        vec[index] = 35;
        assert_eq!(vec[index], 35);
    }
    #[test]
    fn index_arr() {
        #[expect(clippy::useless_vec)]
        let mut vec = [10, 20, 30, 40, 50];
        let index = Index::<i32>::new(2).unwrap();
        assert_eq!(vec[index], 30);
        vec[index] = 35;
        assert_eq!(vec[index], 35);
    }
    #[test]
    fn index_range_vec() {
        #[expect(clippy::useless_vec)]
        let mut vec = vec![10, 20, 30, 40, 50];
        let index_range = IndexRange::<i32>::new(0, 4).unwrap();
        assert_eq!(&vec[index_range], &[10, 20, 30, 40]);
        vec[index_range][2] = 35;
        assert_eq!(&vec[index_range], &[10, 20, 35, 40]);
    }
    #[test]
    fn index_range_arr() {
        let mut vec = [10, 20, 30, 40, 50];
        let index_range = IndexRange::<i32>::new(1, 4).unwrap();
        assert_eq!(&vec[index_range], &[20, 30, 40]);
        vec[index_range][1] = 35;
        assert_eq!(&vec[index_range], &[20, 35, 40]);
    }
    #[test]
    fn index_debug() {
        let index = Index::<i32>::new(5).unwrap();
        assert_eq!(format!("{index:?}"), "Index<i32>(5)");
    }
    #[test]
    fn index_range_debug() {
        let index_range = IndexRange::<i32>::new(1, 4).unwrap();
        assert_eq!(format!("{index_range:?}"), "IndexRange<i32>(1..4)");
    }
}
