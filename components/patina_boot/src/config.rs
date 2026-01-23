//! Boot configuration types.
//!
//! This module provides configuration types for boot orchestration, including
//! [`BootOptions`] which specifies platform-provided boot paths.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use patina::uefi_protocol::device_path::DevicePathBuf;

/// Boot options provided by the platform.
///
/// Platforms configure boot behavior by providing this configuration to the
/// [`BootOrchestrator`](crate::component::BootOrchestrator) component.
///
/// ## Example
///
/// ```rust,ignore
/// use patina_boot::config::BootOptions;
///
/// let options = BootOptions::new()
///     .with_device(nvme_device_path)
///     .with_device(usb_device_path)
///     .with_failure_handler(|| show_error_screen());
/// ```
#[derive(Default)]
pub struct BootOptions {
    /// Boot device paths in priority order.
    devices: Vec<DevicePathBuf>,
    /// Optional hotkey for boot override (e.g., F12 for boot menu).
    hotkey: Option<u16>,
    /// Handler called when all boot options fail.
    failure_handler: Option<Box<dyn Fn() + Send + Sync>>,
}

impl BootOptions {
    /// Create new empty boot options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a boot device path.
    ///
    /// Device paths are tried in the order they are added.
    pub fn with_device(mut self, device: DevicePathBuf) -> Self {
        self.devices.push(device);
        self
    }

    /// Add a hotkey scancode for boot override (not yet implemented).
    ///
    /// This field is reserved for future boot menu functionality.
    /// Currently, the hotkey is stored but not acted upon.
    pub fn with_hotkey(mut self, scancode: u16) -> Self {
        self.hotkey = Some(scancode);
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
    use patina::uefi_protocol::device_path::nodes::EndEntire;

    fn create_test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    #[test]
    fn test_default_boot_options() {
        let options = BootOptions::default();
        assert!(options.hotkey().is_none());
        assert_eq!(options.devices().count(), 0);
    }

    #[test]
    fn test_new_boot_options() {
        let options = BootOptions::new();
        assert_eq!(options.devices().count(), 0);
    }

    #[test]
    fn test_with_single_device() {
        let device = create_test_device_path();
        let options = BootOptions::new().with_device(device);
        assert_eq!(options.devices().count(), 1);
    }

    #[test]
    fn test_with_multiple_devices() {
        let device1 = create_test_device_path();
        let device2 = create_test_device_path();
        let device3 = create_test_device_path();
        let options = BootOptions::new().with_device(device1).with_device(device2).with_device(device3);
        assert_eq!(options.devices().count(), 3);
    }

    #[test]
    fn test_with_hotkey() {
        let options = BootOptions::new().with_hotkey(0x86); // F12
        assert_eq!(options.hotkey(), Some(0x86));
    }

    #[test]
    fn test_failure_handler_called() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let device = create_test_device_path();
        let options = BootOptions::new().with_device(device).with_failure_handler(move || {
            called_clone.store(true, Ordering::SeqCst);
        });

        assert!(!called.load(Ordering::SeqCst));
        options.handle_failure();
        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_failure_handler_not_configured() {
        let options = BootOptions::default();
        // Should not panic when no handler is configured
        options.handle_failure();
    }

    #[test]
    fn test_devices_iterator_order() {
        let device1 = create_test_device_path();
        let device2 = create_test_device_path();
        let options = BootOptions::new().with_device(device1).with_device(device2);

        let devices: Vec<_> = options.devices().collect();
        assert_eq!(devices.len(), 2);
    }
}
