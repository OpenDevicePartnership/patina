//! A library containing multiple `no_std` and `no_alloc` data structures where the core data
//! is stored as a slice that is provided by the caller. The currently supported data structures
//! are a [Binary Search Tree](Bst), a [Red-Black Tree](Rbt), and a [Sorted Slice](SortedSlice).
//! The sorted slice is preferred for it's size and speed when when working with either a small
//! number of elements or when the elements themselves are small. The BST and RBT are preferred
//! in all other cases, with the RBT being the preferred choice when the number of elements is
//! expected to be large.
//!
//! As mentioned above, the data structures are `no_std` and `no_alloc`, meaning they can be used
//! in environments where the standard library is not available, and where dynamic memory
//! allocation is not allowed. An `alloc` feature is available for the crate which adds a few
//! additional methods to the data structures that do require dynamic memory allocation, however
//! the core functionality of the data structures is still `no_std` and `no_alloc`.
//!
//! We use a custom `SliceKey` trait for sorting the elements in the data structures. A blanket
//! implementation is provided for all types that implement the `Ord` trait, however the user can
//! implement the trait for their own types to provide a different key for sorting, than the type
//! itself.
//!
//! ## Benchmarks
//!
//! There are currently some benchmarks available in the `benches` directory. These benchmarks
//! test the performance of the data structures with 4096 entries of 32bit, 128bit, and 384bit
//! index sizes respectively. The tests are as follows:
//!
//! - Insertion: Time to completely fill the data structure with random numbers.
//! - Search: Time it takes to search for every element in the data structure once.
//! - Delete: Time it takes to delete every element in the data structure.
//!
//! ## Examples
//!
//! ```rust
//! use patina_internal_collections::{Bst, Rbt, SortedSlice, SliceKey, node_size};
//!
//! const MAX_SIZE: usize = 4096;
//!
//! let mut mem_bst = [0; MAX_SIZE * node_size::<u32>()];
//! let mut bst: Bst<u32> = Bst::with_capacity(&mut mem_bst);
//!
//! let mut mem_rbt = [0; MAX_SIZE * node_size::<u32>()];
//! let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem_rbt);
//!
//! let mut mem_ss = [0; MAX_SIZE * core::mem::size_of::<u32>()];
//! let mut ss: SortedSlice<u32> = SortedSlice::new(&mut mem_ss);
//!
//! let nums = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
//! for num in nums {
//!     bst.add(num).unwrap();
//!     rbt.add(num).unwrap();
//!     ss.add(num).unwrap();
//! }
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![no_std]
#![feature(let_chains)]
mod bst;
mod node;
mod rbt;
mod sorted_slice;

pub use bst::Bst;
pub use node::node_size;
pub use rbt::Rbt;
pub use sorted_slice::SortedSlice;

/// Public result type for the crate.
pub type Result<T> = core::result::Result<T, Error>;

/// Public error types for the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The storage is full and cannot hold any more nodes.
    OutOfSpace,
    /// The node was not found in the storage.
    NotFound,
    /// The node already exists in the storage.
    AlreadyExists,
    /// The elements need to be sorted before adding them to the slice.
    NotSorted,
}

/// A trait to allow a type to use a different key than `self` for ordering.
pub trait SliceKey {
    /// The type used for sorting the elements in the slice.
    type Key: Ord;

    /// Returns the key.
    fn key(&self) -> &Self::Key;
}

impl<T> SliceKey for T
where
    T: Ord,
{
    type Key = Self;
    fn key(&self) -> &T {
        self
    }
}
