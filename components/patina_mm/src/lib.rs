#![doc = include_str!("../README.md")]
#![doc = concat!(
    "## License\n\n",
    " Copyright (c) Microsoft Corporation.\n\n",
)]
#![cfg_attr(all(not(feature = "std"), not(test), not(feature = "mockall")), no_std)]
#![feature(coverage_attribute)]

#[cfg(any(test, feature = "alloc"))]
pub mod component;
#[cfg(any(test, feature = "alloc"))]
pub mod config;
#[cfg(any(test, feature = "alloc"))]
pub mod service;

pub mod comm_buffer_hob;
pub mod protocol;
