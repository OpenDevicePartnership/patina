//! UEFI Advanced Logger Protocol Support
//!
//! This module provides the component to initialize and publish the advanced
//! logger
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::boxed::Box;
use core::{ffi::c_void, ptr};
use mu_pi::hob::{Hob, PhaseHandoffInformationTable};
use r_efi::efi;
use uefi_component_interface::{DxeComponent, DxeComponentInterface};
use uefi_core::{
    error::{EfiError, Result},
    interface::SerialIO,
};

use crate::{logger::AdvancedLogger, memory_log, memory_log::AdvLoggerInfo};

type AdvancedLoggerWriteProtocol<S> =
    extern "efiapi" fn(*const AdvancedLoggerProtocol<S>, usize, *const u8, usize) -> efi::Status;

/// C struct for the Advanced Logger protocol.
#[repr(C)]
struct AdvancedLoggerProtocol<S>
where
    S: SerialIO + Send + 'static,
{
    signature: u32,
    version: u32,
    write_log: AdvancedLoggerWriteProtocol<S>,
    log_info: efi::PhysicalAddress, // Internal field for access lib.

    // Internal rust access only! Does not exist in C definition.
    adv_logger: &'static AdvancedLogger<'static, S>,
}

impl<S> AdvancedLoggerProtocol<S>
where
    S: SerialIO + Send,
{
    /// Protocol GUID for the Advanced Logger protocol.
    pub const GUID: efi::Guid =
        efi::Guid::from_fields(0x434f695c, 0xef26, 0x4a12, 0x9e, 0xba, &[0xdd, 0xef, 0x00, 0x97, 0x49, 0x7c]);

    /// Signature used for the Advanced Logger protocol.
    pub const SIGNATURE: u32 = 0x50474F4C; // "LOGP"

    /// Current version of the Advanced Logger protocol.
    pub const VERSION: u32 = 2;

    pub const fn new(
        write_log: AdvancedLoggerWriteProtocol<S>,
        log_info: efi::PhysicalAddress,
        adv_logger: &'static AdvancedLogger<S>,
    ) -> Self {
        AdvancedLoggerProtocol { signature: Self::SIGNATURE, version: Self::VERSION, write_log, log_info, adv_logger }
    }
}

/// The component that will install the Advanced Logger protocol.
pub struct AdvancedLoggerComponent<S>
where
    S: SerialIO + Send + 'static,
{
    adv_logger: &'static AdvancedLogger<'static, S>,
}

impl<S> AdvancedLoggerComponent<S>
where
    S: SerialIO + Send + 'static,
{
    /// Creates a new AdvancedLoggerComponent.
    pub const fn new(adv_logger: &'static AdvancedLogger<S>) -> Self {
        Self { adv_logger }
    }

    /// Initialize the advanced logger.
    ///
    /// Initializes the advanced logger memory log based on the provided physical hob
    /// list. The physical hob list is used so this can be initialized before memory
    /// allocations.
    ///
    pub fn init_advanced_logger(&self, physical_hob_list: *const c_void) -> Result<()> {
        let hob_list = Hob::Handoff(unsafe {
            (physical_hob_list as *const PhaseHandoffInformationTable).as_ref::<'static>().unwrap()
        });

        for hob in &hob_list {
            if let Hob::GuidHob(guid_hob, data) = hob {
                if guid_hob.name == memory_log::ADV_LOGGER_HOB_GUID {
                    // SAFETY: The HOB will have a address of the log info
                    // immediately following the HOB header.
                    unsafe {
                        let address: *const efi::PhysicalAddress = ptr::from_ref(data) as *const efi::PhysicalAddress;
                        let log_info_addr = (*address) as efi::PhysicalAddress;
                        self.adv_logger.set_log_info_address(log_info_addr);
                    };
                    return Ok(());
                }
            }
        }

        Err(EfiError::NotFound)
    }

    /// EFI API to write to the advanced logger through the advanced logger protocol.
    extern "efiapi" fn adv_log_write(
        this: *const AdvancedLoggerProtocol<S>,
        error_level: usize,
        buffer: *const u8,
        num_bytes: usize,
    ) -> efi::Status {
        // SAFETY: We have no choice but to trust the caller on the buffer size. convert
        //         to a reference for internal safety.
        let data = unsafe { core::slice::from_raw_parts(buffer, num_bytes) };
        let error_level = error_level as u32;

        // SAFETY: We must trust the C code was a responsible steward of this buffer.
        unsafe { (*this).adv_logger }.log_write(error_level, data);
        efi::Status::SUCCESS
    }
}

impl<S> DxeComponent for AdvancedLoggerComponent<S>
where
    S: SerialIO + Send,
{
    /// Entry point to the AdvancedLoggerComponent.
    ///
    /// Installs the Advanced Logger Protocol for use by non-local components.
    ///
    fn entry_point(&self, dxe_interface: &dyn DxeComponentInterface) -> Result<()> {
        if self.adv_logger.get_log_info().is_none() {
            log::error!("Advanced logger not initialized before component entry point!");
            return Err(EfiError::NotStarted);
        }

        let log_info = self.adv_logger.get_log_info().unwrap();
        let address = log_info as *const AdvLoggerInfo as efi::PhysicalAddress;
        let protocol = AdvancedLoggerProtocol::new(Self::adv_log_write, address, self.adv_logger);

        // deliberate leak
        let interface = Box::into_raw(Box::new(protocol));
        let interface = interface as *mut c_void;

        match dxe_interface.install_protocol_interface(None, AdvancedLoggerProtocol::<S>::GUID, interface) {
            Err(status) => {
                log::error!("Failed to install Advanced Logger protocol! Status = {:#x?}", status);
                Err(EfiError::ProtocolError)
            }
            Ok(_) => {
                log::info!("Advanced Logger protocol installed.");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use core::mem::size_of;

    use mu_pi::hob::{header::Hob, GuidHob, GUID_EXTENSION};
    use serial_writer::UartNull;

    use super::*;

    static TEST_LOGGER: AdvancedLogger<UartNull> =
        AdvancedLogger::new(uefi_logger::Format::Standard, &[], log::LevelFilter::Trace, UartNull {});

    unsafe fn create_adv_logger_hob_list() -> *const c_void {
        const LOG_LEN: usize = 0x2000;
        let log_buff = Box::into_raw(Box::new([0_u8; LOG_LEN]));
        let log_address = log_buff as *const u8 as efi::PhysicalAddress;

        // initialize the log so it's valid for the hob list
        AdvLoggerInfo::initialize_memory_log(log_address, LOG_LEN as u32);

        const HOB_LEN: usize = size_of::<GuidHob>() + size_of::<efi::PhysicalAddress>();
        let hob_buff = Box::into_raw(Box::new([0_u8; HOB_LEN]));
        let hob = hob_buff as *mut GuidHob;
        ptr::write(
            hob,
            GuidHob {
                header: Hob { r#type: GUID_EXTENSION, length: HOB_LEN as u16, reserved: 0 },
                name: memory_log::ADV_LOGGER_HOB_GUID,
            },
        );

        let address: *mut efi::PhysicalAddress = hob.add(1) as *mut efi::PhysicalAddress;
        (*address) = log_address;
        hob_buff as *const c_void
    }

    #[test]
    fn component_test() {
        let component = AdvancedLoggerComponent::new(&TEST_LOGGER);
        let hob_list = unsafe { create_adv_logger_hob_list() };

        let res = component.init_advanced_logger(hob_list);
        assert_eq!(res, Ok(()));

        // TODO: Need to mock the protocol interface but requires final component interface.
    }
}
