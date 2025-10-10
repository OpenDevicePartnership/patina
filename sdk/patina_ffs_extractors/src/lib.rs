//! # Section Extractor Implementations
//!
//! This crate provides a set of Implementations for the `patina_pi::fw_fs::SectionExtractor` trait.
//!
//! ## Features
//!
//! This crate contains the following features, where each feature corresponds to a different
//! implementation of the `SectionExtractorLib` trait. The crate is configured in this manner to
//! reduce compilation times, by only compiling the necessary implementations.
//! - `brotli`: Enables the `SectionExtractorLibBrotli` implementation.
//! - `crc32`: Enables the `Crc32SectionExtractor` implementation to validate CRC32 GUID-defined
//!   sections and return the verified payload.
//! - `lzma`: Enables the `LzmaSectionExtractor` implementation for GUID-defined LZMA compressed
//!   sections.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;

#[cfg(feature = "brotli")]
mod brotli;
#[cfg(feature = "brotli")]
pub use brotli::BrotliSectionExtractor;

#[cfg(feature = "crc32")]
mod crc32;
#[cfg(feature = "crc32")]
pub use crc32::Crc32SectionExtractor;

#[cfg(feature = "lzma")]
mod lzma;
#[cfg(feature = "lzma")]
pub use lzma::LzmaSectionExtractor;

mod composite;
pub use composite::CompositeSectionExtractor;
