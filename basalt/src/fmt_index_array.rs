pub struct IndexVec<'a, T> {
    data: &'a [T],
}

impl<'a, T> From<&'a Vec<T>> for IndexVec<'a, T> {
    fn from(value: &'a Vec<T>) -> Self {
        Self {
            data: value.as_slice(),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for IndexVec<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct IndexedEntry<'a, T> {
            idx: usize,
            val: &'a T,
        }

        impl<T: std::fmt::Debug> std::fmt::Debug for IndexedEntry<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, " ID {} : ", self.idx)?;

                self.val.fmt(f)
            }
        }

        f.debug_list()
            .entries(
                self.data
                    .iter()
                    .enumerate()
                    .map(|(idx, val)| IndexedEntry { idx, val }),
            )
            .finish()
    }
}
