//! GCD Paging Interfaces
//!
//! This module contains the interfaces for the memory GCD to interact with the page table.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use super::{
    AllocateType, Box, CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID, CacheAttributeValue, DEFAULT_ALLOCATION_STRATEGY,
    EVENT_DB, EfiError, GCD, GcdMemoryType, Hob, HobList, MemoryAttributes, MemoryProtectionPolicy, PageAllocator,
    PatinaPageTable, PtError, SIZE_4GB, SpinLockedGcd, UEFI_PAGE_MASK, UEFI_PAGE_SHIFT, UEFI_PAGE_SIZE, UefiPeInfo,
    Vec, align_up, base_guids, dxe_services, efi, hob, pi_guids, protocol_db, uefi_pages_to_size,
};

const PAGE_POOL_CAPACITY: usize = 512;

#[derive(Debug)]
pub(crate) struct PagingAllocator<'a> {
    page_pool: Vec<efi::PhysicalAddress>,
    gcd: &'a SpinLockedGcd,
}

impl<'a> PagingAllocator<'a> {
    pub(crate) fn new(gcd: &'a SpinLockedGcd) -> Self {
        Self { page_pool: Vec::with_capacity(PAGE_POOL_CAPACITY), gcd }
    }
}

impl PageAllocator for PagingAllocator<'_> {
    fn allocate_page(&mut self, align: u64, size: u64, is_root: bool) -> Result<u64, PtError> {
        if align != UEFI_PAGE_SIZE as u64 || size != UEFI_PAGE_SIZE as u64 {
            log::error!("Invalid alignment or size for page allocation: align: {align:#x}, size: {size:#x}");
            return Err(PtError::InvalidParameter);
        }

        if is_root {
            // allocate 1 page
            let len = 1;
            // allocate under 4GB to support x86 MPServices
            let addr: u64 = (SIZE_4GB - 1) as u64;

            // if this is the root page, we need to allocate it under 4GB to support x86 MPServices, they will copy
            // the cr3 register to the APs and the APs come up in real mode, transition to protected mode, enable paging,
            // and then transition to long mode. This means that the root page must be under 4GB so that the 32 bit code
            // can do 32 bit register moves to move it to cr3. For other architectures, this is not necessary, but not
            // an issue to allocate. However, some architectures may not have memory under 4GB, so if we fail here,
            // simply retry with the normal allocation

            let res = self.gcd.memory.lock().allocate_memory_space(
                AllocateType::BottomUp(Some(addr as usize)),
                GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                uefi_pages_to_size!(len),
                protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                None,
            );
            if let Ok(root_page) = res {
                Ok(root_page as u64)
            } else {
                // if we failed, try again with normal allocation
                log::error!(
                    "Failed to allocate root page for the page table page pool, retrying with normal allocation"
                );

                match self.gcd.memory.lock().allocate_memory_space(
                    DEFAULT_ALLOCATION_STRATEGY,
                    GcdMemoryType::SystemMemory,
                    UEFI_PAGE_SHIFT,
                    uefi_pages_to_size!(len),
                    protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                    None,
                ) {
                    Ok(root_page) => Ok(root_page as u64),
                    Err(e) => {
                        // okay we are good and dead now
                        panic!("Failed to allocate root page for the page table page pool: {e}");
                    }
                }
            }
        } else if let Some(page) = self.page_pool.pop() {
            Ok(page)
        } else {
            // allocate 512 pages at a time
            let len = PAGE_POOL_CAPACITY;

            // we only allocate here, not map. The page table is self-mapped, so we don't have to identity
            // map them. This function is called with the page table lock held, so we cannot do that
            match self.gcd.memory.lock().allocate_memory_space(
                DEFAULT_ALLOCATION_STRATEGY,
                GcdMemoryType::SystemMemory,
                UEFI_PAGE_SHIFT,
                uefi_pages_to_size!(len),
                protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
                None,
            ) {
                Ok(addr) => {
                    for i in 0..len {
                        self.page_pool.push(addr as u64 + ((i * UEFI_PAGE_SIZE) as u64));
                    }
                    self.page_pool.pop().ok_or(PtError::OutOfResources)
                }
                Err(e) => {
                    panic!("Failed to allocate pages for the page table page pool {e}");
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(test, feature = "confidential_compute"))]
pub(crate) struct AliasedMapping {
    pub(crate) virtual_address: u64,
    pub(crate) physical_address: u64,
    pub(crate) length: u64,
    pub(crate) attributes: u64,
}

impl SpinLockedGcd {
    pub(super) fn set_paging_attributes(
        &self,
        base_address: usize,
        len: usize,
        attributes: u64,
    ) -> Result<(), EfiError> {
        if let Some(page_table) = &mut *self.page_table.lock() {
            // only apply page table attributes to the page table, not our virtual GCD attributes
            let paging_attrs = MemoryAttributes::from_bits_truncate(attributes)
                & (MemoryAttributes::AccessAttributesMask | MemoryAttributes::CacheAttributesMask);

            let mut unmapped = false;
            let mut update_cache_attributes = true;

            // EFI_MEMORY_RP is a special case, we don't actually want to set it in the page table, we want to unmap
            // the region. It is valid for the region to already be unmapped or partially unmapped in this case. E.g.
            // we might be freeing an entire image but the stack guard page is already unmapped.
            if paging_attrs & MemoryAttributes::ReadProtect == MemoryAttributes::ReadProtect {
                match page_table.unmap_memory_region(base_address as u64, len as u64) {
                    Ok(()) => {
                        log::trace!(
                            target: "paging",
                            "Memory region {base_address:#x?} of length {len:#x?} unmapped",
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to unmap memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                        );
                        debug_assert!(false);
                        return Err(EfiError::InvalidParameter);
                    }
                }
            }

            // we assume that the page table and GCD are in sync. If not, we will debug_assert and return an error here
            // as this indicates a critical error
            let region_attributes = match page_table.query_memory_region(base_address as u64, len as u64) {
                Ok(attrs) => Some(attrs),
                Err((PtError::NoMapping, attrs)) => {
                    // it is not an error if the range is fully not mapped, we just need to map it, unless we are
                    // trying to unmap the region, which we will check for below
                    unmapped = true;

                    // we capture the returned cache attributes here in order to check if we need to send the cache
                    // attribute update later
                    match attrs {
                        CacheAttributeValue::Valid(cache_attributes) => {
                            // we got valid cache attributes for an unmapped region which means we will
                            // need to check later if we need to send the cache attribute update event
                            Some(cache_attributes)
                        }
                        CacheAttributeValue::Unmapped => {
                            // region is unmapped with no cache attributes which means we will need to send
                            // the cache attribute update event
                            None
                        }
                        // this architecture only describes cache attributes in the page table, so don't send the
                        // cache attribute update event
                        CacheAttributeValue::NotSupported => {
                            update_cache_attributes = false;
                            None
                        }
                    }
                }
                Err(e) => {
                    log::error!(
                        "query memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );
                    log::error!("GCD and page table are out of sync. This is a critical error.");
                    log::info!("GCD {GCD}");
                    debug_assert!(false);
                    return Err(EfiError::InvalidParameter);
                }
            };

            // if this region already has the attributes we want, we don't need to do anything
            // in the page table.
            if let Some(region_attrs) = region_attributes
                && (region_attrs & (MemoryAttributes::AccessAttributesMask | MemoryAttributes::CacheAttributesMask))
                    == paging_attrs
                && !unmapped
            {
                log::trace!(
                    target: "paging",
                    "Memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. No paging action taken: Region already mapped with these attributes.",
                );
                return Ok(());
            }

            match page_table.map_memory_region(base_address as u64, len as u64, paging_attrs) {
                Ok(()) => {
                    let new_cache_attributes = paging_attrs & MemoryAttributes::CacheAttributesMask;
                    let old_cache_attributes =
                        region_attributes.map(|attrs| attrs & MemoryAttributes::CacheAttributesMask);

                    // if the cache attributes changed, we need to publish an event, as some architectures
                    // (such as x86) need to populate APs with the caching information
                    if new_cache_attributes != MemoryAttributes::empty() && update_cache_attributes {
                        if let Some(old_cache_attrs) = old_cache_attributes
                            && old_cache_attrs != new_cache_attributes
                        {
                            // Some cache maintenance may be required after all attributes have been applied to clean
                            // up any stale cache entries before the memory is accessed. This is done after the attributes
                            // were applied to ensure no new unexpected cache lines are created.
                            page_table.handle_cacheability_change(
                                base_address as u64,
                                len as u64,
                                old_cache_attrs,
                                new_cache_attributes,
                            );

                            // in this case, we had caching attributes for this region and they do not match the newly
                            // set attributes
                            log::trace!(
                                target: "paging",
                                "Cache attributes for memory region {base_address:#x?} of length {len:#x?} were updated to {new_cache_attributes:#x?} from {old_cache_attrs:#x?}, sending cache attributes changed event",
                            );

                            EVENT_DB.signal_group(CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID.into_inner());
                        } else if unmapped && old_cache_attributes.is_none() {
                            // in this case the region was unmapped and we had no caching attributes set up
                            log::trace!(
                                target: "paging",
                                "Cache attributes for memory region {base_address:#x?} of length {len:#x?} were updated to {new_cache_attributes:#x?} from an unmapped state, sending cache attributes changed event",
                            );

                            EVENT_DB.signal_group(CACHE_ATTRIBUTE_CHANGE_EVENT_GROUP_GUID.into_inner());
                        }
                    }

                    log::trace!(
                        target: "paging",
                        "Memory region {base_address:#x?} of length {len:#x?} mapped with attributes {paging_attrs:#x?}",
                    );
                    Ok(())
                }
                Err(e) => {
                    // this indicates the GCD and page table are out of sync
                    log::error!(
                        "Failed to map memory region {base_address:#x?} of length {len:#x?} with attributes {attributes:#x?}. Status: {e:#x?}",
                    );

                    debug_assert!(false);
                    match e {
                        PtError::OutOfResources => Err(EfiError::OutOfResources),
                        PtError::NoMapping => Err(EfiError::NotFound),
                        _ => Err(EfiError::InvalidParameter),
                    }
                }
            }
        } else {
            // if we don't have the page table, we shouldn't panic, this may just be the case that we are allocating
            // the initial GCD memory space and we haven't initialized the page table yet
            Err(EfiError::NotReady)
        }
    }

    #[cfg(any(test, feature = "confidential_compute"))]
    pub(crate) fn map_aliased_memory_region(
        &self,
        virtual_address: u64,
        physical_address: u64,
        len: u64,
        attributes: u64,
    ) -> Result<(), EfiError> {
        let paging_attrs = MemoryAttributes::from_bits_truncate(attributes)
            & (MemoryAttributes::AccessAttributesMask | MemoryAttributes::CacheAttributesMask);

        let mut page_table_guard = self.page_table.lock();
        self.aliased_mappings.lock().try_reserve(1).map_err(|_| EfiError::OutOfResources)?;
        let page_table = page_table_guard.as_mut().ok_or(EfiError::NotReady)?;

        page_table.map_aliased_memory_region(virtual_address, physical_address, len, paging_attrs).map_err(|err| {
            match err {
                PtError::OutOfResources => EfiError::OutOfResources,
                PtError::NoMapping => EfiError::NoMapping,
                _ => EfiError::InvalidParameter,
            }
        })?;

        self.aliased_mappings.lock().push(AliasedMapping {
            virtual_address,
            physical_address,
            length: len,
            attributes,
        });
        Ok(())
    }

    #[cfg(any(test, feature = "confidential_compute"))]
    pub(crate) fn unmap_aliased_memory_region(&self, virtual_address: u64, len: u64) -> Result<(), EfiError> {
        if !self
            .aliased_mappings
            .lock()
            .iter()
            .any(|mapping| mapping.virtual_address == virtual_address && mapping.length == len)
        {
            return Err(EfiError::NotFound);
        }

        let mut page_table_guard = self.page_table.lock();
        let page_table = page_table_guard.as_mut().ok_or(EfiError::NotReady)?;
        page_table.unmap_memory_region(virtual_address, len).map_err(|err| match err {
            PtError::OutOfResources => EfiError::OutOfResources,
            PtError::NoMapping => EfiError::NoMapping,
            _ => EfiError::InvalidParameter,
        })?;

        self.aliased_mappings
            .lock()
            .retain(|mapping| mapping.virtual_address != virtual_address || mapping.length != len);
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn get_aliased_mappings(&self) -> Vec<AliasedMapping> {
        self.aliased_mappings.lock().clone()
    }

    // Take control of our own destiny and create a page table that the GCD controls
    // This must be done after the GCD is initialized and memory services are available,
    // as we need to allocate memory for the page table structure.
    // This function always uses the GCD functions to map the page table so that the GCD remains in sync with the
    // changes here (setting XP)
    pub(crate) fn init_paging_with(&self, hob_list: &HobList, page_table: Box<dyn PatinaPageTable>) {
        log::info!("Initializing paging for the GCD");

        *self.page_table.lock() = Some(page_table);

        let mut mmio_res_descs: Vec<dxe_services::MemorySpaceDescriptor> =
            Vec::with_capacity(self.memory_descriptor_count() + 10);
        self.memory
            .lock()
            .get_memory_descriptors(mmio_res_descs.as_mut(), |d, _| {
                matches!(d.memory_type, GcdMemoryType::MemoryMappedIo | GcdMemoryType::Reserved)
            })
            .expect("Failed to get MMIO descriptors!");

        // Before we install this page table, we need to ensure that DXE Core is mapped correctly here as well as any
        // allocated memory and MMIO. All other memory will be unmapped initially. Do allocated memory first, then the
        // DXE Core, so that we can ensure that the DXE Core is mapped correctly and not overwritten by the allocated
        // memory attrs. We also need to preallocate memory here so that we do not allocate memory after getting the
        // descriptors
        let mut descriptors: Vec<dxe_services::MemorySpaceDescriptor> =
            Vec::with_capacity(self.memory_descriptor_count() + 10);
        self.memory
            .lock()
            .get_memory_descriptors(&mut descriptors, |d, allocated| {
                if d.memory_type == GcdMemoryType::MemoryMappedIo || d.memory_type == GcdMemoryType::Reserved {
                    // we've already handled MMIO and reserved memory, so skip these
                    return false;
                }
                allocated
            })
            .expect("Failed to get allocated memory descriptors!");

        // now map the memory regions, keeping any cache attributes set in the GCD descriptors
        for desc in descriptors {
            log::trace!(
                target: "paging",
                "Mapping memory region {:#x?} of length {:#x?} with attributes {:#x?}",
                desc.base_address,
                desc.length,
                desc.attributes
            );

            if let Err(err) = self.set_memory_space_attributes(
                desc.base_address as usize,
                desc.length as usize,
                GCD.memory_protection_policy
                    .apply_allocated_memory_protection_policy(desc.attributes, desc.memory_type),
            ) {
                // if we fail to set these attributes (which should just be XP at this point), we should try to
                // continue
                log::error!(
                    "Failed to map memory region {:#x?} of length {:#x?} with attributes {:#x?}. Error: {:?}",
                    desc.base_address,
                    desc.length,
                    desc.attributes,
                    err
                );
                debug_assert!(false);
            }
        }

        // Retrieve the MemoryAllocationModule hob corresponding to the DXE core so that we can map it correctly
        let dxe_core_hob = hob_list
            .iter()
            .find_map(|x| match x {
                Hob::MemoryAllocationModule(module) if module.module_name == base_guids::DXE_CORE_ID => Some(module),
                _ => None,
            })
            .expect("Did not find MemoryAllocationModule Hob for DxeCore. Use patina::guid::DXE_CORE_ID as FFS GUID.");

        // SAFETY: the DXE core HOB points to the loaded image buffer and size.
        let pe_info = unsafe {
            UefiPeInfo::parse_mapped(core::slice::from_raw_parts(
                dxe_core_hob.alloc_descriptor.memory_base_address as *const u8,
                dxe_core_hob.alloc_descriptor.memory_length as usize,
            ))
            .expect("Failed to parse PE info for DXE Core")
        };

        let dxe_core_desc = match self
            .get_memory_descriptor_for_address(dxe_core_hob.alloc_descriptor.memory_base_address, |d, _| {
                d.memory_type != GcdMemoryType::NonExistent
            }) {
            Ok(desc) => desc,
            Err(e) => panic!("DXE Core not mapped in GCD {e:?}"),
        };

        // map the entire image as RW, as the PE headers don't live in the sections
        self.set_memory_space_attributes(
            dxe_core_hob.alloc_descriptor.memory_base_address as usize,
            dxe_core_hob.alloc_descriptor.memory_length as usize,
            GCD.memory_protection_policy
                .apply_allocated_memory_protection_policy(dxe_core_desc.attributes, dxe_core_desc.memory_type),
        )
        .unwrap_or_else(|_| {
            panic!(
                "Failed to map DXE Core image {:#x?} of length {:#x?}",
                dxe_core_hob.alloc_descriptor.memory_base_address, dxe_core_hob.alloc_descriptor.memory_length
            )
        });

        // now map each section with the correct image protections
        for section in pe_info.sections {
            // each section starts at image_base + virtual_address, per PE/COFF spec.
            let section_base_address =
                dxe_core_hob.alloc_descriptor.memory_base_address + u64::from(section.virtual_address);
            let (attributes, _) =
                MemoryProtectionPolicy::apply_image_protection_policy(section.characteristics, &dxe_core_desc);

            // We need to use the virtual size for the section length, but
            // we cannot rely on this to be section aligned, as some compilers rely on the loader to align this
            let aligned_virtual_size = match align_up(section.virtual_size, pe_info.section_alignment) {
                Ok(size) => u64::from(size),
                Err(_) => {
                    panic!(
                        "Failed to align section size {:#x?} with alignment {:#x?}",
                        section.virtual_size, pe_info.section_alignment
                    );
                }
            };

            log::trace!(
                target: "paging",
                "Mapping DXE Core image memory region {section_base_address:#x?} of length {aligned_virtual_size:#x?} with attributes {attributes:#x?}",
            );

            self.set_memory_space_attributes(section_base_address as usize, aligned_virtual_size as usize, attributes)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to map DXE Core image {:#x?} of length {:#x?} with attributes {:#x?}.",
                        dxe_core_hob.alloc_descriptor.memory_base_address,
                        dxe_core_hob.alloc_descriptor.memory_length,
                        attributes
                    )
                });
        }

        // now map MMIO. Drivers expect to be able to access MMIO regions as RW, so we need to map them as such
        for desc in mmio_res_descs {
            // MMIO is not necessarily described at page granularity, but needs to be mapped as such in the page
            // table
            let base_address = desc.base_address as usize & !UEFI_PAGE_MASK;
            let len = (desc.length as usize + UEFI_PAGE_MASK) & !UEFI_PAGE_MASK;
            let new_attributes = GCD
                .memory_protection_policy
                .apply_allocated_memory_protection_policy(desc.attributes, desc.memory_type);

            log::trace!(
                target: "paging",
                "Mapping {:?} region {:#x?} of length {:#x?} with attributes {:#x?}",
                desc.memory_type,
                base_address,
                len,
                new_attributes
            );

            if let Err(err) = self.set_memory_space_attributes(base_address, len, new_attributes) {
                // if we fail to set these attributes we may or may not be able to continue to boot. It depends on
                // if a driver attempts to touch this MMIO region
                log::error!(
                    "Failed to map {:?} region {:#x?} of length {:#x?} with attributes {:#x?}. Error: {:?}",
                    desc.memory_type,
                    base_address,
                    len,
                    new_attributes,
                    err
                );
                debug_assert!(false);
            }
        }

        // Find the stack hob and set attributes.
        if let Some(stack_hob) = hob_list.iter().find_map(|x| match x {
            patina::pi::hob::Hob::MemoryAllocation(hob::MemoryAllocation { header: _, alloc_descriptor: desc })
                if desc.name == pi_guids::MEMORY_ALLOC_STACK_HOB_GUID =>
            {
                Some(desc)
            }
            _ => None,
        }) {
            log::trace!(
                "Found stack hob {:#X?} of length {:#X?}",
                stack_hob.memory_base_address,
                stack_hob.memory_length
            );
            let stack_address = stack_hob.memory_base_address;
            let stack_length = stack_hob.memory_length;

            assert!(
                stack_address != 0 && stack_length != 0,
                "Invalid Stack Configuration: Stack base address {stack_address:#X} for len {stack_length:#X}"
            );

            if let Ok(gcd_desc) = self
                .get_memory_descriptor_for_address(stack_address, |d, _| d.memory_type != GcdMemoryType::NonExistent)
            {
                // Set Stack region to execute protect. We use the allocated memory protection policy here because
                // that matches our standard policy
                let attributes = self
                    .memory_protection_policy
                    .apply_allocated_memory_protection_policy(gcd_desc.attributes, gcd_desc.memory_type);
                match self.set_memory_space_attributes(stack_address as usize, stack_length as usize, attributes) {
                    Ok(()) | Err(EfiError::NotReady) => (),
                    Err(e) => {
                        log::error!(
                            "Could not set NX for memory address {stack_address:#X} for len {stack_length:#X} with error {e:?}"
                        );
                        debug_assert!(false);
                    }
                }
                // Set Guard page to read protect. We keep the NX and cache attributes from above
                match self.set_memory_space_attributes(
                    stack_address as usize,
                    UEFI_PAGE_SIZE,
                    MemoryProtectionPolicy::apply_image_stack_guard_policy(attributes),
                ) {
                    Ok(()) | Err(EfiError::NotReady) => (),
                    Err(e) => {
                        log::error!(
                            "Could not set RP for memory address {stack_address:#X} for len {UEFI_PAGE_SIZE:#X} with error {e:?}"
                        );
                        debug_assert!(false);
                    }
                }
            } else {
                panic!(
                    "Stack memory region {:#X?} of length {:#X?} not found in GCD",
                    stack_hob.memory_base_address, stack_hob.memory_length
                );
            }
        } else {
            panic!("No stack hob found");
        }

        // make sure we didn't map page 0 if it was reserved or MMIO, we are using this for null pointer detection
        // only do this if page 0 actually exists
        if let Ok(descriptor) =
            self.get_memory_descriptor_for_address(0, |d, _| d.memory_type != GcdMemoryType::NonExistent)
            && let Err(err) = self.set_memory_space_attributes(
                0,
                UEFI_PAGE_SIZE,
                MemoryProtectionPolicy::apply_null_page_policy(descriptor.attributes),
            )
        {
            // if we fail to set these attributes we can continue to boot, but we will not be able to detect null
            // pointer dereferences.
            log::error!("Failed to unmap page 0, which is reserved for null pointer detection. Error: {err}");
            debug_assert!(false);
        }

        self.page_table.lock().as_mut().unwrap().install_page_table().expect("Failed to install the page table");

        log::info!("Paging initialized for the GCD");
    }
}
