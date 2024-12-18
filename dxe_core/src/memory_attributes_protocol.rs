#![allow(unused)]
/// Architecture independent public C EFI Memory Attributes Protocol definition.
use crate::{dxe_services, protocols::PROTOCOL_DB};
use alloc::boxed::Box;
use core::ffi::c_void;
use mu_rust_helpers::function;
use r_efi::{efi, eficall, eficall_abi};
use uefi_sdk::error::EfiError;

/// Temporarily host this here as the r-efi maintainer is out and not merging PRs
pub const MEMORY_ATTRIBUTES_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xf4560cf6, 0x40ec, 0x4b4a, 0xa1, 0x92, &[0xbf, 0x1d, 0x57, 0xd0, 0xb1, 0x89]);

pub type GetMemoryAttributes = eficall! {fn(
    *const Protocol,
    efi::PhysicalAddress,
    u64,
    *mut u64,
) -> efi::Status};

pub type SetMemoryAttributes = eficall! {fn(
    *const Protocol,
    efi::PhysicalAddress,
    u64,
    u64,
) -> efi::Status};

pub type ClearMemoryAttributes = eficall! {fn(
    *const Protocol,
    efi::PhysicalAddress,
    u64,
    u64,
) -> efi::Status};

#[repr(C)]
pub struct Protocol {
    pub get_memory_attributes: GetMemoryAttributes,
    pub set_memory_attributes: SetMemoryAttributes,
    pub clear_memory_attributes: ClearMemoryAttributes,
}

#[repr(C)]
pub struct EfiMemoryAttributesProtocolImpl {
    protocol: Protocol,
}

const UEFI_PAGE_MASK: u64 = 0xFFF;

extern "efiapi" fn get_memory_attributes(
    _this: *const Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: *mut u64,
) -> efi::Status {
    // We can only get attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK) != 0 || (length & UEFI_PAGE_MASK) != 0 {
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
            unsafe { *attributes = descriptor.attributes & efi::MEMORY_ACCESS_MASK };
            efi::Status::SUCCESS
        }
        Err(status) => {
            log::error!(
                "Failed to get memory descriptor for address {:#x}: {:?} in {}",
                base_address,
                status,
                function!()
            );
            status
        }
    }
}

extern "efiapi" fn set_memory_attributes(
    _this: *const Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    // We can only set attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK) != 0 || (length & UEFI_PAGE_MASK) != 0 {
        log::error!("base_address and length must be page aligned in {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // UEFI spec only allows MEMORY_RO, MEMORY_RP, and MEMORY_XP to be set through this API
    if attributes == 0 || (attributes & efi::MEMORY_ACCESS_MASK) != attributes {
        log::error!("Invalid attributes {:x?} in {}", attributes, function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // this API only adds new attributes that are set, it ignores all 0 attributes. So, we need to get the memory
    // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
    match dxe_services::core_get_memory_space_descriptor(base_address) {
        Ok(descriptor) => {
            let new_attributes = descriptor.attributes | attributes;
            match dxe_services::core_set_memory_space_attributes(base_address, length, new_attributes) {
                Ok(_) => efi::Status::SUCCESS,
                Err(status) => status,
            }
        }
        Err(status) => {
            log::error!(
                "Failed to get memory descriptor for address {:#x}: {:?} in {}",
                base_address,
                status,
                function!()
            );
            status
        }
    }
}

extern "efiapi" fn clear_memory_attributes(
    _this: *const Protocol,
    base_address: efi::PhysicalAddress,
    length: u64,
    attributes: u64,
) -> efi::Status {
    // We can only clear attributes on page aligned base_addresses and lengths
    if (base_address & UEFI_PAGE_MASK) != 0 || (length & UEFI_PAGE_MASK) != 0 {
        log::error!("base_address and length must be page aligned in {}", function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // UEFI spec only allows MEMORY_RO, MEMORY_RP, and MEMORY_XP to be cleared through this API
    if attributes == 0 || (attributes & efi::MEMORY_ACCESS_MASK) != attributes {
        log::error!("Invalid attributes {:x?} in {}", attributes, function!());
        return efi::Status::INVALID_PARAMETER;
    }

    // this API only adds clears attributes that are set to 1, it ignores all 0 attributes. So, we need to get the memory
    // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
    match dxe_services::core_get_memory_space_descriptor(base_address) {
        Ok(descriptor) => {
            let new_attributes = descriptor.attributes & !attributes;
            match dxe_services::core_set_memory_space_attributes(base_address, length, new_attributes) {
                Ok(_) => efi::Status::SUCCESS,
                Err(status) => status,
            }
        }
        Err(status) => {
            log::error!(
                "Failed to get memory descriptor for address {:#x}: {:?} in {}",
                base_address,
                status,
                function!()
            );
            status
        }
    }
}

impl EfiMemoryAttributesProtocolImpl {
    fn new() -> Self {
        Self { protocol: Protocol { get_memory_attributes, set_memory_attributes, clear_memory_attributes } }
    }
}

/// This function is called by the DXE Core to install the protocol.
pub(crate) fn install_memory_attributes_protocol() {
    let protocol = EfiMemoryAttributesProtocolImpl::new();

    // Convert the protocol to a raw pointer and store it in to protocol DB
    let interface = Box::into_raw(Box::new(protocol));
    let interface = interface as *mut c_void;

    let _ = PROTOCOL_DB.install_protocol_interface(None, MEMORY_ATTRIBUTES_PROTOCOL_GUID, interface);
    log::info!("installed MEMORY_ATTRIBUTES_PROTOCOL_GUID");
}
