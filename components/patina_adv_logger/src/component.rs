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
use patina_sdk::{
    boot_services::{BootServices, StandardBootServices},
    component::IntoComponent,
    error::{EfiError, Result},
    serial::SerialIO,
};
use r_efi::efi;

use crate::{
    logger::AdvancedLogger,
    memory_log::{self, AdvLoggerInfo},
    protocol::AdvancedLoggerProtocol,
};

/// C struct for the internal Advanced Logger protocol for the component.
#[repr(C)]
struct AdvancedLoggerProtocolInternal<S>
where
    S: SerialIO + Send + 'static,
{
    // The public protocol that external callers will depend on.
    protocol: AdvancedLoggerProtocol,

    // Internal component access only! Does not exist in C definition.
    adv_logger: &'static AdvancedLogger<'static, S>,
}

/// The component that will install the Advanced Logger protocol.
#[derive(IntoComponent)]
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
        debug_assert!(!physical_hob_list.is_null(), "Could not initialize adv logger due to null hob list.");
        let hob_list_info =
            unsafe { (physical_hob_list as *const PhaseHandoffInformationTable).as_ref() }.ok_or_else(|| {
                log::error!("Could not initialize adv logger due to null hob list.");
                EfiError::InvalidParameter
            })?;
        let hob_list = Hob::Handoff(hob_list_info);
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
        this: *const AdvancedLoggerProtocol,
        error_level: usize,
        buffer: *const u8,
        num_bytes: usize,
    ) -> efi::Status {
        // SAFETY: We have no choice but to trust the caller on the buffer size. convert
        //         to a reference for internal safety.
        let data = unsafe { core::slice::from_raw_parts(buffer, num_bytes) };
        let error_level = error_level as u32;

        // SAFETY: We must trust the C code was a responsible steward of this buffer.
        let internal = unsafe { &*(this as *const AdvancedLoggerProtocolInternal<S>) };

        internal.adv_logger.log_write(error_level, data);
        efi::Status::SUCCESS
    }

    /// Entry point to the AdvancedLoggerComponent.
    ///
    /// Installs the Advanced Logger Protocol for use by non-local components.
    ///
    fn entry_point(self, bs: StandardBootServices) -> Result<()> {
        let log_info = match self.adv_logger.get_log_info() {
            Some(log_info) => log_info,
            None => {
                log::error!("Advanced logger not initialized before component entry point!");
                return Err(EfiError::NotStarted);
            }
        };

        let address = log_info as *const AdvLoggerInfo as efi::PhysicalAddress;
        let protocol = AdvancedLoggerProtocolInternal {
            protocol: AdvancedLoggerProtocol::new(Self::adv_log_write, address),
            adv_logger: self.adv_logger,
        };

        let protocol = Box::leak(Box::new(protocol));
        match bs.install_protocol_interface(None, &mut protocol.protocol) {
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

    use mu_pi::hob::{GUID_EXTENSION, GuidHob, header::Hob};
    use patina_sdk::serial::uart::UartNull;

    use super::*;

    static TEST_LOGGER: AdvancedLogger<UartNull> =
        AdvancedLogger::new(patina_sdk::log::Format::Standard, &[], log::LevelFilter::Trace, UartNull {});

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
