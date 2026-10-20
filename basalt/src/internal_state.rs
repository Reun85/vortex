/// A marker trait indiciating that there is an easy way to check that the internal constraints of a type are satisfised.
pub trait InternalConstraintCheck {
    type Error;
    /// Returns which internal constraint was violated, if any.
    ///
    /// This function is **NOT allowed** to treat lock/borrow contention as an internal state violation.
    /// Always use a non blocking/panicking method for testing `RefCells`/`Locks`.
    /// Use `get_internal_constraints_error` if you need to avoid locking.
    /// # Errors
    /// Return which internal constraint was violated.
    fn check_internal_constraints(&self) -> Result<(), Self::Error>;
    /// # Safety
    /// It is up to the caller to ensure that the internal constraints are satisfied.
    unsafe fn assert_internal_constraints_unchecked(&self) {
        unsafe {
            std::hint::assert_unchecked(
                self.is_internal_constraints_satisfied(),
            );
        };
    }
    fn assert_internal_constraints(&self) {
        assert!(self.is_internal_constraints_satisfied());
    }

    /// Returns true if all internal constraints are satisfied.
    fn is_internal_constraints_satisfied(&self) -> bool {
        matches!(self.check_internal_constraints(), Ok(()))
    }
}

impl InternalConstraintCheck for () {
    type Error = ();
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<T> InternalConstraintCheck for Option<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        match self.as_ref() {
            Some(inner) => inner.check_internal_constraints(),
            None => Ok(()),
        }
    }
}

impl<T> InternalConstraintCheck for Vec<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.iter()
            .try_for_each(InternalConstraintCheck::check_internal_constraints)
    }
}

impl<T, const N: usize> InternalConstraintCheck for [T; N]
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.iter()
            .try_for_each(InternalConstraintCheck::check_internal_constraints)
    }
}
impl<T> InternalConstraintCheck for &[T]
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.iter()
            .try_for_each(InternalConstraintCheck::check_internal_constraints)
    }
}

impl<T> InternalConstraintCheck for Box<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.as_ref().check_internal_constraints()
    }
}
impl<T> InternalConstraintCheck for std::rc::Rc<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.as_ref().check_internal_constraints()
    }
}

impl<T> InternalConstraintCheck for std::sync::Arc<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        self.as_ref().check_internal_constraints()
    }
}

impl<T> InternalConstraintCheck for std::cell::RefCell<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        match self.try_borrow() {
            Ok(borrowed) => borrowed.check_internal_constraints(),
            Err(_) => Ok(()),
        }
    }
}

impl<T> InternalConstraintCheck for std::sync::Mutex<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        match self.try_lock() {
            Ok(locked) => locked.check_internal_constraints(),
            Err(_) => Ok(()),
        }
    }
}

impl<T> InternalConstraintCheck for std::sync::RwLock<T>
where
    T: InternalConstraintCheck,
{
    type Error = T::Error;
    fn check_internal_constraints(&self) -> Result<(), Self::Error> {
        match self.try_read() {
            Ok(locked) => locked.check_internal_constraints(),
            Err(_) => Ok(()),
        }
    }
}

// Think about this
// impl<T, E> InternalConstraintCheck for Result<T, E>
// where
//     T: InternalConstraintCheck,
// {
//     type Error = T::Error;
//     fn check_internal_constraints(&self) -> Result<(), Self::Error> {
//         match self {
//             Ok(inner) => inner.check_internal_constraints(),
//             Err(_) => Ok(()),
//         }
//     }
// }
