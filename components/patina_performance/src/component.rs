//! Patina Performance Components
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod performance;
mod protocol;

// Re-export the Performance component for easier access.
pub use performance::Performance;
// Re-export of the Measurement enum for easier access.
pub use patina::performance::Measurement;
