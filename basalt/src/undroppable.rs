struct Helper<const DROPPABLE: bool>;
impl<const DROPPABLE: bool> Drop for Helper<DROPPABLE> {
    fn drop(&mut self) {
        struct ConstBlock<const D: bool>;
        impl<const D: bool> ConstBlock<D> {
            const BLOCK: () = {
                assert!(
                    D,
                    "type containing `PhantomUnDrop` cannot be simply dropped."
                );
            };
        }
        // This is here so that the panic happens at compile time.
        let _: () = ConstBlock::<DROPPABLE>::BLOCK;
    }
}
// A marker type that cannot be dropped. `free` must be called on it.
pub struct PhantomUnDrop {
    helper: Helper<false>,
}

impl PhantomUnDrop {
    pub fn free(self) {
        let Self { helper } = self;
        let droppable: Helper<true> = unsafe { std::mem::transmute(helper) };
        std::mem::drop(droppable);
    }
}
#[expect(non_upper_case_globals)]
pub const PhantomUnDrop: PhantomUnDrop = PhantomUnDrop { helper: Helper };
