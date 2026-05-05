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
    boot_services::{BootServices, StandardBootServices, protocol_handler::HandleSearchType},
    device_path::paths::DevicePathBuf,
    error::EfiError,
    runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

use patina::component::service::dxe_dispatch::DxeDispatch;

use crate::{
    boot_orchestrator::BootOrchestrator, config::BootConfig, connect_controller::ConnectController, helpers,
    strategies::ConnectAllStrategy,
};

/// Interleave controller connection with DXE driver dispatch.
///
/// Alternates between the given connect function and dispatching newly loaded
/// drivers (e.g., PCI option ROM drivers) until the total system handle count
/// stops growing across a connect+dispatch round.
///
/// This ensures that drivers loaded from firmware volumes during device
/// enumeration (such as PCI option ROM drivers) are dispatched before
/// continuing enumeration, allowing those drivers to bind to newly
/// discovered controllers.
fn interleave_connect_and_dispatch<B: BootServices, D: DxeDispatch + ?Sized>(
    connect_fn: impl Fn(&B) -> patina::error::Result<()>,
    boot_services: &B,
    dxe_services: &D,
) -> patina::error::Result<()> {
    let mut prev_handle_count = total_handle_count(boot_services)?;

    loop {
        connect_fn(boot_services)?;
        dxe_services.dispatch()?;

        let curr_handle_count = total_handle_count(boot_services)?;
        if curr_handle_count == prev_handle_count {
            return Ok(());
        }
        prev_handle_count = curr_handle_count;
    }
}

fn total_handle_count<B: BootServices>(boot_services: &B) -> patina::error::Result<usize> {
    boot_services.locate_handle_buffer(HandleSearchType::AllHandle).map(|h| h.len()).map_err(EfiError::from)
}

/// Simple boot manager implementing [`BootOrchestrator`].
///
/// Provides a default boot flow suitable for platforms with straightforward
/// boot topologies.
///
/// ## Boot Flow
///
/// 1. Interleave controller connection with DXE driver dispatch for device enumeration
/// 2. Signal EndOfDxe (security lockdown)
/// 3. Discover console devices
/// 4. Detect hotkey (if configured); select alternate devices if pressed
/// 5. Signal ReadyToBoot
/// 6. Iterate boot devices, attempt `LoadImage()`/`StartImage()` for each
/// 7. Call failure handler if all options exhausted
pub struct SimpleBootManager<C: ConnectController = ConnectAllStrategy> {
    config: BootConfig,
    connect_strategy: C,
}

impl SimpleBootManager<ConnectAllStrategy> {
    /// Create a `SimpleBootManager` from a boot configuration.
    ///
    /// Uses [`ConnectAllStrategy`] by default. To customize which controllers
    /// are connected during device enumeration, use
    /// [`with_connect_strategy()`](Self::with_connect_strategy).
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
        Self { config, connect_strategy: ConnectAllStrategy }
    }
}

impl<C: ConnectController> SimpleBootManager<C> {
    /// Create a `SimpleBootManager` with a custom connection strategy.
    ///
    /// ## Example
    ///
    /// ```rust,ignore
    /// use patina_boot::{ConnectController, SimpleBootManager, config::BootConfig};
    /// use patina::{boot_services::BootServices, error::Result};
    ///
    /// struct MyPlatformConnect;
    /// impl<B: BootServices> ConnectController<B> for MyPlatformConnect {
    ///     fn connect(&self, bs: &B) -> Result<()> { /* sequence-specific connect */ Ok(()) }
    /// }
    ///
    /// let manager = SimpleBootManager::with_connect_strategy(
    ///     BootConfig::new(nvme_esp_path()),
    ///     MyPlatformConnect,
    /// );
    /// ```
    pub fn with_connect_strategy(config: BootConfig, strategy: C) -> Self {
        Self { config, connect_strategy: strategy }
    }
}

// Expose config for test assertions
#[cfg(test)]
impl<C: ConnectController> SimpleBootManager<C> {
    pub(crate) fn config(&self) -> &BootConfig {
        &self.config
    }
}

impl<C: ConnectController> BootOrchestrator for SimpleBootManager<C> {
    #[coverage(off)] // Integration point — delegates to helper functions which are individually tested
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_dispatch: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        if let Err(e) =
            interleave_connect_and_dispatch(|bs| self.connect_strategy.connect(bs), boot_services, dxe_dispatch)
        {
            log::error!("interleave_connect_and_dispatch failed: {:?}", e);
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

        // Select boot devices based on hotkey detection
        let boot_devices: Vec<&DevicePathBuf> = if use_hotkey_devices {
            log::info!("Using alternate boot options (hotkey detected)");
            self.config.hotkey_devices().collect()
        } else {
            self.config.devices().collect()
        };

        for device_path in boot_devices {
            // Signal ReadyToBoot before each boot attempt per UEFI 2.11 §3
            if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
                log::error!("signal_ready_to_boot failed: {:?}", e);
            }

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
    use alloc::{boxed::Box, sync::Arc};
    use core::sync::atomic::{AtomicBool, Ordering};
    use patina::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };

    fn test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

    // Tests for interleave_connect_and_dispatch

    struct MockDxeDispatcher {
        results: spin::Mutex<alloc::collections::VecDeque<patina::error::Result<bool>>>,
    }

    impl MockDxeDispatcher {
        fn new(results: &[patina::error::Result<bool>]) -> Self {
            Self { results: spin::Mutex::new(results.iter().cloned().collect()) }
        }
    }

    impl DxeDispatch for MockDxeDispatcher {
        fn dispatch(&self) -> patina::error::Result<bool> {
            self.results.lock().pop_front().expect("MockDxeDispatcher: unexpected dispatch call")
        }
    }

