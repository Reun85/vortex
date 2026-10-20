pub struct DeduplicateByValue<Iter, F, U>
where
    Iter: std::iter::Iterator,
    F: Fn(&Iter::Item) -> U,
    U: std::hash::Hash,
{
    iter: Iter,
    f: F,
    set: std::collections::HashSet<U>,
}
impl<Iter, F, U> DeduplicateByValue<Iter, F, U>
where
    Iter: std::iter::Iterator,
    F: Fn(&Iter::Item) -> U,
    U: std::hash::Hash,
{
    pub fn new(iter: Iter, f: F) -> Self {
        Self {
            iter,
            f,
            set: std::collections::HashSet::new(),
        }
    }
}

pub trait DeduplicateByValueExt: Sized + std::iter::Iterator {
    fn deduplicate_by_value<F, U>(self, f: F) -> DeduplicateByValue<Self, F, U>
    where
        F: Fn(&Self::Item) -> U,
        U: std::hash::Hash + std::cmp::Eq,
    {
        DeduplicateByValue::new(self, f)
    }
}

impl<Iter, F, U> std::iter::Iterator for DeduplicateByValue<Iter, F, U>
where
    Iter: std::iter::Iterator,
    F: Fn(&Iter::Item) -> U,
    U: std::hash::Hash + std::cmp::Eq,
{
    type Item = Iter::Item;
    fn next(&mut self) -> Option<Self::Item> {
        for x in self.iter.by_ref() {
            let v = (self.f)(&x);
            if self.set.insert(v) {
                return Some(x);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        // Likely true
        self.iter.size_hint()
    }
}
