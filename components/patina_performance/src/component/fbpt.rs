//! Patina FBPT Publisher
//!
//! Publishes the Firmware Basic Boot Performance Table (FBPT) at End of DXE. Queries the required size from the
//! [`PerformanceManager`] service, allocates the publishing buffer (preferring the address used on the previous
//! boot), has the service serialize the table into it, reports it through a status code, and installs it as a
//! configuration table so the operating system can find it.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::{cell::Cell, mem, ptr};

use patina::{
    BinaryGuid, Char16Array,
    component::{
        component,
        service::{
            Service,
            memory::{AllocationOptions, MemoryManager, PageAllocationStrategy},
            performance::PerformanceManager,
            uefi_services::{
                config_table::{ConfigTable, ConfigurationTableServices, ConfigurationTableServicesExt},
                event::{EventServices, EventServicesExt},
                protocol::{ProtocolServices, ProtocolServicesExt},
                tpl::Tpl,
            },
        },
    },
    error::EfiError,
    performance::guid::EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID,
    pi::{
        event::END_OF_DXE_EVENT_GROUP_GUID,
        protocol::status_code,
        status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_BS_DRIVER},
    },
    uefi::{memory::EfiMemoryType, runtime_services::RuntimeServices},
    uefi_size_to_pages,
};
use zerocopy::FromBytes;
use zerocopy_derive::{Immutable, KnownLayout};

/// Return the address where the FBPT was allocated during the previous boot.
pub(crate) fn find_previous_table_address(runtime_services: &impl RuntimeServices) -> Option<usize> {
    runtime_services
        .get_variable::<FirmwarePerformanceVariable>(
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_NAME,
            &FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID,
            Some(mem::size_of::<FirmwarePerformanceVariable>()),
        )
        .map(|(v, _)| v.boot_performance_table_pointer)
        .ok()
}

/// Struct representing the firmware performance variable data.
#[repr(C, packed)]
pub(crate) struct FirmwarePerformanceVariable {
    boot_performance_table_pointer: usize,
    _s3_performance_table_pointer: usize,
}

impl FirmwarePerformanceVariable {
    const ADDRESS_VARIABLE_NAME: Char16Array<20> = Char16Array::from_str("FirmwarePerformance");
    const ADDRESS_VARIABLE_GUID: BinaryGuid = BinaryGuid::from_string("C095791A-3001-47B2-80C9-EAC7319F2FA4");
}

impl TryFrom<Vec<u8>> for FirmwarePerformanceVariable {
    type Error = ();

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        if value.len() == mem::size_of::<Self>() {
            // SAFETY: This is safe because the value for ADDRESS_VARIABLE_GUID is an address where a FirmwarePerformanceVariable is.
            Ok(unsafe { ptr::read_unaligned(value.as_ptr().cast::<FirmwarePerformanceVariable>()) })
        } else {
            Err(())
        }
    }
}

/// Publishes the FBPT at End of DXE.
///
/// ## Example Usage
///
/// ```rust
/// use patina_performance::component::fbpt::*;
///
/// let component = FbptPublisher::new();
/// ```
#[derive(Default)]
pub struct FbptPublisher;

/// The UEFI services [`FbptPublisher`] needs, grouped into a tuple to stay within the entry point parameter limit.
type FbptServices = (
    Service<dyn EventServices>,
    Service<dyn ConfigurationTableServices>,
    Service<dyn MemoryManager>,
    Service<dyn ProtocolServices>,
);

#[component]
impl FbptPublisher {
    /// Creates a new instance of the component.
    pub const fn new() -> Self {
        Self
    }

    /// Entry point of [`FbptPublisher`]
    #[cfg_attr(coverage, coverage(off))] // Coverage disabled. Tested using the generic version, see _entry_point.
    fn entry_point(
        self,
        performance: Service<dyn PerformanceManager>,
        runtime_services: patina::uefi::runtime_services::StandardRuntimeServices,
        services: FbptServices,
    ) -> Result<(), EfiError> {
        Self::_entry_point(runtime_services, performance, services)
    }

