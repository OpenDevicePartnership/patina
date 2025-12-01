//! DXE Core Memory Manager
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::boxed::Box;
use patina::{
    base::{UEFI_PAGE_MASK, UEFI_PAGE_SIZE},
    component::service::{
        IntoService, Service,
        memory::{
            AccessType, AllocationOptions, CachingType, MemoryError, MemoryManager, PageAllocation,
            PageAllocationStrategy,
        },
    },
    efi_types::EfiMemoryType,
    error::EfiError,
    test::patina_test,
    u_assert, u_assert_eq, uefi_pages_to_size,
};
use r_efi::efi;

use crate::{
    GCD,
    allocator::{core_allocate_pages, core_free_pages},
    dxe_services,
};

/// Structure for wrapper rust allocator APIs.
#[derive(IntoService)]
#[service(dyn MemoryManager)]
pub(crate) struct CoreMemoryManager;

impl MemoryManager for CoreMemoryManager {
    fn allocate_pages(&self, page_count: usize, options: AllocationOptions) -> Result<PageAllocation, MemoryError> {
        allow_allocations_for_type(options.memory_type())?;
        let mut address: efi::PhysicalAddress = 0;
        let alignment = options.alignment();

        if !alignment.is_power_of_two() || alignment & UEFI_PAGE_MASK != 0 {
            return Err(MemoryError::InvalidAlignment);
        }

        let alloc_type = match options.strategy() {
            PageAllocationStrategy::Any => efi::ALLOCATE_ANY_PAGES,
            PageAllocationStrategy::Address(requested_address) => {
                if requested_address % alignment != 0 {
                    return Err(MemoryError::UnalignedAddress);
                }

                address = requested_address as efi::PhysicalAddress;
                efi::ALLOCATE_ADDRESS
            }
            PageAllocationStrategy::MaxAddress(max_address) => {
                address = max_address as efi::PhysicalAddress;
                efi::ALLOCATE_MAX_ADDRESS
            }
        };

        let result =
            core_allocate_pages(alloc_type, options.memory_type().into(), page_count, &mut address, Some(alignment));

        match result {
            Ok(_) => {
                let allocation = unsafe {
                    PageAllocation::new(address as usize, page_count, &CoreMemoryManager)
                        .map_err(|_| MemoryError::InternalError)?
                };
                Ok(allocation)
            }
            Err(EfiError::OutOfResources) => Err(MemoryError::NoAvailableMemory),
            Err(_) => Err(MemoryError::InternalError),
        }
    }

    unsafe fn free_pages(&self, address: usize, page_count: usize) -> Result<(), MemoryError> {
        let result = core_free_pages(address as efi::PhysicalAddress, page_count);
        match result {
            Ok(_) => Ok(()),
            Err(EfiError::NotFound) => Err(MemoryError::InvalidAddress),
            Err(_) => Err(MemoryError::InternalError),
        }
    }

    fn get_allocator(&self, memory_type: EfiMemoryType) -> Result<&'static dyn core::alloc::Allocator, MemoryError> {
        let allocator =
            crate::allocator::core_get_allocator(memory_type.into()).map_err(|_| MemoryError::UnsupportedMemoryType)?;
        Ok(allocator as &'static dyn core::alloc::Allocator)
    }

    unsafe fn set_page_attributes(
        &self,
        address: usize,
        page_count: usize,
        access: AccessType,
        caching: Option<CachingType>,
    ) -> Result<(), MemoryError> {
        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }

        if !address.is_multiple_of(UEFI_PAGE_SIZE) {
            return Err(MemoryError::UnalignedAddress);
        }

        let access_attributes = match access {
            AccessType::NoAccess => efi::MEMORY_RP,
            AccessType::ReadOnly => efi::MEMORY_RO | efi::MEMORY_XP,
            AccessType::ReadWrite => efi::MEMORY_XP,
            AccessType::ReadExecute => efi::MEMORY_RO,
            AccessType::ReadWriteExecute => return Err(MemoryError::UnsupportedAttributes),
        };

