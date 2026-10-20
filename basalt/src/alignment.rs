#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct Pad<const N: usize = 1>([u8; N]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct PadWord<const N: usize = 1>([Pad<4>; N]);
unsafe impl<const N: usize> bytemuck::Pod for Pad<N> {}
unsafe impl<const N: usize> bytemuck::Pod for PadWord<N> {}
unsafe impl<const N: usize> bytemuck::Zeroable for Pad<N> {}
unsafe impl<const N: usize> bytemuck::Zeroable for PadWord<N> {}

impl<const N: usize> Default for Pad<N> {
    fn default() -> Self {
        Self([0; N])
    }
}
impl<const N: usize> Default for PadWord<N> {
    fn default() -> Self {
        Self([Pad::default(); N])
    }
}
pub fn align_to<
    T: Copy
        + std::ops::Add<Output = T>
        + std::ops::Sub<Output = T>
        + std::ops::Div<Output = T>
        + std::ops::Mul<Output = T>
        + PartialOrd
        + From<u8>
        + std::fmt::Debug,
>(
    a: T,
    b: T,
) -> T {
    crate::strict_assert_ne!(
        b,
        T::from(0),
        "Alignment value must be non-zero."
    );
    (a + b - T::from(1)) / b * b
}

#[cfg(test)]
mod test {
    use super::*;
    fn align_test256() {
        assert_eq!(256, align_to(249, 16));
    }
    fn align_test8() {
        assert_eq!(0, align_to(0, 8));
    }
    fn align_test8_2() {
        assert_eq!(16, align_to(9, 8));
    }
    #[should_panic = "Alignment value must be non-zero."]
    #[cfg(debug_assertions)]
    fn align_test_div0() {
        align_to(5, 0);
    }

    #[test]
    fn size_of_pad() {
        assert_eq!(std::mem::size_of::<Pad<0>>(), 0);
        assert_eq!(std::mem::size_of::<Pad<1>>(), 1);
        assert_eq!(std::mem::size_of::<Pad<2>>(), 2);
        assert_eq!(std::mem::size_of::<Pad<3>>(), 3);
    }
    #[test]
    fn size_of_padword() {
        assert_eq!(std::mem::size_of::<PadWord<0>>(), 0);
        assert_eq!(std::mem::size_of::<PadWord<1>>(), 4);
        assert_eq!(std::mem::size_of::<PadWord<2>>(), 8);
        assert_eq!(std::mem::size_of::<PadWord<3>>(), 12);
    }
}