    /// Entry point that has a generic runtime services parameter, since [`RuntimeServices::get_variable`] has no
    /// `uefi_services` equivalent yet.
    fn _entry_point<R>(
        runtime_services: R,
        performance: Service<dyn PerformanceManager>,
        services: FbptServices,
    ) -> Result<(), EfiError>
    where
        R: RuntimeServices + Clone + 'static,
    {
        let (events, config_table, memory, protocols) = services;
        let previous_address = find_previous_table_address(&runtime_services);

        // The event group may fire more than once. Only the first signal should publish the table.
        let published = Cell::new(false);
        events.on_event_group(END_OF_DXE_EVENT_GROUP_GUID, Tpl::Callback, move || {
            if published.replace(true) {
                log::debug!("Performance: End of DXE signaled again. FBPT was already published, ignoring.");
                return;
            }
            log::debug!("Performance: End of DXE reached, publishing FBPT (previous_address={previous_address:?}).");
            report_fbpt(&performance, previous_address, &config_table, &memory, &protocols);
        })?;

        Ok(())
    }
}

/// Queries the FBPT size, allocates a publishing buffer, has [`PerformanceManager`] serialize the table into it,
/// reports it through a status code, and installs it as a configuration table.
fn report_fbpt(
    performance: &Service<dyn PerformanceManager>,
    previous_address: Option<usize>,
    config_table: &Service<dyn ConfigurationTableServices>,
    memory: &Service<dyn MemoryManager>,
    protocols: &Service<dyn ProtocolServices>,
) {
    let size = match performance.published_table_size() {
        Ok(size) => size,
        Err(e) => {
            log::error!("Performance: Fail to get FBPT size: {e:?}");
            return;
        }
    };

    let Some(buffer) = allocate_fbpt_buffer(memory, previous_address, size) else {
        log::error!("Performance: Fail to allocate FBPT buffer.");
        return;
    };
    let fbpt_address = buffer.as_ptr() as usize;

    if let Err(e) = performance.publish_table(buffer) {
        log::error!("Performance: Fail to serialize FBPT: {e:?}");
        free_fbpt_buffer(memory, fbpt_address, size);
        return;
    }

    let Ok(p) = protocols.locate_protocol::<status_code::StatusCodeProtocol>() else {
        log::error!("Performance: Fail to find status code protocol.");
        return;
    };

    let status = p.report_status_code_with_data(
        EFI_PROGRESS_CODE,
        EFI_SOFTWARE_DXE_BS_DRIVER,
        0,
        patina::guid::CALLER_ID.as_efi_guid(),
        *EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID.as_efi_guid(),
        fbpt_address,
    );
    if status.is_err() {
        log::error!("Performance: Fail to report FBPT status code.");
    }

    // SAFETY: `fbpt_address` is the start of the buffer `allocate_fbpt_buffer` leaked as `'static` (never
    // freed) from `memory.allocate_pages`, and `performance.publish_table` returned `Ok` above, so the first
    // `size_of::<FbptHeader>()` bytes there are already initialized with the FBPT signature and length.
    let header_bytes = unsafe { core::slice::from_raw_parts(fbpt_address as *const u8, mem::size_of::<FbptHeader>()) };
    let Ok(header) = FbptHeader::ref_from_bytes(header_bytes) else {
        log::error!("Performance: FBPT header is misaligned, cannot install configuration table.");
        return;
    };

    let length = header.length;
    match config_table.install_or_replace(header) {
        Ok(()) => log::info!(
            "Performance: Published FBPT at {fbpt_address:#x} (size={length}, reused_previous_address={}).",
            previous_address == Some(fbpt_address)
        ),
        Err(e) => {
            log::error!("Performance: Fail to install configuration table for FBPT firmware performance: {e:?}");
        }
    }
}

