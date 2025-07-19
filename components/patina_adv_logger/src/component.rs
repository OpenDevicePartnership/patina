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
    boot_services::{BootServices, StandardBootServices, event::EventType, tpl::Tpl},
    component::IntoComponent,
    error::{EfiError, Result},
    runtime_services::{RuntimeServices, StandardRuntimeServices},
    serial::SerialIO,
    uefi_protocol::{self, ProtocolInterface, mu_variable_policy},
};
use r_efi::efi::{self, Guid};

use crate::{
    logger::AdvancedLogger,
    memory_log::{self, ADV_LOGGER_HOB_GUID, ADV_LOGGER_LOCATOR_VAR_NAME, AdvLoggerInfo},
    protocol::AdvancedLoggerProtocol,
};

const VARIABLE_WRITE_ARCH_PROTOCOL_GUID: Guid =
    Guid::from_fields(0x6441f818, 0x6362, 0x4e44, 0xb5, 0x70, &[0x7d, 0xba, 0x31, 0xdd, 0x24, 0x53]);

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
    fn entry_point(self, bs: StandardBootServices, rs: StandardRuntimeServices) -> Result<()> {
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
                return Err(EfiError::ProtocolError);
            }
            Ok(_) => {
                log::info!("Advanced Logger protocol installed.");
            }
        }

        // Create an event to write the AdvLoggerLocator variable once the variable write architectural protocol
        // is available
        match bs.create_event(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(variable_write_registered),
            Box::new((bs.clone(), rs.clone(), address)),
        ) {
            Err(status) => {
                log::error!("Failed to create create variable write registered event! Status = {:#x?}", status);
            }
            Ok(event) => {
                if let Err(status) = bs.register_protocol_notify(&VARIABLE_WRITE_ARCH_PROTOCOL_GUID, event) {
                    log::error!("Failed to register protocol notify for variable write event! Status = {:#x?}", status);
                }
            }
        };

        // Create an event to lock the AdvancedLoggerLocator variable if/when the variable policy protocol is
        // available
        match bs.create_event::<Box<StandardBootServices>>(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(variable_policy_registered),
            Box::new(bs.clone()),
        ) {
            Err(status) => {
                log::error!("Failed to create create variable policy registered event! Status = {:#x?}", status);
            }
            Ok(event) => {
                if let Err(status) = bs.register_protocol_notify(
                    &uefi_protocol::mu_variable_policy::MuVariablePolicyProtocol::PROTOCOL_GUID,
                    event,
                ) {
                    log::error!("Failed to register protocol notify for variable write event! Status = {:#x?}", status);
                }
            }
        };

        Ok(())
    }
}

/// Event callback triggered when the variable write architectural protocol is installed that will
/// write the "AdvancedLoggerLocator" variable.
extern "efiapi" fn variable_write_registered(
    event: *mut c_void,
    ctx: Box<(StandardBootServices, StandardRuntimeServices, u64)>,
) {
    let (bs, rs, address) = *ctx;

    // Always close the event to prevent a double-free when ctx is dropped
    let _ = bs.close_event(event);

    // Write the AdvLoggerLocator variable
    if let Err(status) = rs.set_variable(
        ADV_LOGGER_LOCATOR_VAR_NAME,
        &ADV_LOGGER_HOB_GUID,
        r_efi::system::VARIABLE_RUNTIME_ACCESS | r_efi::system::VARIABLE_BOOTSERVICE_ACCESS,
        &address.to_le_bytes(),
    ) {
        log::error!("Failed to set the advanced logger locator variable. Status = {:#x?}", status);
    }
}

/// Event callback triggered when the variable write architectural protocol is installed that will
/// register a Mu variable protection policy on the "AdvancedLoggerLocator" variable.
extern "efiapi" fn variable_policy_registered(event: *mut c_void, bs: Box<StandardBootServices>) {
    // Always close the event to prevent a double-free when bs is dropped
    let _ = bs.close_event(event);

    // Set the policy on the AdvLoggerLocator variable
    match unsafe { bs.locate_protocol::<mu_variable_policy::MuVariablePolicyProtocol>(None) } {
        Ok(protocol) => {
            // Match policy from Mu's AdvLoggerPkg implementation
            if let Err(status) =
                protocol.register_variable_policy(&uefi_protocol::mu_variable_policy::VariablePolicy::LockOnCreate(
                    mu_variable_policy::BasicVariablePolicy {
                        name: ADV_LOGGER_LOCATOR_VAR_NAME,
                        namespace: &ADV_LOGGER_HOB_GUID,
                        min_size: Some(size_of::<efi::PhysicalAddress>() as u32),
                        max_size: Some(size_of::<efi::PhysicalAddress>() as u32),
                        attributes_must_have: Some(
                            r_efi::system::VARIABLE_RUNTIME_ACCESS | r_efi::system::VARIABLE_BOOTSERVICE_ACCESS,
                        ),
                        attributes_cant_have: Some(
                            !(r_efi::system::VARIABLE_RUNTIME_ACCESS | r_efi::system::VARIABLE_BOOTSERVICE_ACCESS),
                        ),
                    },
                ))
            {
                log::error!(
                    "Failed to set variable policy on advanced logger locator variable. Status = {:#x?}",
                    status
                )
            }
        }
        Err(status) => {
            log::error!("Failed to locate variable policy protocol! Status = {:#x?}", status)
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
        unsafe { AdvLoggerInfo::initialize_memory_log(log_address, LOG_LEN as u32) };

        const HOB_LEN: usize = size_of::<GuidHob>() + size_of::<efi::PhysicalAddress>();
        let hob_buff = Box::into_raw(Box::new([0_u8; HOB_LEN]));
        let hob = hob_buff as *mut GuidHob;
        unsafe {
            ptr::write(
                hob,
                GuidHob {
                    header: Hob { r#type: GUID_EXTENSION, length: HOB_LEN as u16, reserved: 0 },
                    name: memory_log::ADV_LOGGER_HOB_GUID,
                },
            )
        };

        let address: *mut efi::PhysicalAddress = unsafe { hob.add(1) } as *mut efi::PhysicalAddress;
        unsafe { (*address) = log_address };
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
