//! Patina MM Integration Tests
//!
//! Tests Patina MM flows in the `patina_mm` crate.
//!
//! ## Logging
//!
//! The `env_logger` crate can be used to enable logging during tests.
//!
//! To enable logging, set the `RUST_LOG` environment variable to the desired
//! log level (e.g., `debug`, `info`, `warn`, `error`) before running the tests.
//!
//! For example, to enable debug logging, run:
//!
//! ```sh
//! RUST_LOG=debug cargo make test -p patina_mm --test <test_name>
//! ```
//!
//! Powershell examples:
//!
//! ```powershell```
//! # Run all Patina MM tests with debug logging
//! $env:RUST_LOG="debug"; cargo make test --package patina_mm
//!
//! # Run the main integration test suite
//! $env:RUST_LOG="debug"; cargo make test --package patina_mm --test patina_mm_integration
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// Common utilities available to all test modules
mod common;

// Test module groups
mod framework;
mod mm_communicator;
mod mm_supervisor;
