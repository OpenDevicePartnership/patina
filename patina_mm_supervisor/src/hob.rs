//! HOB Handling for the MM Supervisor
//!
//! - [`validation`] - checks that what the HOBs describe is safe to act on
//! - [`pass_down`] - the MM Supervisor `PassDown` payload, and parsing it from raw bytes
//! - [`protected_region`] - the payload of the protected-region HOB
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod pass_down;
pub(crate) mod protected_region;
pub(crate) mod validation;