        let cache_attributes = match caching {
            Some(CachingType::Uncached) => Some(efi::MEMORY_UC),
            Some(CachingType::WriteBack) => Some(efi::MEMORY_WB),
            Some(CachingType::WriteCombining) => Some(efi::MEMORY_WC),
            Some(CachingType::WriteThrough) => Some(efi::MEMORY_WT),
            Some(CachingType::WriteProtect) => return Err(MemoryError::UnsupportedAttributes),
            None => None,
        };

        let len = uefi_pages_to_size!(page_count);
        let range = address as u64..address.checked_add(len).ok_or(MemoryError::InvalidAddress)? as u64;

        for desc_result in GCD.iter(address, len) {
            let desc = desc_result.map_err(|_| MemoryError::InternalError)?;
            let current_range = desc.get_range_overlap_with_desc(&range);

            // Always clear all access attributes and set the requested ones.
            let mut new_attributes = desc.attributes & !efi::MEMORY_ACCESS_MASK;
            new_attributes |= access_attributes;

            // If no cache attributes were requested, leave them unchanged.
            if let Some(cache_attributes) = cache_attributes {
                new_attributes &= !efi::CACHE_ATTRIBUTE_MASK;
                new_attributes |= cache_attributes;
            }

            dxe_services::core_set_memory_space_attributes(
                current_range.start,
                current_range.end - current_range.start,
                new_attributes,
            )
            .map_err(|_| MemoryError::InternalError)?;
        }

        Ok(())
    }

    fn get_page_attributes(&self, address: usize, page_count: usize) -> Result<(AccessType, CachingType), MemoryError> {
        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }

        if !address.is_multiple_of(UEFI_PAGE_SIZE) {
            return Err(MemoryError::UnalignedAddress);
        }

        let base_address = address as efi::PhysicalAddress;
        let length = uefi_pages_to_size!(page_count) as u64;
        let attributes = match dxe_services::core_get_memory_space_descriptor(base_address) {
            Ok(descriptor) => {
                if base_address + length > descriptor.base_address + descriptor.length {
                    log::error!("Inconsistent attributes for: base_address {base_address:#x} length {length:#x}");
                    return Err(MemoryError::InconsistentRangeAttributes);
                }
                descriptor.attributes
            }
            Err(status) => {
                log::error!("Failed to get memory descriptor for address {base_address:#x}: {status:?}");
                return Err(MemoryError::InvalidAddress);
            }
        };

        Ok((
            AccessType::from_efi_attributes(attributes),
            CachingType::from_efi_attributes(attributes).unwrap_or(CachingType::WriteBack),
        ))
    }
}

fn allow_allocations_for_type(memory_type: EfiMemoryType) -> Result<(), MemoryError> {
    match memory_type {
        EfiMemoryType::ReservedMemoryType
        | EfiMemoryType::LoaderCode
        | EfiMemoryType::LoaderData
        | EfiMemoryType::BootServicesCode
        | EfiMemoryType::BootServicesData
        | EfiMemoryType::RuntimeServicesCode
        | EfiMemoryType::RuntimeServicesData
        | EfiMemoryType::ACPIReclaimMemory
        | EfiMemoryType::ACPIMemoryNVS
        | EfiMemoryType::MemoryMappedIO
        | EfiMemoryType::MemoryMappedIOPortSpace
        | EfiMemoryType::OemMemoryType(_)
        | EfiMemoryType::OsMemoryType(_) => Ok(()),
        _ => Err(MemoryError::UnsupportedMemoryType),
    }
}

