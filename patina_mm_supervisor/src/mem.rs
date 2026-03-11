//! Memory Management
//!
//! This module contains the memory allocators used by the MM Supervisor Core:
//! - [`page_allocator`] — SMRAM page-granularity allocator for general use
//! - [`paging_allocator`] — dedicated bump allocator for page table structures
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod page_allocator;
pub mod paging_allocator;

pub use page_allocator::{AllocationType, PageAllocator};
pub use paging_allocator::{DEFAULT_PAGING_POOL_PAGES, PagingPoolAllocator, SharedPagingAllocator};
