// #![feature(array_try_map)]
// #![feature(const_trait_impl)]
#![feature(likely_unlikely)]
// do we need this?
#![feature(negative_impls)]
#![feature(min_specialization)]
// Start of code
#![allow(dead_code)]
pub mod alignment;
pub mod assertion;
pub mod borrow_wrapper;
pub mod byte;
pub mod compose_vector;
pub mod index;
pub mod internal_state;
pub mod iter;
pub mod replace_iter;
pub mod reuse_index;
pub mod starting_fold;
pub mod thread;
pub mod undroppable;
pub mod unsafe_ref;
pub mod utils;

mod fmt_index_array;

pub use crate::{
    alignment::{Pad, PadWord},
    fmt_index_array::IndexVec,
    internal_state::InternalConstraintCheck,
    iter::{DeduplicateByValue, DeduplicateByValueExt},
    starting_fold::{InitializeThenFold, InitializeThenTryFold},
    undroppable::PhantomUnDrop,
    unsafe_ref::{UnsafeCell, UnsafeRef, UnsafeRefMut},
    utils::{constant_index_array, uninit_vec},
};
