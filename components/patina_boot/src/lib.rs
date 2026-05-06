//! Boot Orchestration Components
//!
//! This crate provides boot orchestration for Patina firmware, implementing
//! UEFI Specification 2.11 Chapter 3 (Boot Manager) and PI Specification BDS phase requirements.
//!
//! ## Architecture
//!
//! - [`BootOrchestrator`]: A trait defining the boot flow interface. Platforms implement this
//!   trait to customize boot behavior.
//! - [`BootDispatcher`]: The Patina component that installs the BDS architectural protocol and
//!   delegates to a `BootOrchestrator` implementation when invoked by the DXE core.
//! - [`SimpleBootManager`]: A default `BootOrchestrator` implementation for platforms with
//!   straightforward boot topologies.
//!
//! ## Configuration
//!
//! - [`config::BootConfig`]: Boot configuration for `BootOrchestrator` implementations
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
#![feature(never_type)]

pub mod boot_dispatcher;
pub mod boot_orchestrator;
pub mod config;
pub mod connect_controller;
pub mod helpers;
pub mod orchestrators;
pub mod strategies;

pub use boot_dispatcher::BootDispatcher;
pub use boot_orchestrator::BootOrchestrator;
pub use connect_controller::ConnectController;
pub use orchestrators::SimpleBootManager;
pub use strategies::ConnectAllStrategy;
