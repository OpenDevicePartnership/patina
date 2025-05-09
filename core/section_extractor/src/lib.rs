//! # Section Extractor Implementations
//!
//! This crate provides a set of Implementations for the `mu_pi::fw_fs::SectionExtractor` trait.
//!
//! ## Features
//!
//! This crate contains the following features, where each feature corresponds to a different
//! implementation of the `SectionExtractorLib` trait. The crate is configured in this manner to
//! reduce compilation times, by only compiling the necessary implementations.
//! - `brotli`: Enables the `SectionExtractorLibBrotli` implementation.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

mod null;
pub use null::NullSectionExtractor;

#[cfg(feature = "brotli")]
mod brotli;
#[cfg(feature = "brotli")]
pub use brotli::BrotliSectionExtractor;

#[cfg(feature = "uefi_decompress")]
mod uefi_decompress;
#[cfg(feature = "uefi_decompress")]
pub use uefi_decompress::UefiDecompressSectionExtractor;

#[cfg(feature = "crc32")]
mod crc32;
#[cfg(feature = "crc32")]
pub use crc32::Crc32SectionExtractor;

mod composite;
pub use composite::CompositeSectionExtractor;
