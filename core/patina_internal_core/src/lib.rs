//! Internal library for shared core code.
//!
//! ** Internal Use Only **
//!
//! This library is for internal use only and should not be used by external projects. The
//! APIs in this library are not stable or safe for use outside of their intended scope.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![no_std]
#![cfg_attr(coverage, feature(coverage_attribute))]

extern crate alloc;

pub mod collections;
pub mod depex;
