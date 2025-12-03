#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
)]
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![feature(coverage_attribute)]

pub mod component;
pub mod config;
pub mod protocol;
pub mod service;
