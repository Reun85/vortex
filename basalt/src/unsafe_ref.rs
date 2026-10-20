//! Allows for interior mutability without runtime checks.
//! It is only intended to be used in situations where the borrow checking rules can be guaranteed
//! by the user.
#![allow(dead_code)]

#[cfg(debug)]
type UnsafeCellInner<T> = std::cell::RefCell<T>;
#[cfg(not(debug))]
type UnsafeCellInner<T> = std::cell::UnsafeCell<T>;
/// Misuse of this struct can lead to undefined behaviour
/// # Safety
/// For safety reason in debug builds, this struct is a wrapper around
/// [`RefCell`](`std::cell::RefCell`) to catch borrow errors in debug builds.
/// In distribution builds, the runtime borrow checker is removed by being a simple wrapper over
/// [`UnsafeCell`](`std::cell::UnsafeCell`).
pub struct UnsafeCell<T: ?Sized> {
    pub(crate) inner: UnsafeCellInner<T>,
}

#[cfg(debug)]
type UnsafeRefInner<'a, T> = std::cell::Ref<'a, T>;

#[cfg(not(debug))]
type UnsafeRefInner<'a, T> = &'a T;
pub struct UnsafeRef<'a, T: ?Sized> {
    inner: UnsafeRefInner<'a, T>,
}
impl<T> std::ops::Deref for UnsafeRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        #[cfg(not(debug))]
        {
            self.inner
        }
        #[cfg(debug)]
        {
            self.inner.deref()
        }
    }
}

#[cfg(debug)]
type UnsafeRefMutInner<'a, T> = std::cell::RefMut<'a, T>;

#[cfg(not(debug))]
type UnsafeRefMutInner<'a, T> = &'a mut T;

pub struct UnsafeRefMut<'a, T: ?Sized> {
    inner: UnsafeRefMutInner<'a, T>,
}

impl<T> std::ops::Deref for UnsafeRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        #[cfg(not(debug))]
        {
            self.inner
        }
        #[cfg(debug)]
        {
            self.inner.deref()
        }
    }
}

impl<T> std::ops::DerefMut for UnsafeRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        #[cfg(not(debug))]
        {
            self.inner
        }
        #[cfg(debug)]
        {
            self.inner.deref_mut()
        }
    }
}

impl<T: ?Sized> UnsafeCell<T> {
    const unsafe fn get(&self) -> UnsafeRef<'_, T> {
        #[cfg(not(debug))]
        {
            let value =
                unsafe { std::ptr::NonNull::new_unchecked(self.inner.get()) };
            UnsafeRef {
                inner: unsafe { value.as_ref() },
            }
        }
        #[cfg(debug)]
        {
            UnsafeRef {
                inner: self.inner.borrow(),
            }
        }
    }
    const unsafe fn get_mut(&self) -> UnsafeRefMut<'_, T> {
        #[cfg(not(debug))]
        {
            let mut value =
                unsafe { std::ptr::NonNull::new_unchecked(self.inner.get()) };

            UnsafeRefMut {
                inner: unsafe { value.as_mut() },
            }
        }
        #[cfg(debug)]
        {
            UnsafeRefMut {
                inner: self.inner.borrow_mut(),
            }
        }
    }
}
impl<T: Sized> UnsafeCell<T> {
    unsafe fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<T> UnsafeCell<T> {
    const fn new(value: T) -> Self {
        Self {
            inner: UnsafeCellInner::new(value),
        }
    }
}

impl<T> From<T> for UnsafeCell<T> {
    fn from(t: T) -> Self {
        Self::new(t)
    }
}

impl<T: ?Sized> !Sync for UnsafeCell<T> {}

impl<T: Default> Default for UnsafeCell<T> {
    fn default() -> Self {
        Self {
            inner: UnsafeCellInner::default(),
        }
    }
}

#[cfg(debug)]

impl<T: Clone> Clone for UnsafeCell<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
#[cfg(debug)]
impl<T: PartialEq> PartialEq for UnsafeCell<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
#[cfg(debug)]
impl<T: Eq> Eq for UnsafeCell<T> {}
#[cfg(debug)]
impl<T: PartialOrd> PartialOrd for UnsafeCell<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.inner.partial_cmp(&other.inner)
    }
}
#[cfg(debug)]
impl<T: Ord> Ord for UnsafeCell<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.inner.cmp(&other.inner)
    }
}
#[cfg(debug)]
impl<T: std::fmt::Debug> std::fmt::Debug for UnsafeCell<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(test)]
mod test {

    use super::*;
    #[test]
    fn multiple_unmutable_refs() {
        let x = UnsafeCell::new(5);
        let y = unsafe { x.get() };
        let z = unsafe { x.get() };
        assert_eq!(*y, 5);
        assert_eq!(*z, 5);
    }

    #[test]
    fn single_unmutable() {
        let x = UnsafeCell::new(5);
        let mut y = unsafe { x.get_mut() };
        assert_eq!(*y, 5);
        *y = 10;
        #[expect(clippy::drop_non_drop)]
        drop(y);
        assert_eq!(*unsafe { x.get() }, 10);
    }

    #[test]
    #[cfg(debug)]
    #[should_panic]
    fn broken() {
        let x = UnsafeCell::new(5);
        let _z = unsafe { x.get() };
        let mut _y = unsafe { x.get_mut() };
    }
    #[test]
    #[cfg(not(debug))]
    fn broken() {
        let x = UnsafeCell::new(5);
        let _z = unsafe { x.get() };
        let mut _y = unsafe { x.get_mut() };
    }
}
