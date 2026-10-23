//! Hob related module used by the Supervisor
//!
//! - [`lookup`] - where is a given HOB, reporting a miss as [`Option::None`]
//! - [`validation`] - is what the HOBs describe safe to act on
//! - [`pass_down`] - the MM Supervisor `PassDown` payload, and parsing it from raw bytes
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

pub(crate) mod lookup;
pub(crate) mod pass_down;
pub(crate) mod validation;
