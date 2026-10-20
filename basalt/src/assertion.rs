#[cfg(feature = "strict_assert")]
#[macro_export]
macro_rules! strict_assert(
    ( $( $arg:tt )* ) => {
        assert!( $( $arg )* )
    }
);
#[cfg(feature = "strict_assert")]
#[macro_export]
macro_rules! strict_assert_eq(
    ( $( $arg:tt )* ) => {
        assert_eq!( $( $arg )* )
    }
);

#[cfg(feature = "strict_assert")]
#[macro_export]
macro_rules! strict_assert_ne(
    ( $( $arg:tt )* ) => {
        assert_ne!( $( $arg )* )
    }
);

#[cfg(not(feature = "strict_assert"))]
#[macro_export]
macro_rules! strict_assert(
    ( $( $arg:tt )* ) => {
        debug_assert!( $( $arg )* )
    }
);
#[cfg(not(feature = "strict_assert"))]
#[macro_export]
macro_rules! strict_assert_eq(
    ( $( $arg:tt )* ) => {
        debug_assert_eq!( $( $arg )* )
    }
);

#[cfg(not(feature = "strict_assert"))]
#[macro_export]
macro_rules! strict_assert_ne(
    ( $( $arg:tt )* ) => {
        debug_assert_ne!( $( $arg )* )
    }
);

#[cfg(test)]
mod test {
    #[cfg(all(not(feature = "strict_assert"), debug_assertions))]
    #[test]
    #[should_panic = "assertion failed: false"]
    fn debug_assert_no_strict() {
        crate::strict_assert!(false);
    }
    #[cfg(feature = "strict_assert")]
    #[should_panic = "assertion failed: false"]
    #[test]
    fn debug_assert_strict() {
        crate::strict_assert!(false);
    }
}
