//! Boot orchestration components.
//!
//! This module provides the core boot components:
//! - [`ConsoleDiscovery`]: Discovers console devices and populates ConIn/ConOut/ErrOut variables
//! - [`BootOrchestrator`]: Orchestrates the boot flow with device enumeration, event signaling, and boot execution
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

mod boot_orchestrator;
mod console_discovery;

pub use boot_orchestrator::BootOrchestrator;
pub use console_discovery::ConsoleDiscovery;
