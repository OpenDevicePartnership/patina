//! Boot Orchestration Components
//!
//! This crate provides boot orchestration components for Patina firmware, implementing
//! UEFI Specification 2.11 Chapter 3 (Boot Manager) and PI Specification BDS phase requirements.
//!
//! ## Components
//!
//! - [`component::ConsoleDiscovery`]: Discovers console devices and populates ConIn/ConOut/ErrOut variables
//! - [`component::BootOrchestrator`]: Orchestrates the boot flow with device enumeration, event signaling, and boot execution
//!
//! ## Configuration
//!
//! - [`config::BootOptions`]: Platform-provided boot options as device paths
//!
//! ## Helper Functions
//!
//! The [`helpers`] module provides helper functions for platforms implementing custom boot flows.
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
pub mod config;
pub mod helpers;
