//! Hello World Sample Components
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
mod function_component;
mod struct_component;

pub use function_component::{Name, log_hello};
pub use struct_component::{GreetingsEnum, HelloStruct};
