use alloc::boxed::Box;
use patina_sdk::test::patina_test;
use patina_sdk::{
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
    uefi_pages_to_size,
};
use patina_sdk::{u_assert, u_assert_eq};
use r_efi::efi;

use crate::{
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
        // TODO: Because the allocator has to live for a undefined amount of time
        // to allow for freeing the memory, we can only use the static allocators.
        // This should be changed in the future.
        let allocator = match memory_type {
            EfiMemoryType::LoaderCode => &crate::allocator::EFI_LOADER_CODE_ALLOCATOR,
            EfiMemoryType::BootServicesCode => &crate::allocator::EFI_BOOT_SERVICES_CODE_ALLOCATOR,
            EfiMemoryType::BootServicesData => &crate::allocator::EFI_BOOT_SERVICES_DATA_ALLOCATOR,
            EfiMemoryType::RuntimeServicesCode => &crate::allocator::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR,
            EfiMemoryType::RuntimeServicesData => &crate::allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
            _ => {
                return Err(MemoryError::UnsupportedMemoryType);
            }
        };
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

        if address % UEFI_PAGE_SIZE != 0 {
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

        let mut current_base: u64 = address as u64;
        let range_end: u64 = (address + uefi_pages_to_size!(page_count)) as u64;
        while current_base < range_end {
            let descriptor =
                match crate::dxe_services::core_get_memory_space_descriptor(current_base as efi::PhysicalAddress) {
                    Ok(descriptor) => descriptor,
                    Err(e) => {
                        log::error!("Memory descriptor fetching failed with error {:#x?} for {:#x}", e, current_base,);
                        return Err(MemoryError::InvalidAddress);
                    }
                };
            let descriptor_end = descriptor.base_address + descriptor.length;

            // it is still legal to split a descriptor and only set the attributes on part of it
            let next_base = u64::min(descriptor_end, range_end);
            let current_len = next_base - current_base;

            // Always clear all access attributes and set the requested ones.
            let mut new_attributes = descriptor.attributes & !efi::MEMORY_ACCESS_MASK;
            new_attributes |= access_attributes;

            // If no cache attributes were requested, leave them unchanged.
            if let Some(cache_attributes) = cache_attributes {
                new_attributes &= !efi::CACHE_ATTRIBUTE_MASK;
                new_attributes |= cache_attributes;
            }

            match dxe_services::core_set_memory_space_attributes(current_base, current_len, new_attributes) {
                Ok(_) => {}
                Err(_) => return Err(MemoryError::InternalError),
            };
            current_base = next_base;
        }

        Ok(())
    }

    fn get_page_attributes(&self, address: usize, page_count: usize) -> Result<(AccessType, CachingType), MemoryError> {
        if page_count == 0 {
            return Err(MemoryError::InvalidPageCount);
        }

        if address % UEFI_PAGE_SIZE != 0 {
            return Err(MemoryError::UnalignedAddress);
        }

        let base_address = address as efi::PhysicalAddress;
        let length = uefi_pages_to_size!(page_count) as u64;
        let attributes = match dxe_services::core_get_memory_space_descriptor(base_address) {
            Ok(descriptor) => {
                if base_address + length > descriptor.base_address + descriptor.length {
                    log::error!("Inconsistent attributes for: base_address {:#x} length {:#x}", base_address, length);
                    return Err(MemoryError::InconsistentRangeAttributes);
                }
                descriptor.attributes
            }
            Err(status) => {
                log::error!("Failed to get memory descriptor for address {:#x}: {:?}", base_address, status,);
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
fn memory_manager_allocations_test(mm: Service<dyn MemoryManager>) -> patina_sdk::test::Result {
    // Allocate a page, and make sure it is accessible.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let mut data = allocation.into_boxed_slice::<u8>();
    u_assert_eq!(data.len(), UEFI_PAGE_SIZE, "Failed to free page.");
    data[0] = 42;
    drop(data);

    // allocate a page, free it then allocate the address.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let address = allocation.into_raw_ptr::<u8>() as usize;
    let result = unsafe { mm.free_pages(address, 1) };
    u_assert!(result.is_ok(), "Failed to free page.");
    let result = mm.allocate_pages(1, AllocationOptions::new().with_strategy(PageAllocationStrategy::Address(address)));
    u_assert!(result.is_ok(), "Failed to allocate page by address");
    u_assert_eq!(result.unwrap().into_raw_ptr::<u8>() as usize, address, "Failed to allocate correct address");

    // Allocate an aligned address.
    const TEST_ALIGNMENT: usize = 0x400000;
    let result = mm.allocate_pages(8, AllocationOptions::new().with_alignment(TEST_ALIGNMENT));
    u_assert!(result.is_ok(), "Failed to allocate single aligned pages.");
    let allocation = result.unwrap();
    u_assert_eq!(allocation.page_count(), 8);
    let address = allocation.into_raw_ptr::<u8>() as usize;
    u_assert_eq!(address % TEST_ALIGNMENT, 0, "Allocated page not correctly aligned.");
    let result = unsafe { mm.free_pages(address, 8) };
    u_assert!(result.is_ok(), "Failed to free page.");

    // Get an allocator
    let result = mm.get_allocator(EfiMemoryType::BootServicesData);
    u_assert!(result.is_ok(), "Failed to free page.");
    let allocator = result.unwrap();

    // Allocate and free a simple structure using the allocator.
    let boxed_struct = Box::new_in(42, allocator);
    u_assert_eq!(*boxed_struct, 42, "Failed to allocate boxed struct.");
    drop(boxed_struct);

    Ok(())
}

#[patina_test]
fn memory_manager_attributes_test(mm: Service<dyn MemoryManager>) -> patina_sdk::test::Result {
    // The default attributes for memory should be read/write.
    let result = mm.allocate_pages(1, AllocationOptions::new());
    u_assert!(result.is_ok(), "Failed to allocate single page.");
    let allocation = result.unwrap();
    let address = allocation.into_raw_ptr::<u8>() as usize;
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
