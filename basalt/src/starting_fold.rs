pub trait InitializeThenFold<Starter, Fold, Output>
where
    Self: Iterator,
    Starter: FnOnce(<Self as Iterator>::Item) -> Output,
    Fold: FnMut(Output, <Self as Iterator>::Item) -> Output,
{
    /// Return `None` if the iterator is empty, otherwise apply `starter` to the first item
    /// and then apply `fold` to the result and each subsequent item in the iterator.
    /// Equivalent to
    /// ```rs
    /// fn initialize_then_fold(mut self, starter: S, fold: F) -> Option<O> {
    /// let first = self.next()?;
    /// let acc = starter(first);
    /// Some(self.fold(acc, fold))
    /// }
    /// ```
    fn initialize_then_fold(
        self,
        starter: Starter,
        fold: Fold,
    ) -> Option<Output>;
}

impl<S, F, O, T> InitializeThenFold<S, F, O> for T
where
    T: Iterator,
    S: FnOnce(<Self as Iterator>::Item) -> O,
    F: FnMut(O, <Self as Iterator>::Item) -> O,
{
    fn initialize_then_fold(mut self, starter: S, fold: F) -> Option<O> {
        let first = self.next()?;
        let acc = starter(first);
        Some(self.fold(acc, fold))
    }
}
pub trait InitializeThenTryFold<Starter, Fold, Output, Err>
where
    Self: Iterator,
    Starter: FnOnce(<Self as Iterator>::Item) -> Result<Output, Err>,
    Fold: FnMut(Output, <Self as Iterator>::Item) -> Result<Output, Err>,
{
    /// Return `None` if the iterator is empty, otherwise apply `starter` to the first item
    /// and then apply `try_fold` to the result and each subsequent item in the iterator.
    fn initialize_then_try_fold(
        self,
        starter: Starter,
        fold: Fold,
    ) -> Option<Result<Output, Err>>;
}

impl<S, F, O, T, Err> InitializeThenTryFold<S, F, O, Err> for T
where
    T: Iterator,
    S: FnOnce(<Self as Iterator>::Item) -> Result<O, Err>,
    F: FnMut(O, <Self as Iterator>::Item) -> Result<O, Err>,
{
    fn initialize_then_try_fold(
        mut self,
        starter: S,
        fold: F,
    ) -> Option<Result<O, Err>> {
        let first = self.next()?;
        Some(starter(first).and_then(|first| self.try_fold(first, fold)))
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn initialize_then_fold() {
        use super::InitializeThenFold;
        let v = vec![10, 2, 3, 4];
        let res = v.into_iter().initialize_then_fold(|x| x, |acc, x| acc - x);
        assert_eq!(res, Some(10 - 2 - 3 - 4));
        let empty: Vec<i32> = vec![];
        let res = empty
            .into_iter()
            .initialize_then_fold(|x| x, |acc, x| acc - x);
        assert_eq!(res, None);
    }
    #[test]
    fn initialize_then_try_fold() {
        use super::InitializeThenTryFold;
        let v = vec![10, 2, 3, 4];
        let res = v.into_iter().initialize_then_try_fold(
            |first| {
                if first > 5 {
                    Ok(first)
                } else {
                    Err("too small")
                }
            },
            |acc, x| {
                if acc > 1 {
                    Ok(acc - x)
                } else {
                    Err("too small")
                }
            },
        );
        assert_eq!(res, Some(Ok(10 - 2 - 3 - 4)));
        let v = vec![4, 2, 2, 4];
        let res = v.into_iter().initialize_then_try_fold(
            |first| {
                if first > 5 {
                    Ok(first)
                } else {
                    Err("too small")
                }
            },
            |acc, x| {
                if acc > 1 {
                    Ok(acc - x)
                } else {
                    Err("too small")
                }
            },
        );
        assert_eq!(res, Some(Err("too small")));
        let empty: Vec<i32> = vec![];
        let res = empty.into_iter().initialize_then_try_fold(
            |first| {
                if first > 5 {
                    Ok(first)
                } else {
                    Err("too small")
                }
            },
            |acc, x| if x > 1 { Ok(acc - x) } else { Err("too small") },
        );
        assert_eq!(res, None);
    }
}
