//! Simple boot manager implementation.
//!
//! [`SimpleBootManager`] implements the [`BootOrchestrator`](crate::BootOrchestrator)
//! trait for platforms with straightforward boot topologies. It supports:
//!
//! - Flexible multi-device boot via [`BootConfig`](crate::config::BootConfig)
//! - Optional hotkey detection for alternate boot paths
//! - Configurable failure handler
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::vec::Vec;

use patina::{
    boot_services::StandardBootServices, device_path::paths::DevicePathBuf, error::EfiError,
    runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

use crate::{boot_orchestrator::BootOrchestrator, config::BootConfig, helpers};

/// Simple boot manager implementing [`BootOrchestrator`].
///
/// Provides a default boot flow suitable for platforms with straightforward
/// boot topologies.
///
/// ## Boot Flow
///
/// 1. Connect all controllers for device enumeration
/// 2. Signal EndOfDxe (security lockdown)
/// 3. Discover console devices
/// 4. Detect hotkey (if configured); select alternate devices if pressed
/// 5. Signal ReadyToBoot
/// 6. Iterate boot devices, attempt `LoadImage()`/`StartImage()` for each
/// 7. Call failure handler if all options exhausted
pub struct SimpleBootManager {
    config: BootConfig,
}

impl SimpleBootManager {
    /// Create a `SimpleBootManager` from a boot configuration.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// use patina_boot::{SimpleBootManager, config::BootConfig};
    ///
    /// let manager = SimpleBootManager::new(
    ///     BootConfig::new(nvme_esp_path())
    ///         .with_device(nvme_recovery_path())
    ///         .with_hotkey(0x16)
    ///         .with_hotkey_device(usb_device_path())
    ///         .with_failure_handler(|| show_error_screen("Boot failed")),
    /// );
    /// ```
    pub fn new(config: BootConfig) -> Self {
        Self { config }
    }
}

// Expose config for test assertions
#[cfg(test)]
impl SimpleBootManager {
    pub(crate) fn config(&self) -> &BootConfig {
        &self.config
    }
}

impl BootOrchestrator for SimpleBootManager {
    #[coverage(off)] // Integration point — delegates to helper functions which are individually tested
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        if let Err(e) = helpers::connect_all(boot_services) {
            log::error!("connect_all failed: {:?}", e);
        }

        if let Err(e) = helpers::signal_bds_phase_entry(boot_services) {
            log::error!("signal_bds_phase_entry failed: {:?}", e);
        }

        if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
            log::error!("discover_console_devices failed: {:?}", e);
        }

        // Check for hotkey press after devices are connected and consoles discovered
        let use_hotkey_devices =
            if let Some(hotkey) = self.config.hotkey() { helpers::detect_hotkey(boot_services, hotkey) } else { false };

        if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
            log::error!("signal_ready_to_boot failed: {:?}", e);
        }

        // Select boot devices based on hotkey detection
        let boot_devices: Vec<&DevicePathBuf> = if use_hotkey_devices {
            log::info!("Using alternate boot options (hotkey detected)");
            self.config.hotkey_devices().collect()
        } else {
            self.config.devices().collect()
        };

        for device_path in boot_devices {
            match helpers::boot_from_device_path(boot_services, image_handle, device_path) {
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

        self.config.handle_failure();
        log::error!("All boot options exhausted");
        Err(EfiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use patina::device_path::{node_defs::EndEntire, paths::DevicePathBuf};

    fn test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_new() {
        let config = BootConfig::new(test_device_path()).with_hotkey(0x16).with_hotkey_device(test_device_path());
        let manager = SimpleBootManager::new(config);
        assert_eq!(manager.config().hotkey(), Some(0x16));
        assert_eq!(manager.config().devices().count(), 1);
        assert_eq!(manager.config().hotkey_devices().count(), 1);
    }

    #[test]
    fn test_with_hotkey() {
        let config = BootConfig::new(test_device_path())
            .with_device(test_device_path())
            .with_hotkey(0x16)
            .with_hotkey_device(test_device_path());
        let manager = SimpleBootManager::new(config);
        assert_eq!(manager.config().hotkey(), Some(0x16));
        assert_eq!(manager.config().devices().count(), 2);
        assert_eq!(manager.config().hotkey_devices().count(), 1);
    }

    #[test]
    fn test_without_hotkey() {
        let config = BootConfig::new(test_device_path()).with_device(test_device_path());
        let manager = SimpleBootManager::new(config);
        assert!(manager.config().hotkey().is_none());
        assert_eq!(manager.config().devices().count(), 2);
        assert_eq!(manager.config().hotkey_devices().count(), 0);
    }

    #[test]
    fn test_with_failure_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let config = BootConfig::new(test_device_path()).with_failure_handler(move || {
            called_clone.store(true, Ordering::SeqCst);
        });
        let manager = SimpleBootManager::new(config);

        assert!(!called.load(Ordering::SeqCst));
        manager.config().handle_failure();
        assert!(called.load(Ordering::SeqCst));
    }
}
