//! Hello World Sample Components
//!
//! A simple component used for demonstration.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![feature(coverage_attribute)]

mod struct_component;

pub use struct_component::{GreetingsEnum, HelloStruct};
