#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
    " SPDX-License-Identifier: Apache-2.0\n",
)]
#![cfg_attr(all(not(feature = "std"), not(test), not(doc)), no_std)]
#![deny(missing_docs)]
#![cfg_attr(coverage, feature(coverage_attribute))]

#[cfg(any(feature = "alloc", doc))]
extern crate alloc;

mod memory_log;
mod writer;

pub mod logger;

#[cfg(any(doc, feature = "reader"))]
pub mod reader;

#[cfg(any(doc, feature = "component"))]
pub mod component;
#[cfg(feature = "component")]
mod integration_test;
#[cfg(any(doc, feature = "component"))]
pub mod protocol;

#[cfg(any(doc, feature = "std"))]
pub mod parser;
