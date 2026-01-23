//! Console discovery component.
//!
//! Locates GOP and SimpleTextInput protocol handles, creates device paths,
//! and writes ConIn/ConOut/ErrOut variables.
//!
//! ## Rationale
//!
//! Console setup is a component because platforms may need custom console configuration
//! (serial-only, specific GOP preference, headless operation). As a component, platforms
//! can replace it entirely without modifying orchestration code.
//!
//! ## Prerequisites
//!
//! Device controllers exposing GOP and input protocols must be connected before
//! this component runs.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    boot_services::StandardBootServices, component::component, error::Result, runtime_services::StandardRuntimeServices,
};

use crate::helpers;

/// Console discovery component.
///
/// Scans for GOP and SimpleTextInput protocol handles, creates device paths,
/// and writes `ConIn`/`ConOut`/`ErrOut` UEFI variables.
pub struct ConsoleDiscovery;

#[component]
impl ConsoleDiscovery {
    /// Entry point for the console discovery component.
    #[coverage(off)] // Component integration - tested via integration tests
    fn entry_point(self, boot_services: StandardBootServices, runtime_services: StandardRuntimeServices) -> Result<()> {
        helpers::discover_console_devices(boot_services.as_ref(), runtime_services.as_ref())
    }
}