#[patina_test]
fn memory_manager_allocations_test(mm: Service<dyn MemoryManager>) -> patina::test::Result {
    // Allocate a page, and make sure it is accessible.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let mut data = allocation.into_boxed_slice::<u8>();
    u_assert_eq!(data.len(), UEFI_PAGE_SIZE, "Failed to free page.");
    data[0] = 42;
    drop(data);

    // Allocate a page, free it then allocate the address.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let address = allocation.into_raw_ptr::<u8>().unwrap() as usize;
    let result = unsafe { mm.free_pages(address, 1) };
    u_assert!(result.is_ok(), "Failed to free page.");
    let result = mm.allocate_pages(1, AllocationOptions::new().with_strategy(PageAllocationStrategy::Address(address)));
    u_assert!(result.is_ok(), "Failed to allocate page by address");
    u_assert_eq!(result.unwrap().into_raw_ptr::<u8>().unwrap() as usize, address, "Failed to allocate correct address");

    // Allocate an aligned address.
    const TEST_ALIGNMENT: usize = 0x400000;
    let result = mm.allocate_pages(8, AllocationOptions::new().with_alignment(TEST_ALIGNMENT));
    u_assert!(result.is_ok(), "Failed to allocate single aligned pages.");
    let allocation = result.unwrap();
    u_assert_eq!(allocation.page_count(), 8);
    let address = allocation.into_raw_ptr::<u8>().unwrap() as usize;
    u_assert_eq!(address % TEST_ALIGNMENT, 0, "Allocated page not correctly aligned.");
    let result = unsafe { mm.free_pages(address, 8) };
    u_assert!(result.is_ok(), "Failed to free page.");

    // Allocate with a max address limit.
    let max_address = 0x1000_0000;
    let result =
        mm.allocate_pages(1, AllocationOptions::new().with_strategy(PageAllocationStrategy::MaxAddress(max_address)));
    u_assert!(result.is_ok(), "Failed to allocate with max address limit.");
    let allocation = result.unwrap();
    let address = allocation.into_raw_ptr::<u8>().unwrap() as usize;
    u_assert!((address + UEFI_PAGE_SIZE - 1) <= max_address, "Allocated address exceeds max address limit.");

    // Get an allocator.
    let result = mm.get_allocator(EfiMemoryType::BootServicesData);
    u_assert!(result.is_ok(), "Failed to get allocator.");
    let allocator = result.unwrap();

    // Allocate and free a simple structure using the allocator.
    let boxed_struct = Box::new_in(42, allocator);
    u_assert_eq!(*boxed_struct, 42, "Failed to allocate boxed struct from allocator.");
    drop(boxed_struct);

    // Get a dynamic allocator.
    let result = mm.get_allocator(EfiMemoryType::ACPIReclaimMemory);
    u_assert!(result.is_ok(), "Failed to get dynamic allocator.");
    let allocator = result.unwrap();

    // Allocate and free a simple structure using the allocator.
    let boxed_struct = Box::new_in(42, allocator);
    u_assert_eq!(*boxed_struct, 42, "Failed to allocate boxed struct from dynamic allocator.");
    drop(boxed_struct);

    Ok(())
}

#[patina_test]
fn memory_manager_attributes_test(mm: Service<dyn MemoryManager>) -> patina::test::Result {
    // The default attributes for memory should be read/write.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let address = allocation.into_raw_ptr::<u8>().unwrap() as usize;
    let result = mm.get_page_attributes(address, 1);
    u_assert!(result.is_ok(), "Failed to get original page attributes.");
    let (access, caching) = result.unwrap();
    u_assert_eq!(access, AccessType::ReadWrite, "Allocation did not return Read/Write access.");

    // Test changing the attributes to read only.
    let result = unsafe { mm.set_page_attributes(address, 1, AccessType::ReadOnly, None) };
    u_assert!(result.is_ok(), "Failed to set page attributes.");
    let result = mm.get_page_attributes(address, 1);
    u_assert!(result.is_ok(), "Failed to get altered page attributes.");
    let (access, new_caching) = result.unwrap();
    u_assert_eq!(access, AccessType::ReadOnly, "Allocation did not return ReadOnly access.");
    u_assert_eq!(new_caching, caching, "Caching type changes unexpectedly.");

    // Free the page
    let result = unsafe { mm.free_pages(address, 1) };
    u_assert!(result.is_ok(), "Failed to free page.");

    Ok(())
}
