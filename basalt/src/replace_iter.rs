use std::iter::Iterator;
pub struct ReplaceIterator<I: Iterator> {
    iter: I,
    at: usize,
    ind: usize,
    item: Option<<I as Iterator>::Item>,
}

impl<I: Iterator> ReplaceIterator<I> {
    const fn new(iter: I, at: usize, item: <I as Iterator>::Item) -> Self {
        let item = Some(item);
        Self {
            iter,
            at,
            item,
            ind: 0,
        }
    }
}

impl<I: Iterator> Iterator for ReplaceIterator<I> {
    type Item = <I as Iterator>::Item;
    fn next(&mut self) -> Option<Self::Item> {
        let n = self.iter.next()?;
        if self.item.is_some() {
            if self.ind == self.at {
                self.ind += 1;
                Some(unsafe { self.item.take().unwrap_unchecked() })
            } else {
                self.ind += 1;
                Some(n)
            }
        } else {
            Some(n)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

pub trait ToReplaceIterator
where
    Self: Iterator + Sized,
{
    fn replace_item_at(
        self,
        at: usize,
        item: <Self as Iterator>::Item,
    ) -> ReplaceIterator<Self>;
}

impl<I: Iterator> ToReplaceIterator for I {
    fn replace_item_at(
        self,
        at: usize,
        item: <Self as Iterator>::Item,
    ) -> ReplaceIterator<Self> {
        ReplaceIterator::new(self, at, item)
    }
}