/// A 4-byte `'FBPT'` signature followed by the table's total length.
#[repr(C, packed)]
#[derive(FromBytes, KnownLayout, Immutable)]
struct FbptHeader {
    _signature: u32,
    length: u32,
}

impl ConfigTable for FbptHeader {
    const TABLE_GUID: BinaryGuid = EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID;

    fn table_len(&self) -> usize {
        self.length as usize
    }
}

/// Allocates a reserved-memory buffer large enough to publish the FBPT.
///
/// The allocation prefers `previous_address` (the location used on the previous boot) so the table can be placed
/// consistently, falling back to any address below 4 GiB.
fn allocate_fbpt_buffer(
    memory: &Service<dyn MemoryManager>,
    previous_address: Option<usize>,
    size: usize,
) -> Option<&'static mut [u8]> {
    let pages = uefi_size_to_pages!(size);

    let allocation = previous_address
        .and_then(|address| {
            memory
                .allocate_pages(
                    pages,
                    AllocationOptions::new()
                        .with_memory_type(EfiMemoryType::ReservedMemoryType)
                        .with_strategy(PageAllocationStrategy::Address(address)),
                )
                .ok()
        })
        .or_else(|| {
            // `MaxAddress` requests any physical address below the given bound (u32::MAX = 4 GiB). The firmware
            // chooses the actual address.
            memory
                .allocate_pages(
                    pages,
                    AllocationOptions::new()
                        .with_memory_type(EfiMemoryType::ReservedMemoryType)
                        .with_strategy(PageAllocationStrategy::MaxAddress(u32::MAX as usize)),
                )
                .ok()
        })?;

    Some(allocation.leak_as_slice::<u8>())
}

