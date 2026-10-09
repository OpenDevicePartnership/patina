//! Hand-Off Blocks Handed Down by the MM IPL
//!
//! Everything the supervisor does with the incoming HOB list, split by what each part
//! answers:
//!
//! - [`lookup`] - where is a given HOB, reporting a miss as [`Option::None`]
//! - [`validation`] - is what the HOBs describe safe to act on
//! - [`pass_down`] - the MM Supervisor `PassDown` payload, and parsing it from raw bytes
//!
//! The producer sits outside the supervisor's trust boundary, so a lookup never implies the
//! payload it finds is trustworthy. [`validation`] is what decides that, and it runs before
//! any HOB content is consumed.
//!
//! Not to be confused with [`patina::pi::hob`], which defines the HOB types themselves. This
//! module only consumes them.
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
