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
extern crate alloc;

use patina::{
    boot_services::StandardBootServices,
    component::{
        component,
        params::{Config, Handle},
    },
    error::{EfiError, Result},
    runtime_services::StandardRuntimeServices,
};

use crate::{config::BootOptions, helpers};

/// Boot orchestrator component.
///
/// Orchestrates boot execution using boot options provided via [`Config<BootOptions>`].
/// Connects all controllers for device topology enumeration, signals BDS phase events
/// (EndOfDxe, ReadyToBoot), and executes boot options via `LoadImage()`/`StartImage()`
/// with 5-minute watchdog per UEFI Section 3.1.2.
///
/// Handles boot failures by attempting subsequent options. Never returns on success.
pub struct BootOrchestrator;

#[component]
impl BootOrchestrator {
    /// Entry point for the boot orchestrator component.
    ///
    /// # Flow
    ///
    /// 1. Connect all controllers for device enumeration
    /// 2. Signal EndOfDxe (security components perform lockdown)
    /// 3. Discover consoles
    /// 4. Check for hotkey press; if detected, use alternate boot options
    /// 5. Signal ReadyToBoot
    /// 6. Execute boot options from config (or hotkey_devices if hotkey detected)
    /// 7. If all boot options fail, call failure handler
    #[coverage(off)] // Component integration - tested via integration tests
    fn entry_point(
        self,
        boot_services: StandardBootServices,
        runtime_services: StandardRuntimeServices,
        boot_options: Config<BootOptions>,
        image_handle: Option<Handle>,
    ) -> Result<()> {
        helpers::connect_all(boot_services.as_ref())?;
        helpers::signal_bds_phase_entry(boot_services.as_ref())?;
        helpers::discover_console_devices(boot_services.as_ref(), runtime_services.as_ref())?;

        // Check for hotkey press after devices are connected and consoles discovered
        let use_hotkey_devices = if let Some(hotkey) = boot_options.hotkey() {
            helpers::detect_hotkey(boot_services.as_ref(), hotkey)
        } else {
            false
        };

        helpers::signal_ready_to_boot(boot_services.as_ref())?;

        // Per UEFI spec, the parent handle must be a valid image handle (has LoadedImage protocol).
        // The Handle must be provided by the component framework - we cannot safely guess
        // which handle is correct from the handle database as ordering is not guaranteed.
        let parent_handle = image_handle.ok_or_else(|| {
            log::error!("Handle not provided - required for LoadImage parent handle");
            EfiError::InvalidParameter
        })?;

        // Select boot devices based on hotkey detection
        let boot_devices: alloc::vec::Vec<_> = if use_hotkey_devices {
            log::info!("Using alternate boot options (hotkey detected)");
            boot_options.hotkey_devices().collect()
        } else {
            boot_options.devices().collect()
        };

        for device_path in boot_devices {
            match helpers::boot_from_device_path(boot_services.as_ref(), *parent_handle, device_path) {
                Ok(()) => {
                    // Boot image returned control (e.g., EFI application exited).
                    // Continue to try next boot option.
                    log::warn!("Boot option returned, trying next...");
                }
                Err(_) => {
                    log::warn!("Boot option failed, trying next...");
                }
            }
        }

        boot_options.handle_failure();
        log::error!("All boot options exhausted and failure handler returned");
        loop {
            core::hint::spin_loop();
        }
    }
}
