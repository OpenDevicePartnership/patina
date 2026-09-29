#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
)]
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![deny(missing_docs)]
#![cfg_attr(coverage, feature(coverage_attribute))]

extern crate alloc;

pub mod component;
pub mod config;
pub mod service;
