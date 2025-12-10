//! Boot orchestrator component.
//!
//! Orchestrates the boot flow with device enumeration, event signaling, and boot execution.
//!
//! ## Rationale
//!
//! Boot orchestration is a component that represents the primary platform customization
//! point. Platforms provide boot options as configuration data, allowing different boot policies
//! without changing orchestration logic. Platforms needing entirely different boot flows can
//! replace the component.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use patina::{
    boot_services::StandardBootServices,
    component::{component, params::Config},
    error::Result,
    runtime_services::StandardRuntimeServices,
};

use crate::{config::BootOptions, library};

/// Boot orchestrator component.
///
/// Orchestrates boot execution using boot options provided via [`Config<BootOptions>`].
/// Performs connect-dispatch interleaving for device topology enumeration, signals BDS
/// phase events (EndOfDxe, ReadyToBoot), and executes boot options via
/// `LoadImage()`/`StartImage()` with 5-minute watchdog per UEFI Section 3.1.2.
///
/// Handles boot failures by attempting subsequent options. Never returns on success.
pub struct BootOrchestrator;

#[component]
impl BootOrchestrator {
    /// Entry point for the boot orchestrator component.
    ///
    /// # Flow
    ///
    /// 1. Perform connect-dispatch interleaving for device enumeration
    /// 2. Signal EndOfDxe (security components perform lockdown)
    /// 3. Discover consoles
    /// 4. Signal ReadyToBoot
    /// 5. Execute boot options from config
    /// 6. If all boot options fail, call failure handler
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        boot_options: Config<BootOptions>,
    ) -> Result<()> {
        // Interleave connection and dispatch for device enumeration
        library::interleave_connect_and_dispatch(boot_services.as_ref())?;

        // Signal EndOfDxe - security components perform lockdown
        library::signal_bds_phase_entry(boot_services.as_ref())?;

        // Discover consoles
        library::discover_console_devices(boot_services.as_ref(), runtime_services.as_ref())?;

        // Signal ReadyToBoot
        library::signal_ready_to_boot(boot_services.as_ref())?;

        // Execute boot options from config
        for device_path in boot_options.devices() {
            if library::boot_from_device_path(boot_services.as_ref(), device_path).is_ok() {
                // Boot succeeded - this should never return
                unreachable!("Boot succeeded but returned");
            }
            log::warn!("Boot option failed, trying next...");
        }

        // All options failed
        boot_options.handle_failure();

        // If failure handler returns, we have no recovery path
        log::error!("All boot options exhausted and failure handler returned");
        loop {
            // Halt - no valid boot target
            core::hint::spin_loop();
        }
    }
}
