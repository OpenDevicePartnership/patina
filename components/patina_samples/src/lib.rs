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
mod function_component;
mod struct_component;

pub use function_component::{Name, log_hello};
pub use struct_component::{GreetingsEnum, HelloStruct};