/// Frees the FBPT buffer allocated by `allocate_fbpt_buffer`.
fn free_fbpt_buffer(memory: &Service<dyn MemoryManager>, address: usize, size: usize) {
    let pages = uefi_size_to_pages!(size);

    // SAFETY: `address` was allocated by `allocate_fbpt_buffer`, which used `memory.allocate_pages` to allocate this
    //         buffer, so it is safe to free using `memory.free_pages`.
    if let Err(e) = unsafe { memory.free_pages(address, pages) } {
        log::error!("Performance: Failed to free FBPT buffer at {address:#x}: {e:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use core::{any::TypeId, ffi::c_void};
    use patina::{
        component::service::{memory::StdMemoryManager, performance::MockPerformanceManager},
        standard::efi,
        uefi::runtime_services::MockRuntimeServices,
    };

    use patina::component::service::uefi_services::{
        config_table::MockConfigurationTableServices, event::MockEventServices, protocol::MockProtocolServices,
    };

    fn runtime_services_without_previous_address() -> MockRuntimeServices {
        let mut runtime_services = MockRuntimeServices::new();
        runtime_services
            .expect_get_variable::<FirmwarePerformanceVariable>()
            .once()
            .returning(|_, _, _| Err(efi::Status::NOT_FOUND));
        runtime_services
    }

    #[test]
    fn test_find_previous_address() {
        let mut runtime_services = MockRuntimeServices::new();

        runtime_services
            .expect_get_variable::<FirmwarePerformanceVariable>()
            .once()
            .withf(|name, namespace, size_hint| {
                assert_eq!(FirmwarePerformanceVariable::ADDRESS_VARIABLE_NAME.as_char16_str(), name);
                assert_eq!(&FirmwarePerformanceVariable::ADDRESS_VARIABLE_GUID, namespace);
                assert_eq!(&Some(16), size_hint);
                true
            })
            .returning(|_, _, _| {
                Ok((
                    FirmwarePerformanceVariable {
                        boot_performance_table_pointer: 0x12341234,
                        _s3_performance_table_pointer: 0,
                    },
                    16,
                ))
            });

        let address = find_previous_table_address(&runtime_services);

        assert_eq!(Some(0x12341234), address);
    }

    #[test]
    fn test_fbpt_publisher_entry_point_registers_end_of_dxe_event() {
        let mut events = MockEventServices::new();
        events
            .expect_create_event_for_group()
            .once()
            .withf(|group, tpl, _callback| {
                assert_eq!(group, &END_OF_DXE_EVENT_GROUP_GUID);
                assert_eq!(tpl, &Tpl::Callback);
                true
            })
            .returning(|_, _, _| {
                Ok(patina::component::service::uefi_services::event::Event::from_raw(
                    core::ptr::NonNull::<c_void>::dangling().as_ptr(),
                )
                .unwrap())
            });

        let performance: Service<dyn PerformanceManager> = Service::mock(Box::new(MockPerformanceManager::new()));
        let services: FbptServices = (
            Service::mock(Box::new(events)),
            Service::mock(Box::new(MockConfigurationTableServices::new())),
            Service::mock(Box::new(StdMemoryManager::new())),
            Service::mock(Box::new(MockProtocolServices::new())),
        );

        let result = FbptPublisher::_entry_point(runtime_services_without_previous_address(), performance, services);

        assert!(result.is_ok());
    }

    #[test]
    fn test_fbpt_publisher_report_fbpt_publishes_table_once() {
        let mut events = MockEventServices::new();
        events.expect_create_event_for_group().once().returning(|_, _, mut callback| {
            let event = patina::component::service::uefi_services::event::Event::from_raw(
                core::ptr::NonNull::<c_void>::dangling().as_ptr(),
            )
            .unwrap();
            // Signal the event group twice. Only the first signal should publish the table (enforced by the
            // `.once()` expectations on the mocks below).
            callback(event);
            callback(event);
            Ok(event)
        });

        let mut performance = MockPerformanceManager::new();
        performance.expect_published_table_size().once().returning(|| Ok(64usize));
        performance.expect_publish_table().once().returning(|_| Ok(()));

        static REPORT_STATUS_CODE_CALLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        extern "efiapi" fn report_status_code(
            _a: u32,
            _b: u32,
            _c: u32,
            _d: *const efi::Guid,
            _e: *const patina::pi::protocol::status_code::EfiStatusCodeData,
        ) -> efi::Status {
            REPORT_STATUS_CODE_CALLED.store(true, core::sync::atomic::Ordering::Relaxed);
            efi::Status::SUCCESS
        }
        let status_code_protocol =
            Box::leak(Box::new(patina::pi::protocol::status_code::StatusCodeProtocol { report_status_code }));

        let mut protocols = MockProtocolServices::new();
        protocols.expect_locate_interface().once().returning(|_| {
            patina::component::service::uefi_services::protocol::ProtocolPtr::from_raw(core::ptr::from_mut(
                status_code_protocol,
            ) as *mut c_void)
            .ok_or(patina::component::service::uefi_services::protocol::ProtocolError::NotFound)
        });

        let mut config_table = MockConfigurationTableServices::new();
        config_table.expect_replace_typed_table().once().returning(|guid, type_id, _| {
            assert_eq!(guid, EDKII_FPDT_EXTENDED_FIRMWARE_PERFORMANCE_GUID);
            assert_eq!(type_id, TypeId::of::<FbptHeader>());
            Ok(())
        });

        let services: FbptServices = (
            Service::mock(Box::new(events)),
            Service::mock(Box::new(config_table)),
            Service::mock(Box::new(StdMemoryManager::new())),
            Service::mock(Box::new(protocols)),
        );

        let result = FbptPublisher::_entry_point(
            runtime_services_without_previous_address(),
            Service::mock(Box::new(performance)),
            services,
        );

        assert!(result.is_ok());
        assert!(REPORT_STATUS_CODE_CALLED.load(core::sync::atomic::Ordering::Relaxed));
    }
}
