#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
    " SPDX-License-Identifier: Apache-2.0\n",
)]
#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]
#![cfg_attr(coverage, feature(coverage_attribute))]

extern crate alloc;

pub mod component;
mod mm;
