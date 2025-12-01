#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
    " SPDX-License-Identifier: Apache-2.0\n",
)]
#![cfg_attr(all(not(feature = "std"), not(doc)), no_std)]
#![feature(coverage_attribute)]

extern crate alloc;

pub mod component;
pub mod logger;
pub mod protocol;

#[cfg(any(doc, feature = "std"))]
pub mod parser;

mod integration_test;
mod memory_log;
