//! Sample Patina Components
//!
//! This crate provides example component implementations demonstrating various
//! Patina component patterns and usage models.
//!
//! ## Examples
//!
//! - [`component::hello_world::HelloStruct`]: Demonstrates a struct-based component with default entry point
//! - [`component::hello_world::GreetingsEnum`]: Demonstrates an enum-based component with custom entry point
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(not(feature = "std"), no_std)]
#![feature(coverage_attribute)]

pub mod component;
