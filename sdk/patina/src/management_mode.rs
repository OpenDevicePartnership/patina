//! Management Mode (MM) SDK for Patina
//!
//! This crate provides the Management Mode (MM) related definitions for Patina.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub mod comm_buffer_hob;
pub mod protocol;

// Re-export commonly used items for easier access
pub use comm_buffer_hob::MmCommBufferStatus;
