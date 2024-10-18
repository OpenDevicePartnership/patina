//! Hello World Sample Component
//!
//! A simple component used for demonstration.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(not(feature = "std"), no_std)]
mod hello;

pub use hello::HelloComponent;
