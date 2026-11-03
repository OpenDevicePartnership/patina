//! Management Mode (MM) definitions for Patina.
//!
//! This module provides the Management Mode (MM) related definitions for Patina.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod comm_buffer_hob;
pub mod event;
pub mod guid;
#[cfg(any(test, feature = "alloc"))]
pub mod mm_services;
pub mod protocol;
pub mod supervisor;

// Re-export commonly used items for easier access
pub use comm_buffer_hob::MmCommBufferStatus;
