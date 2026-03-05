//! Boot configuration types.
//!
//! [`BootConfig`] provides the platform boot configuration used by
//! [`BootOrchestrator`](crate::BootOrchestrator) implementations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use patina::device_path::paths::DevicePathBuf;

/// Boot options provided by the platform.
///
/// Platforms configure boot behavior by providing this configuration to the
/// [`SimpleBootManager`](crate::SimpleBootManager).
///
/// ## Example
///
/// ```rust,ignore
/// use patina_boot::config::BootConfig;
///
/// let config = BootConfig::new(nvme_device_path)
///     .with_device(usb_device_path)
///     .with_hotkey(0x16) // F12 (UEFI scan code)
///     .with_hotkey_device(usb_device_path) // Boot from USB when F12 pressed
///     .with_failure_handler(|| show_error_screen());
/// ```
pub struct BootConfig {
    /// Boot device paths in priority order.
    devices: Vec<DevicePathBuf>,
    /// Optional hotkey for boot override (e.g., F12 for boot menu).
    hotkey: Option<u16>,
    /// Alternate boot device paths used when hotkey is detected.
    hotkey_devices: Vec<DevicePathBuf>,
    /// Handler called when all boot options fail.
    failure_handler: Option<Box<dyn Fn() + Send + Sync>>,
}

impl BootConfig {
    /// Create a new boot configuration with an initial boot device.
    ///
    /// At least one boot device is required. Additional devices can be added
    /// with [`with_device`](Self::with_device).
    pub fn new(device: DevicePathBuf) -> Self {
        Self { devices: alloc::vec![device], hotkey: None, hotkey_devices: Vec::new(), failure_handler: None }
    }

    /// Add a boot device path.
    ///
    /// Device paths are tried in the order they are added.
    pub fn with_device(mut self, device: DevicePathBuf) -> Self {
        self.devices.push(device);
        self
    }

    /// Add a hotkey scancode for boot override.
    ///
    /// When this hotkey is detected during boot, the orchestrator will use
    /// the alternate boot options configured via [`with_hotkey_device`](Self::with_hotkey_device)
    /// instead of the primary boot devices.
    ///
    /// Note: Hotkey detection reads and consumes all pending keystrokes from the
    /// keyboard buffer. Any keys pressed before detection will not be available
    /// to subsequent code.
    pub fn with_hotkey(mut self, scancode: u16) -> Self {
        self.hotkey = Some(scancode);
        self
    }

    /// Add an alternate boot device path used when the hotkey is detected.
    ///
    /// Hotkey devices are tried in the order they are added, but only when
    /// the configured hotkey is detected during boot.
    pub fn with_hotkey_device(mut self, device: DevicePathBuf) -> Self {
        self.hotkey_devices.push(device);
        self
    }

    /// Add a failure handler called when all boot options fail.
    pub fn with_failure_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.failure_handler = Some(Box::new(handler));
        self
    }

    /// Get the hotkey scancode, if configured.
    pub fn hotkey(&self) -> Option<u16> {
        self.hotkey
    }

    /// Returns an iterator over all configured boot device paths.
    pub fn devices(&self) -> impl Iterator<Item = &DevicePathBuf> {
        self.devices.iter()
    }

    /// Returns an iterator over alternate boot device paths used when hotkey is detected.
    pub fn hotkey_devices(&self) -> impl Iterator<Item = &DevicePathBuf> {
        self.hotkey_devices.iter()
    }

    /// Call the failure handler if configured.
    ///
    /// This is called when all boot options have been exhausted.
    pub fn handle_failure(&self) {
        if let Some(handler) = &self.failure_handler {
            handler();
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;

    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicBool, Ordering};
    use patina::device_path::node_defs::EndEntire;

    fn create_test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_new_requires_device() {
        let config = BootConfig::new(create_test_device_path());
        assert_eq!(config.devices().count(), 1);
        assert!(config.hotkey().is_none());
        assert_eq!(config.hotkey_devices().count(), 0);
    }

    #[test]
    fn test_with_additional_devices() {
        let config = BootConfig::new(create_test_device_path())
            .with_device(create_test_device_path())
            .with_device(create_test_device_path());
        assert_eq!(config.devices().count(), 3);
    }

    #[test]
    fn test_with_hotkey() {
        let config = BootConfig::new(create_test_device_path()).with_hotkey(0x16); // F12
        assert_eq!(config.hotkey(), Some(0x16));
    }

    #[test]
    fn test_with_hotkey_device() {
        let config = BootConfig::new(create_test_device_path()).with_hotkey_device(create_test_device_path());
        assert_eq!(config.hotkey_devices().count(), 1);
    }

    #[test]
    fn test_with_multiple_hotkey_devices() {
        let config = BootConfig::new(create_test_device_path())
            .with_hotkey_device(create_test_device_path())
            .with_hotkey_device(create_test_device_path());
        assert_eq!(config.hotkey_devices().count(), 2);
    }

    #[test]
    fn test_hotkey_with_hotkey_devices() {
        let config = BootConfig::new(create_test_device_path())
            .with_hotkey(0x16) // F12
            .with_hotkey_device(create_test_device_path());

        assert_eq!(config.devices().count(), 1);
        assert_eq!(config.hotkey(), Some(0x16));
        assert_eq!(config.hotkey_devices().count(), 1);
    }

    #[test]
    fn test_failure_handler_called() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let config = BootConfig::new(create_test_device_path()).with_failure_handler(move || {
            called_clone.store(true, Ordering::SeqCst);
        });

        assert!(!called.load(Ordering::SeqCst));
        config.handle_failure();
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_failure_handler_not_configured() {
        let config = BootConfig::new(create_test_device_path());
        // Should not panic when no handler is configured
        config.handle_failure();
    }

    #[test]
    fn test_devices_iterator_order() {
        let config = BootConfig::new(create_test_device_path()).with_device(create_test_device_path());
        let devices: Vec<_> = config.devices().collect();
        assert_eq!(devices.len(), 2);
    }
}
