//! DXE Core Memory Attributes Protocol
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![allow(unused)]
/// Architecture independent public C EFI Memory Attributes Protocol definition.
use crate::{dxe_services, protocol_db, protocols::PROTOCOL_DB};
use alloc::boxed::Box;
use core::{
    ffi::c_void,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};
use mu_rust_helpers::function;
use patina::{base::UEFI_PAGE_MASK, error::EfiError};
use r_efi::efi;

#[repr(C)]
pub struct EfiMemoryAttributesProtocolImpl {
    protocol: efi::protocols::memory_attribute::Protocol,
}

extern "efiapi" fn get_memory_attributes(
    _this: *mut efi::protocols::memory_attribute::Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: *mut u64,
) -> efi::Status {
    // We can only get attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK as u64) != 0 || (length & UEFI_PAGE_MASK as u64) != 0 {
        log::error!("base_address and length must be page aligned in {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    if attributes.is_null() {
        log::error!("Attributes is null, failing {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // this API only returns the MEMORY_ACCESS attributes, per UEFI spec
    // TODO: This should really go to the page table, not GCD, even though GCD is the source of truth...page table actually is
    match dxe_services::core_get_memory_space_descriptor(base_address) {
        Ok(descriptor) => {
            if descriptor.base_address != base_address || descriptor.length != length {
                log::error!(
                    "{} Inconsistent attributes for: base_address {:#x} length {:#x}",
                    function!(),
                    base_address,
                    length
                );
                return efi::Status::NO_MAPPING;
            }
            // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
            unsafe { attributes.write_unaligned(descriptor.attributes & efi::MEMORY_ACCESS_MASK) };
            efi::Status::SUCCESS
        }
        Err(status) => {
            log::error!(
                "Failed to get memory descriptor for address {:#x}: {:?} in {}",
                base_address,
                status,
                function!()
            );
            efi::Status::NO_MAPPING
        }
    }
}

extern "efiapi" fn set_memory_attributes(
    _this: *mut efi::protocols::memory_attribute::Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    // We can only set attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK as u64) != 0 || (length & UEFI_PAGE_MASK as u64) != 0 {
        log::error!("base_address and length must be page aligned in {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // UEFI spec only allows MEMORY_RO, MEMORY_RP, and MEMORY_XP to be set through this API
    if attributes == 0 || (attributes & efi::MEMORY_ACCESS_MASK) != attributes {
        log::error!("Invalid attributes {:x?} in {}", attributes, function!());
        return efi::Status::INVALID_PARAMETER;
    }

    let mut current_base = base_address;
    let range_end = (base_address + length);
    while current_base < range_end {
        let descriptor = match dxe_services::core_get_memory_space_descriptor(current_base as efi::PhysicalAddress) {
            Ok(descriptor) => descriptor,
            Err(e) => {
                log::error!(
                    "Memory descriptor fetching failed with error {:#x?} for {:#x} in {}",
                    e,
                    current_base,
                    function!()
                );
                // Only a few error codes are allowed per UEFI spec, so return unsupported
                return efi::Status::UNSUPPORTED;
            }
        };
        let descriptor_end = descriptor.base_address + descriptor.length;

        // it is still legal to split a descriptor and only set the attributes on part of it
        let next_base = u64::min(descriptor_end, range_end);
        let current_len = next_base - current_base;

        // this API only adds new attributes that are set, it ignores all 0 attributes. So, we need to get the memory
        // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
        let new_attributes = descriptor.attributes | attributes;

        match dxe_services::core_set_memory_space_attributes(current_base, current_len, new_attributes) {
            Ok(_) => {}
            // only a few status codes are allowed per UEFI spec, so return unsupported
            // we don't have a reliable mechanism to reset any previously set attributes if an earlier block succeeded
            // because any tracking mechanism would be require memory allocations which could change the descriptors
            // and cause some attributes to be set on a potentially incorrect memory region. At this point if we have
            // failed, the system is dead, barring a bootloader allocating new memory and attempting to set attributes
            // there, because this API is only used by a bootloader setting memory attributes for the next image it is
            // loading. The expectation is that on a future boot the platform would disable this protocol.
            Err(status) => return efi::Status::UNSUPPORTED,
        };
        current_base = next_base;
    }
    efi::Status::SUCCESS
}

extern "efiapi" fn clear_memory_attributes(
    _this: *mut efi::protocols::memory_attribute::Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    // We can only clear attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK as u64) != 0 || (length & UEFI_PAGE_MASK as u64) != 0 {
        log::error!("base_address and length must be page aligned in {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // UEFI spec only allows MEMORY_RO, MEMORY_RP, and MEMORY_XP to be cleared through this API
    if attributes == 0 || (attributes & efi::MEMORY_ACCESS_MASK) != attributes {
        log::error!("Invalid attributes {:x?} in {}", attributes, function!());
        return efi::Status::INVALID_PARAMETER;
    }

    let mut current_base = base_address;
    let range_end = (base_address + length);
    while current_base < range_end {
        let descriptor = match dxe_services::core_get_memory_space_descriptor(current_base as efi::PhysicalAddress) {
            Ok(descriptor) => descriptor,
            Err(e) => {
                log::error!(
                    "Memory descriptor fetching failed with error {:#x?} for {:#x} in {}",
                    e,
                    current_base,
                    function!()
                );
                // Only a few error codes are allowed per UEFI spec, so return unsupported
                return efi::Status::UNSUPPORTED;
            }
        };
        let descriptor_end = descriptor.base_address + descriptor.length;

        // it is still legal to split a descriptor and only set the attributes on part of it
        let next_base = u64::min(descriptor_end, range_end);
        let current_len = next_base - current_base;

        // this API only adds clears attributes that are set to 1, it ignores all 0 attributes. So, we need to get the memory
        // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
        let new_attributes = descriptor.attributes & !attributes;

        match dxe_services::core_set_memory_space_attributes(current_base, current_len, new_attributes) {
            Ok(_) => {}
            // only a few status codes are allowed per UEFI spec, so return unsupported
            // we don't have a reliable mechanism to reset any previously set attributes if an earlier block succeeded
            // because any tracking mechanism would be require memory allocations which could change the descriptors
            // and cause some attributes to be set on a potentially incorrect memory region. At this point if we have
            // failed, the system is dead, barring a bootloader allocating new memory and attempting to set attributes
            // there, because this API is only used by a bootloader setting memory attributes for the next image it is
            // loading. The expectation is that on a future boot the platform would disable this protocol.
            Err(status) => return efi::Status::UNSUPPORTED,
        };
        current_base = next_base;
    }
    efi::Status::SUCCESS
}

impl EfiMemoryAttributesProtocolImpl {
    fn new() -> Self {
        Self {
            protocol: efi::protocols::memory_attribute::Protocol {
                get_memory_attributes,
                set_memory_attributes,
                clear_memory_attributes,
            },
        }
    }
}

static MEMORY_ATTRIBUTES_PROTOCOL_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static MEMORY_ATTRIBUTES_PROTOCOL_INTERFACE: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// This function is called by the DXE Core to install the protocol.
pub(crate) fn install_memory_attributes_protocol() {
    let protocol = EfiMemoryAttributesProtocolImpl::new();

    // Convert the protocol to a raw pointer and store it in to protocol DB
    let interface = Box::into_raw(Box::new(protocol));
    let interface = interface as *mut c_void;
    MEMORY_ATTRIBUTES_PROTOCOL_INTERFACE.store(interface, Ordering::SeqCst);

    match PROTOCOL_DB.install_protocol_interface(None, efi::protocols::memory_attribute::PROTOCOL_GUID, interface) {
        Ok((handle, _)) => unsafe {
            MEMORY_ATTRIBUTES_PROTOCOL_HANDLE.store(handle, Ordering::SeqCst);
        },
        Err(e) => {
            log::error!("Failed to install MEMORY_ATTRIBUTES_PROTOCOL_GUID: {e:?}");
        }
    }
}

#[cfg(feature = "compatibility_mode_allowed")]
/// This function is called in compatibility mode to uninstall the protocol.
pub(crate) fn uninstall_memory_attributes_protocol() {
    unsafe {
        match (
            MEMORY_ATTRIBUTES_PROTOCOL_HANDLE.load(Ordering::SeqCst),
            MEMORY_ATTRIBUTES_PROTOCOL_INTERFACE.load(Ordering::SeqCst),
        ) {
            (handle, interface) if handle != protocol_db::INVALID_HANDLE && !interface.is_null() => {
                match PROTOCOL_DB.uninstall_protocol_interface(
                    handle,
                    efi::protocols::memory_attribute::PROTOCOL_GUID,
                    interface,
                ) {
                    Ok(_) => {
                        log::info!("uninstalled MEMORY_ATTRIBUTES_PROTOCOL_GUID");
                    }
                    Err(e) => {
                        log::error!("Failed to uninstall MEMORY_ATTRIBUTES_PROTOCOL_GUID: {e:?}");
                    }
                }
            }
            _ => {
                log::error!("MEMORY_ATTRIBUTES_PROTOCOL_GUID was not installed");
            }
        }
    }
}
