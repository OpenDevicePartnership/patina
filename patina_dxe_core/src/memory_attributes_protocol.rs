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
use crate::{GCD, dxe_services, protocol_db, protocols::PROTOCOL_DB};
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

    let mut found_attrs = None;
    let req_range = base_address..(base_address + length);

    // this API only returns the MEMORY_ACCESS attributes, per UEFI spec
    for desc_result in GCD.iter(base_address as usize, length as usize) {
        let descriptor = match desc_result {
            Ok(desc) => desc,
            Err(_) => {
                log::error!(
                    "No descriptors found for range [{:#x}, {:#x}) in {}",
                    base_address,
                    base_address + length,
                    function!()
                );
                return efi::Status::NO_MAPPING;
            }
        };

        // if we have already found attributes, ensure they are consistent
        match found_attrs {
            Some(attrs) if attrs != (descriptor.attributes & efi::MEMORY_ACCESS_MASK) => {
                log::error!(
                    "{} Inconsistent attributes found in range [{:#x}, {:#x})",
                    function!(),
                    base_address,
                    base_address + length
                );
                return efi::Status::NO_MAPPING;
            }
            None => found_attrs = Some(descriptor.attributes & efi::MEMORY_ACCESS_MASK),
            _ => {}
        }
    }

    if let Some(attrs) = found_attrs {
        // Safety: caller must provide a valid pointer to receive the attributes. It is null-checked above.
        unsafe { attributes.write_unaligned(attrs) };
        efi::Status::SUCCESS
    } else {
        log::error!(
            "No descriptors found for range [{:#x}, {:#x}) in {}",
            base_address,
            base_address + length,
            function!()
        );
        efi::Status::NO_MAPPING
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

    let end = match base_address.checked_add(length) {
        Some(e) => e,
        None => {
            log::error!("Address overflow in {}", function!());
            return efi::Status::INVALID_PARAMETER;
        }
    };

    let range = base_address..end;

    for desc_result in GCD.iter(base_address as usize, length as usize) {
        let descriptor = match desc_result {
            Ok(desc) => desc,
            Err(_) => {
                log::error!(
                    "No descriptors found for range [{:#x}, {:#x}) in {}",
                    base_address,
                    base_address + length,
                    function!()
                );
                return efi::Status::UNSUPPORTED;
            }
        };

        // this API only adds new attributes that are set, it ignores all 0 attributes. So, we need to get the memory
        // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
        let new_attributes = descriptor.attributes | attributes;

        let current_range = descriptor.get_range_overlap_with_desc(&range);

        // only a few status codes are allowed per UEFI spec, so return unsupported
        // we don't have a reliable mechanism to reset any previously set attributes if an earlier block succeeded
        // because any tracking mechanism would require memory allocations which could change the descriptors
        // and cause some attributes to be set on a potentially incorrect memory region. At this point if we have
        // failed, the system is dead, barring a bootloader allocating new memory and attempting to set attributes
        // there, because this API is only used by a bootloader setting memory attributes for the next image it is
        // loading. The expectation is that on a future boot the platform would disable this protocol.
        match dxe_services::core_set_memory_space_attributes(
            current_range.start,
            (current_range.end - current_range.start),
            new_attributes,
        ) {
            Ok(_) => {}
            Err(e) => {
                log::error!(
                    "Failed to set memory attributes for range [{:#x}, {:#x}) in {}: {:?}",
                    current_range.start,
                    current_range.end,
                    function!(),
                    e
                );
                return efi::Status::UNSUPPORTED;
            }
        }
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

    let end = match base_address.checked_add(length) {
        Some(e) => e,
        None => {
            log::error!("Address overflow in {}", function!());
            return efi::Status::INVALID_PARAMETER;
        }
    };
    let range = base_address..end;

    for desc_result in GCD.iter(base_address as usize, length as usize) {
        let descriptor = match desc_result {
            Ok(desc) => desc,
            Err(_) => {
                log::error!(
                    "No descriptors found for range [{:#x}, {:#x}) in {}",
                    base_address,
                    base_address + length,
                    function!()
                );
                return efi::Status::UNSUPPORTED;
            }
        };
        // this API only adds clears attributes that are set to 1, it ignores all 0 attributes. So, we need to get the memory
        // descriptor first and then set the new attributes as the GCD API takes into account all attributes set or unset.
        let new_attributes = descriptor.attributes & !attributes;
        let current_range = descriptor.get_range_overlap_with_desc(&range);

        // only a few status codes are allowed per UEFI spec, so return unsupported
        // we don't have a reliable mechanism to reset any previously set attributes if an earlier block succeeded
        // because any tracking mechanism would require memory allocations which could change the descriptors
        // and cause some attributes to be set on a potentially incorrect memory region. At this point if we have
        // failed, the system is dead, barring a bootloader allocating new memory and attempting to set attributes
        // there, because this API is only used by a bootloader setting memory attributes for the next image it is
        // loading. The expectation is that on a future boot the platform would disable this protocol.
        match dxe_services::core_set_memory_space_attributes(
            current_range.start,
            (current_range.end - current_range.start),
            new_attributes,
        ) {
            Ok(_) => {}
            Err(e) => {
                log::error!(
                    "Failed to clear memory attributes for range [{:#x}, {:#x}) in {}: {:?}",
                    current_range.start,
                    current_range.end,
                    function!(),
                    e
                );
                return efi::Status::UNSUPPORTED;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        GCD,
        gcd::{AllocateType, init_gcd},
        test_support::{self, MockPageTable, MockPageTableWrapper},
    };

    use patina::{
        base::{UEFI_PAGE_SHIFT, align_up},
        pi::dxe_services::GcdMemoryType,
    };
    use std::{cell::RefCell, ptr, rc::Rc};

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            // SAFETY: Test code - resetting the global GCD state for test isolation.
            // The test lock is used to prevent concurrent access.
            unsafe {
                GCD.reset();
            }
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_get_memory_attributes_invalid_alignment() {
        let mut attrs: u64 = 0;
        let status = get_memory_attributes(core::ptr::null_mut(), 0x1001, 0x1000, &mut attrs as *mut u64);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_get_memory_attributes_null_attributes() {
        let status = get_memory_attributes(core::ptr::null_mut(), 0x1000, 0x1000, core::ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_set_memory_attributes_invalid_alignment() {
        let status = set_memory_attributes(core::ptr::null_mut(), 0x1001, 0x1000, efi::MEMORY_RO);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_set_memory_attributes_invalid_attributes() {
        let status = set_memory_attributes(core::ptr::null_mut(), 0x1000, 0x1000, 0xdeadbeef);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_clear_memory_attributes_invalid_alignment() {
        let status = clear_memory_attributes(core::ptr::null_mut(), 0x1001, 0x1000, efi::MEMORY_RO);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_clear_memory_attributes_invalid_attributes() {
        let status = clear_memory_attributes(core::ptr::null_mut(), 0x1000, 0x1000, 0xdeadbeef);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn test_get_memory_attributes_single_descriptor() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x2000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            let mut attrs: u64 = 0;
            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x2000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, efi::MEMORY_XP);
        });
    }

    #[test]
    fn test_get_memory_attributes_partial_descriptor() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x3000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            let mut attrs: u64 = 0;
            let status =
                get_memory_attributes(core::ptr::null_mut(), addr as u64 + 0x1000, 0x1000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, efi::MEMORY_XP);
        });
    }

    #[test]
    fn test_get_memory_attributes_multiple_descriptors() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x3000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            GCD.set_memory_space_attributes(addr + 0x1000, 0x1000, efi::MEMORY_UC | efi::MEMORY_XP).unwrap();

            let mut attrs: u64 = 0;
            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, efi::MEMORY_XP);
        });
    }

    #[test]
    fn test_get_memory_attributes_multiple_descriptors_different_attrs() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x3000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            GCD.set_memory_space_attributes(addr + 0x1000, 0x1000, efi::MEMORY_RO).unwrap();

            let mut attrs: u64 = 0;
            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::NO_MAPPING);
        });
    }

    #[test]
    fn test_get_memory_attributes_no_mapping() {
        with_locked_state(|| {
            GCD.init(48, 16);

            let mut attrs: u64 = 0;
            let status = get_memory_attributes(core::ptr::null_mut(), 0x0_u64, 0x3000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::NO_MAPPING);
        });
    }

    #[test]
    fn test_set_memory_attributes_single_descriptor() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x2000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            let mut attrs: u64 = efi::MEMORY_RO;
            let status = set_memory_attributes(core::ptr::null_mut(), addr as u64, 0x2000, attrs);
            assert_eq!(status, efi::Status::SUCCESS);

            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x2000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, efi::MEMORY_RO | efi::MEMORY_XP);
        });
    }

    #[test]
    fn test_set_memory_attributes_multiple_descriptors() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x3000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            GCD.set_memory_space_attributes(addr + 0x1000, 0x1000, efi::MEMORY_UC | efi::MEMORY_XP);

            let mut attrs: u64 = efi::MEMORY_RO;
            let status = set_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, attrs);
            assert_eq!(status, efi::Status::SUCCESS);

            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, efi::MEMORY_RO | efi::MEMORY_XP);
        });
    }

    #[test]
    fn test_clear_memory_attributes_single_descriptor() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x2000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            let mut attrs: u64 = efi::MEMORY_XP;
            let status = clear_memory_attributes(core::ptr::null_mut(), addr as u64, 0x2000, attrs);
            assert_eq!(status, efi::Status::SUCCESS);

            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x2000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, 0);
        });
    }

    #[test]
    fn test_clear_memory_attributes_multiple_descriptors() {
        with_locked_state(|| {
            GCD.init(48, 16);

            // Add memory and MMIO regions
            let mem = unsafe { crate::test_support::get_memory(0x120000) };
            let address = align_up(mem.as_ptr() as usize, 0x1000).unwrap();

            // SAFETY: We just allocated this memory for testing.
            unsafe {
                GCD.init_memory_blocks(
                    patina::pi::dxe_services::GcdMemoryType::SystemMemory,
                    address,
                    0x110000,
                    efi::CACHE_ATTRIBUTE_MASK | efi::MEMORY_ACCESS_MASK,
                )
                .unwrap();
            }

            let mock_table = Rc::new(RefCell::new(MockPageTable::new()));
            let mock_page_table = Box::new(MockPageTableWrapper::new(Rc::clone(&mock_table)));
            GCD.add_test_page_table(mock_page_table);

            let addr = GCD
                .allocate_memory_space(
                    AllocateType::TopDown(None),
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    0x3000,
                    0x7 as efi::Handle,
                    None,
                )
                .unwrap();

            GCD.set_memory_space_attributes(addr + 0x1000, 0x1000, efi::MEMORY_XP | efi::MEMORY_WC);

            let mut attrs: u64 = efi::MEMORY_XP;
            let status = clear_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, attrs);
            assert_eq!(status, efi::Status::SUCCESS);

            let status = get_memory_attributes(core::ptr::null_mut(), addr as u64, 0x3000, &mut attrs as *mut u64);
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(attrs, 0);
        });
    }
}
