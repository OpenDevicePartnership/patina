//! SMBIOS Support
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]

mod component;
pub mod smbios;

pub use component::SmbiosProviderManager;
