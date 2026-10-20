use std::borrow::Borrow;
pub struct AsRefWrapper<T, Q>
where
    T: AsRef<Q>,
{
    value: T,
    _marker: std::marker::PhantomData<Q>,
}
impl<T, Q> std::ops::Deref for AsRefWrapper<T, Q>
where
    T: AsRef<Q>,
{
    type Target = Q;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref()
    }
}
impl<T, Q> From<T> for AsRefWrapper<T, Q>
where
    T: AsRef<Q>,
{
    fn from(own: T) -> Self {
        Self {
            value: own,
            _marker: std::marker::PhantomData,
        }
    }
}
impl<T, Q> AsRef<Q> for AsRefWrapper<T, Q>
where
    T: AsRef<Q>,
{
    fn as_ref(&self) -> &Q {
        self.value.as_ref()
    }
}

pub struct AsMutWrapper<T, Q>
where
    T: AsMut<Q>,
{
    value: T,
    _marker: std::marker::PhantomData<Q>,
}
impl<T, Q> std::ops::Deref for AsMutWrapper<T, Q>
where
    T: AsMut<Q> + AsRef<Q>,
{
    type Target = Q;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref()
    }
}

impl<T, Q> std::ops::DerefMut for AsMutWrapper<T, Q>
where
    T: AsMut<Q> + AsRef<Q>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut()
    }
}

impl<T, Q> From<T> for AsMutWrapper<T, Q>
where
    T: AsMut<Q>,
{
    fn from(own: T) -> Self {
        Self {
            value: own,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T, Q> AsRef<Q> for AsMutWrapper<T, Q>
where
    T: AsMut<Q> + AsRef<Q>,
{
    fn as_ref(&self) -> &Q {
        self.value.as_ref()
    }
}
impl<T, Q> AsMut<Q> for AsMutWrapper<T, Q>
where
    T: AsMut<Q>,
{
    fn as_mut(&mut self) -> &mut Q {
        self.value.as_mut()
    }
}

pub struct BorrowWrapper<T, Q>
where
    T: Borrow<Q>,
{
    value: T,
    _marker: std::marker::PhantomData<Q>,
}
impl<T, Q> std::ops::Deref for BorrowWrapper<T, Q>
where
    T: Borrow<Q>,
{
    type Target = Q;

    fn deref(&self) -> &Self::Target {
        self.value.borrow()
    }
}
impl<T, Q> From<T> for BorrowWrapper<T, Q>
where
    T: Borrow<Q>,
{
    fn from(own: T) -> Self {
        Self {
            value: own,
            _marker: std::marker::PhantomData,
        }
    }
}
impl<T, Q> Borrow<Q> for BorrowWrapper<T, Q>
where
    T: Borrow<Q>,
{
    fn borrow(&self) -> &Q {
        self.value.borrow()
    }
}
