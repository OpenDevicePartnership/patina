//! Common Test Infrastructure for Patina MM Integration Tests
//!
//! This module provides shared test infrastructure, constants, and utilities
//! used across all MM integration tests.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod constants;
pub mod framework;
pub mod handlers;
pub mod message_parser;
pub mod real_component_framework;

// Re-export commonly used items for test infrastructure
pub use {constants::*, framework::*, handlers::*, message_parser::*, real_component_framework::*};
