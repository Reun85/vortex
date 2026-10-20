//! Other usefull functions
//!

#[must_use]
pub const fn constant_index_array<const N: usize>() -> [usize; N] {
    let mut arr = [0; N];
    let mut i = 0;
    while i < N {
        arr[i] = i;
        i += 1;
    }
    arr
}

/// Create a `Vec` of `MaybeUninit` with the given length
/// # Safety
/// The caller must ensure that the elements are properly initialized before use
#[must_use]
pub unsafe fn uninit_vec<T>(len: usize) -> Vec<std::mem::MaybeUninit<T>> {
    let mut v = Vec::with_capacity(len);
    unsafe {
        v.set_len(len);
    }
    v
}