    fn leaked_boot_services_for_box() -> &'static MockBootServices {
        Box::leak(Box::new({
            let mut m = MockBootServices::new();
            m.expect_free_pool().returning(|_| Ok(()));
            m
        }))
    }

    fn mock_handle_buffer(
        handle_addrs: &[usize],
        boot_services: &'static MockBootServices,
    ) -> BootServicesBox<'static, [efi::Handle], MockBootServices> {
        let handles: Vec<efi::Handle> = handle_addrs.iter().map(|&a| a as efi::Handle).collect();
        let leaked = handles.leak();
        // SAFETY: leaked is a valid pointer+length from Vec::leak.
        unsafe { BootServicesBox::from_raw_parts_mut(leaked.as_mut_ptr(), leaked.len(), boot_services) }
    }

    /// Mock `locate_handle_buffer` that returns the next handle count from a sequence on each call.
    /// The final value repeats once the sequence is exhausted.
    fn expect_handle_count_sequence(boot_mock: &mut MockBootServices, counts: &'static [usize]) {
        let box_mock = leaked_boot_services_for_box();
        let call_idx = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        boot_mock.expect_locate_handle_buffer().returning(move |_| {
            let i = call_idx.fetch_add(1, Ordering::SeqCst).min(counts.len() - 1);
            let addrs: Vec<usize> = (1..=counts[i]).collect();
            Ok(mock_handle_buffer(&addrs, box_mock))
        });
    }

    #[test]
    fn test_simple_boot_manager_interleave_converges_when_handle_count_stable() {
        let mut boot_mock = MockBootServices::new();
        expect_handle_count_sequence(&mut boot_mock, &[1, 1]);
        let dxe_mock = MockDxeDispatcher::new(&[Ok(false)]);

        let result = interleave_connect_and_dispatch(|_bs: &MockBootServices| Ok(()), &boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_simple_boot_manager_interleave_loops_while_handle_count_grows() {
        let mut boot_mock = MockBootServices::new();
        // initial probe: 1, after round 1: 2 (grew → continue), after round 2: 2 (stable → exit)
        expect_handle_count_sequence(&mut boot_mock, &[1, 2, 2]);
        let dxe_mock = MockDxeDispatcher::new(&[Ok(true), Ok(false)]);

        let result = interleave_connect_and_dispatch(|_bs: &MockBootServices| Ok(()), &boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_simple_boot_manager_interleave_initial_probe_failure_propagates() {
        let mut boot_mock = MockBootServices::new();
        boot_mock.expect_locate_handle_buffer().returning(|_| Err(efi::Status::NOT_FOUND));
        let dxe_mock = MockDxeDispatcher::new(&[]);

        let result = interleave_connect_and_dispatch(|_bs: &MockBootServices| Ok(()), &boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_boot_manager_interleave_dispatch_failure_propagates() {
        let mut boot_mock = MockBootServices::new();
        expect_handle_count_sequence(&mut boot_mock, &[1]);
        let dxe_mock = MockDxeDispatcher::new(&[Err(EfiError::DeviceError)]);

        let result = interleave_connect_and_dispatch(|_bs: &MockBootServices| Ok(()), &boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_boot_manager_interleave_custom_connect_fn_failure() {
        let mut boot_mock = MockBootServices::new();
        expect_handle_count_sequence(&mut boot_mock, &[1]);
        let dxe_mock = MockDxeDispatcher::new(&[]);

        let result =
            interleave_connect_and_dispatch(|_bs: &MockBootServices| Err(EfiError::DeviceError), &boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_boot_manager_interleave_custom_connect_fn_success() {
        let mut boot_mock = MockBootServices::new();
        expect_handle_count_sequence(&mut boot_mock, &[1, 1]);
        let dxe_mock = MockDxeDispatcher::new(&[Ok(false)]);

        let result = interleave_connect_and_dispatch(|_bs: &MockBootServices| Ok(()), &boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    // Tests for ConnectController and with_connect_strategy

    #[test]
    fn test_simple_boot_manager_with_connect_strategy() {
        let config = BootConfig::new(test_device_path());
        let manager = SimpleBootManager::with_connect_strategy(config, ConnectAllStrategy);
        assert_eq!(manager.config().devices().count(), 1);
    }

    #[test]
    fn test_simple_boot_manager_with_closure_connect_strategy() {
        let config = BootConfig::new(test_device_path()).with_hotkey(0x16);
        let manager = SimpleBootManager::with_connect_strategy(config, |_bs: &StandardBootServices| Ok(()));
        assert_eq!(manager.config().hotkey(), Some(0x16));
    }

    #[test]
    fn test_simple_boot_manager_new() {
        let config = BootConfig::new(test_device_path()).with_hotkey(0x16).with_hotkey_device(test_device_path());
        let manager = SimpleBootManager::new(config);
        assert_eq!(manager.config().hotkey(), Some(0x16));
        assert_eq!(manager.config().devices().count(), 1);
        assert_eq!(manager.config().hotkey_devices().count(), 1);
    }

    #[test]
    fn test_simple_boot_manager_with_hotkey() {
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
    fn test_simple_boot_manager_without_hotkey() {
        let config = BootConfig::new(test_device_path()).with_device(test_device_path());
        let manager = SimpleBootManager::new(config);
        assert!(manager.config().hotkey().is_none());
        assert_eq!(manager.config().devices().count(), 2);
        assert_eq!(manager.config().hotkey_devices().count(), 0);
    }

    #[test]
    fn test_simple_boot_manager_with_failure_handler() {
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
