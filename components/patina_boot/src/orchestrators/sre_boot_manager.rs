//! System Recovery Environment boot manager.
//!
//! [`SreBootManager`] implements the [`BootOrchestrator`](crate::BootOrchestrator)
//! trait for platforms shipping a System Recovery Environment alongside the main
//! OS. The current skeleton implements the **normal** boot path only:
//!
//! 1. Interleave controller connection with DXE driver dispatch
//! 2. Signal EndOfDxe (security lockdown)
//! 3. Discover console devices
//! 4. Write-lock the NVMe boot partition (volatile, until power cycle)
//! 5. Signal ReadyToBoot
//! 6. Boot the main OS device
//!
//! Hotkey detection (Power+Vol-Up → SRE), SRE WIM RAM-disk boot, and capsule
//! update orchestration are tracked separately and will layer onto this skeleton.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::service::dxe_dispatch::DxeDispatch,
    device_path::paths::DevicePathBuf,
    error::EfiError,
    runtime_services::StandardRuntimeServices,
};
use r_efi::efi;

use crate::{boot_orchestrator::BootOrchestrator, helpers};

fn interleave_connect_and_dispatch<B: BootServices, D: DxeDispatch + ?Sized>(
    boot_services: &B,
    dxe_services: &D,
) -> patina::error::Result<()> {
    const MAX_ROUNDS: usize = 10;

    for _round in 0..MAX_ROUNDS {
        helpers::connect_all(boot_services)?;
        if !dxe_services.dispatch()? {
            return Ok(());
        }
    }

    debug_assert!(false, "connect-dispatch interleaving did not converge after {MAX_ROUNDS} rounds");

    Ok(())
}

/// SRE boot manager implementing [`BootOrchestrator`].
///
/// Skeleton — normal boot path only. The SRE-entry hotkey, WIM-to-RAM-disk boot,
/// and capsule-update pre-boot hook will land in subsequent issues and extend this
/// orchestrator without changing the public constructor surface.
pub struct SreBootManager {
    boot_partition_path: DevicePathBuf,
    main_os_path: DevicePathBuf,
}

impl SreBootManager {
    /// Construct an `SreBootManager` from the device paths of the boot partition
    /// (to be write-locked before OS hand-off) and the main OS boot device.
    pub fn new(boot_partition_path: DevicePathBuf, main_os_path: DevicePathBuf) -> Self {
        Self { boot_partition_path, main_os_path }
    }
}

impl BootOrchestrator for SreBootManager {
    #[coverage(off)] // Integration point — delegates to helper functions which are individually tested.
    fn execute(
        &self,
        boot_services: &StandardBootServices,
        runtime_services: &StandardRuntimeServices,
        dxe_dispatch: &dyn DxeDispatch,
        image_handle: efi::Handle,
    ) -> Result<!, EfiError> {
        if let Err(e) = interleave_connect_and_dispatch(boot_services, dxe_dispatch) {
            log::error!("interleave_connect_and_dispatch failed: {:?}", e);
        }

        if let Err(e) = helpers::signal_bds_phase_entry(boot_services) {
            log::error!("signal_bds_phase_entry failed: {:?}", e);
        }

        if let Err(e) = helpers::discover_console_devices(boot_services, runtime_services) {
            log::error!("discover_console_devices failed: {:?}", e);
        }

        if let Err(e) = crate::partition::lock_partition_write(boot_services, &self.boot_partition_path) {
            log::error!("lock_partition_write failed: {:?}", e);
        }

        if let Err(e) = helpers::signal_ready_to_boot(boot_services) {
            log::error!("signal_ready_to_boot failed: {:?}", e);
        }

        match helpers::boot_from_device_path(boot_services, image_handle, &self.main_os_path) {
            Ok(()) => log::warn!("Main OS boot returned control"),
            Err(_) => log::warn!("Main OS boot failed"),
        }

        log::error!("SRE normal boot exhausted main OS path");
        Err(EfiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::{boxed::Box, sync::Arc, vec::Vec};
    use patina::{
        boot_services::{MockBootServices, boxed::BootServicesBox},
        device_path::{node_defs::EndEntire, paths::DevicePathBuf},
    };

    fn test_device_path() -> DevicePathBuf {
        DevicePathBuf::from_device_path_node_iter(core::iter::once(EndEntire))
    }

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

    #[test]
    fn test_new_constructs() {
        let _ = SreBootManager::new(test_device_path(), test_device_path());
    }

    #[test]
    fn test_interleave_single_round_no_drivers_dispatched() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Ok(false)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    #[test]
    fn test_interleave_dispatch_failure_propagates() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Err(EfiError::DeviceError)]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_err());
    }

    #[test]
    fn test_interleave_stops_at_max_rounds() {
        let box_mock = leaked_boot_services_for_box();
        let mut boot_mock = MockBootServices::new();

        boot_mock.expect_locate_handle_buffer().returning(move |_| Ok(mock_handle_buffer(&[1], box_mock)));
        boot_mock.expect_connect_controller().returning(|_, _, _, _| Ok(()));

        let dxe_mock = MockDxeDispatcher::new(&[Ok(true); 10]);

        let result = interleave_connect_and_dispatch(&boot_mock, &dxe_mock);
        assert!(result.is_ok());
    }

    // Type-level confirmation that SreBootManager satisfies BootOrchestrator's
    // Send + Sync + 'static bounds at compile time.
    #[test]
    fn test_implements_boot_orchestrator() {
        fn assert_orchestrator<T: BootOrchestrator>() {}
        assert_orchestrator::<SreBootManager>();
    }

    // Confirm the manager is constructible behind an Arc<dyn BootOrchestrator>,
    // matching the BootDispatcher consumption path.
    #[test]
    fn test_arc_dyn_construction() {
        let _: Arc<dyn BootOrchestrator> = Arc::new(SreBootManager::new(test_device_path(), test_device_path()));
    }
}
