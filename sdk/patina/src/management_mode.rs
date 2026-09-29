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
pub mod protocol;

// Re-export commonly used items for easier access
pub use comm_buffer_hob::MmCommBufferStatus;
