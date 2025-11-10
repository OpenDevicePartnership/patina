//! Memory Allocator
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
mod fixed_size_block_allocator;
mod uefi_allocator;

#[cfg(test)]
#[coverage(off)]
mod usage_tests;

use core::{
    ffi::c_void,
    fmt::Debug,
    mem,
    ops::Range,
    ptr::NonNull,
    slice::{self, from_raw_parts_mut},
};

extern crate alloc;
use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use mu_rust_helpers::function;

use crate::{
    GCD, config_tables,
    gcd::{self, AllocateType as AllocationStrategy},
    memory_attributes_table::MemoryAttributesTable,
    protocol_db::{self, INVALID_HANDLE},
    protocols::PROTOCOL_DB,
    systemtables::EfiSystemTable,
    tpl_lock,
};
use patina::pi::{
    dxe_services::{self, GcdMemoryType, MemorySpaceDescriptor},
    hob::{self, EFiMemoryTypeInformation, Hob, HobList, MEMORY_TYPE_INFO_HOB_GUID},
};
use r_efi::{efi, system::TPL_HIGH_LEVEL};
pub use uefi_allocator::UefiAllocator;

use patina::{
    base::{SIZE_4KB, UEFI_PAGE_MASK, UEFI_PAGE_SIZE},
    error::EfiError,
    guids::{self, HOB_MEMORY_ALLOC_STACK},
    uefi_size_to_pages,
};

// Allocation Strategy when not specified by caller.
pub const DEFAULT_ALLOCATION_STRATEGY: AllocationStrategy = AllocationStrategy::TopDown(None);

// Private tracking guid used to generate new handles for allocator tracking
// {9D1FA6E9-0C86-4F7F-A99B-DD229C9B3893}
const PRIVATE_ALLOCATOR_TRACKING_GUID: efi::Guid =
    efi::Guid::from_fields(0x9d1fa6e9, 0x0c86, 0x4f7f, 0xa9, 0x9b, &[0xdd, 0x22, 0x9c, 0x9b, 0x38, 0x93]);

pub(crate) const DEFAULT_PAGE_ALLOCATION_GRANULARITY: usize = SIZE_4KB;

// Per the UEFI spec, AARCH64 runtime pages need to be allocated on 64KB boundaries in units of 64KB to accommodate
// OSes that use 16KB or 64KB page sizes. Other architectures use 4KB pages, so we don't have any additional
// granularity requirements for them.
cfg_if::cfg_if! {
    if #[cfg(target_arch = "aarch64")] {
        pub(crate) const RUNTIME_PAGE_ALLOCATION_GRANULARITY: usize = patina::base::SIZE_64KB;
    } else {
        pub(crate) const RUNTIME_PAGE_ALLOCATION_GRANULARITY: usize = DEFAULT_PAGE_ALLOCATION_GRANULARITY;
    }
}

// The boot services data allocator is special as it is used as the GlobalAllocator instance for the DXE Rust core.
// This means that any rust heap allocations (e.g. Box::new()) will come from this allocator unless explicitly directed
// to a different allocator. This allocator does not need to be public since all dynamic allocations will implicitly
// allocate from it.
#[cfg_attr(target_os = "uefi", global_allocator)]
pub(crate) static EFI_BOOT_SERVICES_DATA_ALLOCATOR: UefiAllocator = UefiAllocator::new(
    &GCD,
    NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_DATA)),
    protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE,
    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
);

// The following allocators are directly used by the core. These allocators are declared static so that they can easily
// be used in the core without e.g. the overhead of acquiring a lock to retrieve them from the allocator map that all
// the other allocators use.
pub static EFI_LOADER_CODE_ALLOCATOR: UefiAllocator = UefiAllocator::new(
    &GCD,
    NonNull::from_ref(GCD.memory_type_info(efi::LOADER_CODE)),
    protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE,
    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
);

pub static EFI_BOOT_SERVICES_CODE_ALLOCATOR: UefiAllocator = UefiAllocator::new(
    &GCD,
    NonNull::from_ref(GCD.memory_type_info(efi::BOOT_SERVICES_CODE)),
    protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE,
    DEFAULT_PAGE_ALLOCATION_GRANULARITY,
);

// This needs to call MemoryAttributesTable::install on allocation/deallocation, hence having the real callback
// passed in
pub static EFI_RUNTIME_SERVICES_CODE_ALLOCATOR: UefiAllocator = UefiAllocator::new(
    &GCD,
    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_CODE)),
    protocol_db::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR_HANDLE,
    RUNTIME_PAGE_ALLOCATION_GRANULARITY,
);

// This needs to call MemoryAttributesTable::install on allocation/deallocation, hence having the real callback
// passed in
pub static EFI_RUNTIME_SERVICES_DATA_ALLOCATOR: UefiAllocator = UefiAllocator::new(
    &GCD,
    NonNull::from_ref(GCD.memory_type_info(efi::RUNTIME_SERVICES_DATA)),
    protocol_db::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR_HANDLE,
    RUNTIME_PAGE_ALLOCATION_GRANULARITY,
);

static STATIC_ALLOCATORS: &[&UefiAllocator] = &[
    &EFI_LOADER_CODE_ALLOCATOR,
    &EFI_BOOT_SERVICES_CODE_ALLOCATOR,
    &EFI_BOOT_SERVICES_DATA_ALLOCATOR,
    &EFI_RUNTIME_SERVICES_CODE_ALLOCATOR,
    &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR,
];

fn memory_attributes_to_str(f: &mut core::fmt::Formatter<'_>, attributes: u64) -> core::fmt::Result {
    let mut attrs = Vec::new();
    let mut string_len = 0;

    if attributes & efi::MEMORY_UC != 0 {
        attrs.push("UC");
        string_len += 2;
    }
    if attributes & efi::MEMORY_WC != 0 {
        attrs.push("WC");
        string_len += 2;
    }
    if attributes & efi::MEMORY_WT != 0 {
        attrs.push("WT");
        string_len += 2;
    }
    if attributes & efi::MEMORY_WB != 0 {
        attrs.push("WB");
        string_len += 2;
    }
    if attributes & efi::MEMORY_UCE != 0 {
        attrs.push("UCE");
        string_len += 3;
    }
    if attributes & efi::MEMORY_WP != 0 {
        attrs.push("WP");
        string_len += 2;
    }
    if attributes & efi::MEMORY_RP != 0 {
        attrs.push("RP");
        string_len += 2;
    }
    if attributes & efi::MEMORY_XP != 0 {
        attrs.push("XP");
        string_len += 2;
    }
    if attributes & efi::MEMORY_NV != 0 {
        attrs.push("NV");
        string_len += 2;
    }
    if attributes & efi::MEMORY_MORE_RELIABLE != 0 {
        attrs.push("MR");
        string_len += 2;
    }
    if attributes & efi::MEMORY_RO != 0 {
        attrs.push("RO");
        string_len += 2;
    }
    if attributes & efi::MEMORY_SP != 0 {
        attrs.push("SP");
        string_len += 2;
    }
    if attributes & efi::MEMORY_CPU_CRYPTO != 0 {
        attrs.push("CC");
        string_len += 2;
    }
    if attributes & efi::MEMORY_RUNTIME != 0 {
        attrs.push("RT");
        string_len += 2;
    }

    if string_len + attrs.len() > 20 || attrs.is_empty() {
        write!(f, "{attributes:<#20X}")?;
        return Ok(());
    }

    write!(f, "{:<20}", attrs.join("|"))
}

fn memory_type_to_str(f: &mut core::fmt::Formatter<'_>, memory_type: efi::MemoryType) -> core::fmt::Result {
    let string = match memory_type {
        efi::RESERVED_MEMORY_TYPE => "Reserved Memory",
        efi::LOADER_CODE => "Loader Code",
        efi::LOADER_DATA => "Loader Data",
        efi::BOOT_SERVICES_CODE => "BootServicesCode",
        efi::BOOT_SERVICES_DATA => "BootServicesData",
        efi::RUNTIME_SERVICES_CODE => "RuntimeServicesCode",
        efi::RUNTIME_SERVICES_DATA => "RuntimeServicesData",
        efi::CONVENTIONAL_MEMORY => "Conventional Memory",
        efi::UNUSABLE_MEMORY => "Unusable Memory",
        efi::ACPI_RECLAIM_MEMORY => "ACPI Reclaim Memory",
        efi::ACPI_MEMORY_NVS => "ACPI Memory NVS",
        efi::MEMORY_MAPPED_IO => "Memory Mapped IO",
        efi::MEMORY_MAPPED_IO_PORT_SPACE => "Memory Mapped IO Port Space",
        efi::PAL_CODE => "PAL Code",
        efi::PERSISTENT_MEMORY => "Persistent Memory",
        _ => "Unknown Memory Type",
    };

    write!(f, "{string:<25}")
}

pub struct MemoryDescriptorSlice<'a>(pub &'a [efi::MemoryDescriptor]);

pub struct MemoryDescriptorRef<'a>(&'a efi::MemoryDescriptor);

impl Debug for MemoryDescriptorRef<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        memory_type_to_str(f, self.0.r#type)?;
        write!(f, "{:<#20X} {:<#15X} {:<#16X}", self.0.physical_start, self.0.virtual_start, self.0.number_of_pages)?;
        memory_attributes_to_str(f, self.0.attribute)?;
        Ok(())
    }
}

impl Debug for MemoryDescriptorSlice<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(
            f,
            "{:<24} {:<20} {:<15} {:<15} {:<20}",
            "Type", "Physical Start", "Virtual Start", "Number of Pages", "Attributes"
        )?;
        for descriptor in self.0 {
            writeln!(f, "{:?}", MemoryDescriptorRef(descriptor))?;
        }
        Ok(())
    }
}

#[allow(dead_code)]
/// Return a vector of the memory ranges owned by a particular allocator
/// Returns an empty vector if the memory type is not found
/// This function is used for compatibility mode code to set RWX attributes on memory ranges for Loader Code/Data,
/// but it is not specific to compatibility mode, which is why it is marked as allow(dead_code) as opposed to behind
/// the compatibility_mode_allowed feature flag. It is valid for other code to use this API in the absence of
/// compatibility mode.
pub(crate) fn get_memory_ranges_for_memory_type(memory_type: efi::MemoryType) -> Vec<Range<efi::PhysicalAddress>> {
    for allocator in ALLOCATORS.lock().iter() {
        if allocator.memory_type() == memory_type {
            return allocator.get_memory_ranges().collect();
        }
    }
    Vec::new()
}

// The following structure is used to track additional allocators that are created in response to allocation requests
// that are not satisfied by the static allocators.
static ALLOCATORS: tpl_lock::TplMutex<AllocatorMap> = AllocatorMap::new();
struct AllocatorMap {
    map: BTreeMap<efi::MemoryType, &'static UefiAllocator>,
}

impl AllocatorMap {
    const fn new() -> tpl_lock::TplMutex<Self> {
        tpl_lock::TplMutex::new(TPL_HIGH_LEVEL, AllocatorMap { map: BTreeMap::new() }, "AllocatorMapLock")
    }
}

impl AllocatorMap {
    // Returns an iterator that returns references to the static allocators followed by the custom allocators.
    fn iter(&self) -> impl Iterator<Item = &'static UefiAllocator> {
        STATIC_ALLOCATORS.iter().copied().chain(self.map.values().copied())
    }

    // Retrieves an allocator for the given memory type, creating one if it doesn't already exist.
    //
    // NOTE: the handle argument is only used if creation of a new allocator is required, and is passed here because
    // creation of the handle requires allocations and cannot be done while holding the allocator lock. An implication
    // of this is that in some race conditions, the handle specified here may not be the final handle of the allocator
    // if it has been created in a separate context asynchronously.
    //
    // Code calling this should provide a handle obtained from the result of [`handle_for_memory_type`], but should not
    // make any assumptions that this handle will be the actual handle associated with the allocator. If the "real"
    // allocator handle is required, it can be obtained with [`UefiAllocator::handle`] on the returned allocator.
    fn get_or_create_allocator(
        &mut self,
        memory_type: efi::MemoryType,
        handle: efi::Handle,
    ) -> Result<&'static UefiAllocator, EfiError> {
        if let Some(allocator) = STATIC_ALLOCATORS.iter().find(|x| x.memory_type() == memory_type) {
            return Ok(allocator);
        }
        Ok(self.get_or_create_dynamic_allocator(memory_type, handle))
    }

    // retrieves a dynamic allocator from the map and creates a new one with the given handle if it doesn't exist.
    // See note on `handle` in [`get_or_create_allocator`]
    fn get_or_create_dynamic_allocator(
        &mut self,
        memory_type: efi::MemoryType,
        handle: efi::Handle,
    ) -> &'static UefiAllocator {
        // the lock ensures exclusive access to the map, but an allocator may have been created already; so only create
        // the allocator if it doesn't yet exist for this memory type. MAT callbacks are only needed for Runtime
        // Services Code and Data, which are static allocators, so we can always do None here
        self.map.entry(memory_type).or_insert_with(|| {
            let granularity = match memory_type {
                efi::RESERVED_MEMORY_TYPE
                | efi::RUNTIME_SERVICES_CODE
                | efi::RUNTIME_SERVICES_DATA
                | efi::ACPI_MEMORY_NVS => RUNTIME_PAGE_ALLOCATION_GRANULARITY,
                _ => UEFI_PAGE_SIZE,
            };

            // If this is one of the memory types tracked by the system table, we will use the memory type info struct
            // from the GCD. Otherwise, we will just leak a new memory type info struct with the given memory type and
            // have the allocator use it.
            let memory_type_info = if (memory_type as usize) <= GCD.memory_type_info_table().len() {
                NonNull::from_ref(GCD.memory_type_info(memory_type))
            } else {
                NonNull::from_ref(Box::leak(Box::new(EFiMemoryTypeInformation { memory_type, number_of_pages: 0 })))
            };

            Box::leak(Box::new(UefiAllocator::new(&GCD, memory_type_info, handle, granularity)))
        })
    }

    // retrieves an allocator if it exists
    #[cfg(test)]
    fn get_allocator(&self, memory_type: efi::MemoryType) -> Option<&UefiAllocator> {
        self.iter().find(|x| x.memory_type() == memory_type)
    }

    //Returns a handle for the given memory type.
    // Handles are sourced from several places (in order).
    // 1. Well-known handles.
    // 2. The handle of an active allocator without a well-known handle that matches the memory type.
    // 3. A freshly created handle.
    //
    // Note: this routine is used to generate new handles for the creation of allocators as needed; this means that an
    // Ok() result from this routine doesn't necessarily guarantee that an allocator associated with this handle exists or
    // memory type exists.
    fn handle_for_memory_type(memory_type: efi::MemoryType) -> Result<efi::Handle, EfiError> {
        match memory_type {
            efi::RESERVED_MEMORY_TYPE => Ok(protocol_db::RESERVED_MEMORY_ALLOCATOR_HANDLE),
            efi::LOADER_CODE => Ok(protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE),
            efi::LOADER_DATA => Ok(protocol_db::EFI_LOADER_DATA_ALLOCATOR_HANDLE),
            efi::BOOT_SERVICES_CODE => Ok(protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE),
            efi::BOOT_SERVICES_DATA => Ok(protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE),
            efi::ACPI_RECLAIM_MEMORY => Ok(protocol_db::EFI_ACPI_RECLAIM_MEMORY_ALLOCATOR_HANDLE),
            efi::ACPI_MEMORY_NVS => Ok(protocol_db::EFI_ACPI_MEMORY_NVS_ALLOCATOR_HANDLE),
            // Check to see if it is an invalid type. Memory types efi::PERSISTENT_MEMORY and above to 0x6FFFFFFF are illegal.
            efi::PERSISTENT_MEMORY..=0x6FFFFFFF => Err(EfiError::InvalidParameter)?,
            // not a well known handle or illegal memory type - check the active allocators and create a handle if it doesn't
            // already exist.
            _ => {
                if let Some(handle) = ALLOCATORS
                    .lock()
                    .iter()
                    .find_map(|x| if x.memory_type() == memory_type { Some(x.handle()) } else { None })
                {
                    return Ok(handle);
                }
                let (handle, _) = PROTOCOL_DB.install_protocol_interface(
                    None,
                    PRIVATE_ALLOCATOR_TRACKING_GUID,
                    core::ptr::null_mut(),
                )?;
                Ok(handle)
            }
        }
    }

    fn memory_type_for_handle(&self, handle: efi::Handle) -> Option<efi::MemoryType> {
        self.iter().find_map(|x| if x.handle() == handle { Some(x.memory_type()) } else { None })
    }

    // resets the ALLOCATOR map to empty and resets the static allocators.
    #[cfg(test)]
    unsafe fn reset(&mut self) {
        self.map.clear();
        for allocator in STATIC_ALLOCATORS.iter() {
            allocator.reset();
        }
    }
}

#[cfg(target_os = "uefi")]
#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}

extern "efiapi" fn allocate_pool(pool_type: efi::MemoryType, size: usize, buffer: *mut *mut c_void) -> efi::Status {
    if buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    match core_allocate_pool(pool_type, size) {
        Err(err) => err.into(),
        // Safety: caller must ensure that buffer is a valid pointer. It is null-checked above.
        Ok(allocation) => unsafe {
            buffer.write_unaligned(allocation);
            efi::Status::SUCCESS
        },
    }
}

pub fn core_allocate_pool(pool_type: efi::MemoryType, size: usize) -> Result<*mut c_void, EfiError> {
    // It is not valid to attempt to allocate these memory types
    if matches!(pool_type, efi::CONVENTIONAL_MEMORY | efi::PERSISTENT_MEMORY | efi::UNACCEPTED_MEMORY_TYPE) {
        return Err(EfiError::InvalidParameter);
    }

    let handle = AllocatorMap::handle_for_memory_type(pool_type)?;
    match ALLOCATORS.lock().get_or_create_allocator(pool_type, handle) {
        Ok(allocator) => {
            let mut buffer: *mut c_void = core::ptr::null_mut();

            unsafe { allocator.allocate_pool(size, core::ptr::addr_of_mut!(buffer)).map(|_| buffer) }
        }
        Err(err) => Err(err),
    }
}

extern "efiapi" fn free_pool(buffer: *mut c_void) -> efi::Status {
    match core_free_pool(buffer) {
        Ok(_) => efi::Status::SUCCESS,
        Err(status) => status.into(),
    }
}

pub fn core_free_pool(buffer: *mut c_void) -> Result<(), EfiError> {
    if buffer.is_null() {
        return Err(EfiError::InvalidParameter);
    }
    let allocators = ALLOCATORS.lock();
    unsafe {
        if allocators.iter().any(|allocator| allocator.free_pool(buffer).is_ok()) {
            Ok(())
        } else {
            Err(EfiError::InvalidParameter)
        }
    }
}

extern "efiapi" fn allocate_pages(
    allocation_type: efi::AllocateType,
    memory_type: efi::MemoryType,
    pages: usize,
    memory: *mut efi::PhysicalAddress,
) -> efi::Status {
    match core_allocate_pages(allocation_type, memory_type, pages, memory, None) {
        Ok(_) => efi::Status::SUCCESS,
        Err(status) => status.into(),
    }
}

pub fn core_allocate_pages(
    allocation_type: efi::AllocateType,
    memory_type: efi::MemoryType,
    pages: usize,
    memory: *mut efi::PhysicalAddress,
    alignment: Option<usize>,
) -> Result<(), EfiError> {
    if memory.is_null() {
        return Err(EfiError::InvalidParameter);
    }

    // It is not valid to attempt to allocate these memory types
    if matches!(memory_type, efi::CONVENTIONAL_MEMORY | efi::PERSISTENT_MEMORY | efi::UNACCEPTED_MEMORY_TYPE) {
        return Err(EfiError::InvalidParameter);
    }

    let handle = AllocatorMap::handle_for_memory_type(memory_type)?;
    let alignment = alignment.unwrap_or(UEFI_PAGE_SIZE);

    let res = match ALLOCATORS.lock().get_or_create_allocator(memory_type, handle) {
        Ok(allocator) => {
            let result = match allocation_type {
                efi::ALLOCATE_ANY_PAGES => allocator.allocate_pages(DEFAULT_ALLOCATION_STRATEGY, pages, alignment),
                efi::ALLOCATE_MAX_ADDRESS => {
                    // Safety: caller must ensure that "memory" is a valid pointer. It is null-checked above.
                    let address = unsafe { memory.read_unaligned() };
                    allocator.allocate_pages(AllocationStrategy::TopDown(Some(address as usize)), pages, alignment)
                }
                efi::ALLOCATE_ADDRESS => {
                    // Safety: caller must ensure that "memory" is a valid pointer. It is null-checked above.
                    let address = unsafe { memory.read_unaligned() };
                    allocator.allocate_pages(AllocationStrategy::Address(address as usize), pages, alignment)
                }
                _ => Err(EfiError::InvalidParameter),
            };

            if let Ok(ptr) = result {
                // Safety: caller must ensure that "memory" is a valid pointer. It is null-checked above.
                unsafe { memory.write_unaligned(ptr.expose_provenance().get() as u64) }
                Ok(())
            } else {
                result.map(|_| ())
            }
        }
        Err(err) => Err(err),
    };

    // If the memory type is runtime services code or data, we need to install the memory attributes table to reflect
    // the update. The MAT logic will decide if it is a proper time to install the MAT or not.
    match memory_type {
        efi::RUNTIME_SERVICES_CODE | efi::RUNTIME_SERVICES_DATA => {
            if res.is_ok() {
                MemoryAttributesTable::install();
            }
        }
        _ => {}
    }

    res
}

pub fn core_get_allocator(memory_type: efi::MemoryType) -> Result<&'static UefiAllocator, EfiError> {
    let handle = AllocatorMap::handle_for_memory_type(memory_type)?;
    ALLOCATORS.lock().get_or_create_allocator(memory_type, handle)
}

extern "efiapi" fn free_pages(memory: efi::PhysicalAddress, pages: usize) -> efi::Status {
    match core_free_pages(memory, pages) {
        Ok(_) => efi::Status::SUCCESS,
        Err(status) => status.into(),
    }
}

pub fn core_free_pages(memory: efi::PhysicalAddress, pages: usize) -> Result<(), EfiError> {
    let size = match pages.checked_mul(UEFI_PAGE_SIZE) {
        Some(size) => size,
        None => return Err(EfiError::InvalidParameter),
    };

    if memory.checked_add(size as u64).is_none() {
        return Err(EfiError::InvalidParameter);
    }

    if memory.checked_rem(UEFI_PAGE_SIZE as efi::PhysicalAddress) != Some(0) {
        return Err(EfiError::InvalidParameter);
    }

    let allocators = ALLOCATORS.lock();

    let mut memory_type = efi::CONVENTIONAL_MEMORY;

    let res = unsafe {
        if allocators.iter().any(|allocator| {
            memory_type = allocator.memory_type();
            allocator.free_pages(memory as usize, pages).is_ok()
        }) {
            Ok(())
        } else {
            Err(EfiError::NotFound)
        }
    };

    // Release the lock on allocators, as it raises the TPL to TPL_HIGH_LEVEL. During the MAT install, it will attempt
    // to lock the system tables, which will result in an attempt to raise the TPL to a lower level because the system
    // tables are locked at TPL_NOTIFY
    drop(allocators);

    // If the memory type is runtime services code or data, we need to install the memory attributes table to reflect
    // the update. The MAT logic will decide if it is a proper time to install the MAT or not.
    match memory_type {
        efi::RUNTIME_SERVICES_CODE | efi::RUNTIME_SERVICES_DATA => {
            if res.is_ok() {
                MemoryAttributesTable::install();
            }
        }
        _ => {}
    }

    res
}

extern "efiapi" fn copy_mem(destination: *mut c_void, source: *mut c_void, length: usize) {
    // Safety: caller must ensure that the source and destination are valid for length bytes.
    unsafe { core::ptr::copy(source as *mut u8, destination as *mut u8, length) }
}

extern "efiapi" fn set_mem(buffer: *mut c_void, size: usize, value: u8) {
    // Safety: caller must ensure that the buffer is valid for size bytes.
    unsafe {
        let dst_buffer = from_raw_parts_mut(buffer as *mut u8, size);
        dst_buffer.fill(value);
    }
}

fn merge_blocks(
    mut previous_blocks: Vec<efi::MemoryDescriptor>,
    current: efi::MemoryDescriptor,
) -> Vec<efi::MemoryDescriptor> {
    //if current can be merged with the last block of the previous blocks, merge it.
    if let Some(descriptor) = previous_blocks.last_mut()
        && descriptor.r#type == current.r#type
        && descriptor.attribute == current.attribute
        && descriptor.physical_start + descriptor.number_of_pages * UEFI_PAGE_SIZE as u64 == current.physical_start
    {
        descriptor.number_of_pages += current.number_of_pages;
        return previous_blocks;
    }
    //otherwise, just add the new block on the end of the list.
    previous_blocks.push(current);
    previous_blocks
}

/// Get the memory map descriptors from the GCD.
///
/// ## Arguments
///
/// * `active_attributes` - Specifies whether the `attributes` field of the memory descriptors should
///   include the active attributes (true) or capabilities (false) of the memory but not
///   necessarily the active attributes. The capabilities should be used for memory map generation
///   as the UEFI specification requires.
///
pub(crate) fn get_memory_map_descriptors(active_attributes: bool) -> Result<Vec<efi::MemoryDescriptor>, EfiError> {
    let mut descriptors: Vec<MemorySpaceDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);

    // the fold operation would allocate boot services data, which we cannot do because we cannot change the memory map
    // after getting the descriptors from the GCD. We would now be invalid if we ended up overflowing a pool and getting
    // more memory from the GCD. Therefore, we need to pre-allocate memory before we get the GCD descriptors
    // to ensure we don't overflow the boot services data pool. Let's make sure we have a few extra descriptors
    let merged_descriptors: Vec<efi::MemoryDescriptor> = Vec::with_capacity(GCD.memory_descriptor_count() + 10);

    GCD.get_memory_descriptors(&mut descriptors).expect("get_memory_descriptors failed.");

    //Note: get_memory_descriptors is should already be ordered, so sort is unnecessary.
    //descriptors.sort_unstable_by(|a, b|a.physical_start.cmp(&b.physical_start));

    Ok(descriptors
        .iter()
        .filter_map(|descriptor| {
            let memory_type = ALLOCATORS.lock().memory_type_for_handle(descriptor.image_handle).or({
                match descriptor.memory_type {
                    // free memory not tracked by any allocator.
                    GcdMemoryType::SystemMemory => Some(efi::CONVENTIONAL_MEMORY),

                    // MMIO. Note: there could also be MMIO tracked by the allocators which would not hit this case.
                    GcdMemoryType::MemoryMappedIo => {
                        // we should only be returning runtime MMIO here
                        if descriptor.attributes & efi::MEMORY_RUNTIME == 0 {
                            return None;
                        }

                        Some(efi::MEMORY_MAPPED_IO)
                    }

                    // Persistent. Note: this type is not allocatable, but might be created by agents other than the core directly
                    // in the GCD.
                    GcdMemoryType::Persistent => Some(efi::PERSISTENT_MEMORY),

                    // Unaccepted. Note: this type is not allocatable, but might be created by agents other than the core directly
                    // in the GCD.
                    GcdMemoryType::Unaccepted => Some(efi::UNACCEPTED_MEMORY_TYPE),

                    // Reserved.
                    GcdMemoryType::Reserved => Some(efi::RESERVED_MEMORY_TYPE),

                    // Other memory types are ignored for purposes of the memory map
                    _ => None,
                }
            })?;

            let number_of_pages = uefi_size_to_pages!(descriptor.length as usize) as u64;
            if number_of_pages == 0 {
                debug_assert!(false, "GCD returned a memory descriptor smaller than a page.");
                return None; //skip entries for things smaller than a page
            }
            if (descriptor.base_address % UEFI_PAGE_SIZE as u64) != 0 {
                debug_assert!(false, "GCD returned a non-page-aligned memory descriptor.");
                return None; //skip entries not page aligned.
            }

            let mut attributes = match active_attributes {
                true => descriptor.attributes,
                false => {
                    // when we are building the EFI memory map, follow edk2 conventions as OSes will expect that.
                    // When using the capabilities, drop the runtime attribute and
                    // pick it up from the active attributes. We also drop the access attributes because
                    // some OSes think the EFI_MEMORY_MAP attribute field is actually set attributes, not
                    // capabilities.
                    match descriptor.memory_type {
                        GcdMemoryType::Persistent => {
                            (descriptor.capabilities & !(efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME))
                                | (descriptor.attributes & efi::MEMORY_RUNTIME)
                                | efi::MEMORY_NV
                        }
                        _ => {
                            (descriptor.capabilities & !(efi::MEMORY_ACCESS_MASK | efi::MEMORY_RUNTIME))
                                | (descriptor.attributes & efi::MEMORY_RUNTIME)
                        }
                    }
                }
            };

            if matches!(memory_type, efi::RUNTIME_SERVICES_CODE | efi::RUNTIME_SERVICES_DATA) {
                // Add the runtime attribute for runtime services code and data as
                // higher level code will expect this but it is not explicitly tracked.
                attributes |= efi::MEMORY_RUNTIME;
            }

            Some(efi::MemoryDescriptor {
                r#type: memory_type,
                physical_start: descriptor.base_address,
                virtual_start: 0,
                number_of_pages,
                attribute: attributes,
            })
        })
        .fold(merged_descriptors, merge_blocks))
}

extern "efiapi" fn get_memory_map(
    memory_map_size: *mut usize,
    memory_map: *mut efi::MemoryDescriptor,
    map_key: *mut usize,
    descriptor_size: *mut usize,
    descriptor_version: *mut u32,
) -> efi::Status {
    if memory_map_size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    if !descriptor_size.is_null() {
        // Safety: caller must ensure that descriptor_size is a valid pointer if it is not null.
        unsafe { descriptor_size.write_unaligned(mem::size_of::<efi::MemoryDescriptor>()) };
    }

    if !descriptor_version.is_null() {
        // Safety: caller must ensure that descriptor_version is a valid pointer if it is not null.
        unsafe { descriptor_version.write_unaligned(efi::MEMORY_DESCRIPTOR_VERSION) };
    }

    // Safety: caller must ensure that memory_map_size is a valid pointer. It is null-checked above.
    let map_size = unsafe { memory_map_size.read_unaligned() };

    let efi_descriptors = match get_memory_map_descriptors(false) {
        Ok(descriptors) => descriptors,
        Err(status) => return status.into(),
    };

    assert_ne!(efi_descriptors.len(), 0);

    let required_map_size = efi_descriptors.len() * mem::size_of::<efi::MemoryDescriptor>();

    // Safety: caller must ensure that memory_map_size is a valid pointer. It is null-checked above.
    unsafe { memory_map_size.write_unaligned(required_map_size) };

    if map_size < required_map_size {
        return efi::Status::BUFFER_TOO_SMALL;
    }

    if memory_map.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Rust will try to prevent an unaligned copy, given no one checks whether their points are aligned
    // treat the slice as a u8 slice and copy the bytes.
    let efi_descriptors_ptr = efi_descriptors.as_ptr() as *mut u8;

    // Safety: caller must ensure that memory_map is a valid pointer. It is null-checked above.
    //         caller must ensure that map_key is a valid pointer if it is not null.
    unsafe {
        core::ptr::copy(efi_descriptors_ptr, memory_map as *mut u8, required_map_size);

        if !map_key.is_null() {
            let memory_map_as_bytes = slice::from_raw_parts(memory_map as *mut u8, required_map_size);
            map_key.write_unaligned(crc32fast::hash(memory_map_as_bytes) as usize);
        }
    }

    log::debug!(target: "efi_memory_map", "EFI_MEMORY_MAP: \n{:?}", MemoryDescriptorSlice(&efi_descriptors));

    efi::Status::SUCCESS
}

pub fn terminate_memory_map(map_key: usize) -> Result<(), EfiError> {
    let mm_desc = get_memory_map_descriptors(false)?;
    let mm_desc_size = mm_desc.len() * mem::size_of::<efi::MemoryDescriptor>();
    let mm_desc_bytes: &[u8] = unsafe { slice::from_raw_parts(mm_desc.as_ptr() as *const u8, mm_desc_size) };

    let current_map_key = crc32fast::hash(mm_desc_bytes) as usize;
    if map_key == current_map_key { Ok(()) } else { Err(EfiError::InvalidParameter) }
}

pub fn install_memory_type_info_table(system_table: &mut EfiSystemTable) -> Result<(), EfiError> {
    let table_ptr = NonNull::from(GCD.memory_type_info_table()).cast::<c_void>().as_ptr();
    config_tables::core_install_configuration_table(guids::MEMORY_TYPE_INFORMATION, table_ptr, system_table)
}

fn process_hob_allocations(hob_list: &HobList) {
    for hob in hob_list.iter() {
        match hob {
            Hob::MemoryAllocation(hob::MemoryAllocation { header: _, alloc_descriptor: desc })
            | Hob::MemoryAllocationModule(hob::MemoryAllocationModule {
                header: _,
                alloc_descriptor: desc,
                module_name: _,
                entry_point: _,
            }) => {
                log::trace!("[{}] Processing Memory Allocation HOB:\n{:#x?}\n\n", function!(), hob);

                // Some PEI implementations generate "EfiConventionalMemory" MemoryAllocationHobs as a side effect of
                // using MemoryAllocationHob structures for memory allocation tracking in PEI. These represent "freed"
                // memory, which is the default state for memory in the GCD. So we do not need to insert them here.
                if desc.memory_type == efi::CONVENTIONAL_MEMORY {
                    log::info!(
                        "Skipping Memory Allocation HOB that represents free memory at {:#x?} of length {:#x?}.",
                        desc.memory_base_address,
                        desc.memory_length
                    );
                    continue;
                }

                if desc.memory_length == 0 {
                    log::warn!("Memory Allocation HOB has a 0 length, ignoring.\n{hob:#x?}");
                    continue;
                }

                if desc.memory_base_address == 0 {
                    log::warn!(
                        "Memory Allocation HOB has a 0 base address, ignoring. Page 0 cannot be allocated:\n{hob:#x?}"
                    );
                    continue;
                }

                //Use allocate_pages here to record these allocations and keep the allocator stats up to date.
                //Note: PI spec 1.8 III-5.4.1.1 stipulates that memory allocations must have page-granularity,
                //which allows us to use allocate_pages. Check and warn if an allocation doesn't meet the alignment
                //criteria and skip it.
                if (desc.memory_base_address & UEFI_PAGE_MASK as u64) != 0
                    || (desc.memory_length & UEFI_PAGE_MASK as u64) != 0
                {
                    log::warn!("Memory Allocation HOB has invalid address or length granularity:\n{hob:#x?}");
                    continue;
                }

                let mut address = desc.memory_base_address;
                match GCD.get_memory_descriptor_for_address(address) {
                    // we found the region in the GCD, so we can allocate it
                    Ok(gcd_desc) => {
                        if gcd_desc.base_address == desc.memory_base_address
                            && gcd_desc.length == desc.memory_length
                            && gcd_desc.image_handle != INVALID_HANDLE
                        {
                            // check to see if a duplicate HOB has already added this allocation
                            log::trace!(
                                "Duplicate allocation HOB at {:#x?} of length {:#x?}. Skipping allocation.",
                                desc.memory_base_address,
                                desc.memory_length
                            );
                            continue;
                        }
                        let alloc_res = match gcd_desc.memory_type {
                            // if this is system memory, we use core_allocate_pages to allocate it
                            // so that we can track the allocation in the allocator
                            GcdMemoryType::SystemMemory => core_allocate_pages(
                                efi::ALLOCATE_ADDRESS,
                                desc.memory_type,
                                uefi_size_to_pages!(desc.memory_length as usize),
                                &mut address as *mut efi::PhysicalAddress,
                                None,
                            ),
                            GcdMemoryType::NonExistent | GcdMemoryType::Unaccepted => {
                                // we can't allocate memory in a non-existent or unaccepted memory type
                                log::error!(
                                    "Memory Allocation HOB specifies a non-existent or unaccepted memory type: {:#x?}. Cannot allocate memory.",
                                    desc.memory_type
                                );
                                continue;
                            }
                            // for all other memory types, we can allocate it directly in the GCD
                            // because they are not managed by the allocators
                            _ => GCD
                                .allocate_memory_space(
                                    AllocationStrategy::Address(desc.memory_base_address as usize),
                                    gcd_desc.memory_type,
                                    0,
                                    desc.memory_length as usize,
                                    protocol_db::DXE_CORE_HANDLE,
                                    None,
                                )
                                .map(|_| ()),
                        };

                        if let Err(err) = alloc_res {
                            if err == EfiError::NotFound && desc.name != guids::ZERO {
                                // Guided Memory Allocation Hobs are typically MemoryAllocationModule or
                                // MemoryAllocationStack HOBs which have corresponding non-guided allocation HOBs
                                // associated with them; they are rejected as duplicates if we attempt to log them.
                                // Only log trace messages for these.
                                log::trace!(
                                    "Failed to allocate memory space for memory allocation HOB at {:#x?} of length {:#x?}. Error: {:x?}",
                                    desc.memory_base_address,
                                    desc.memory_length,
                                    err
                                );
                            } else {
                                log::error!(
                                    "Failed to allocate memory space for memory allocation HOB at {:#x?} of length {:#x?}. Error: {:x?}",
                                    desc.memory_base_address,
                                    desc.memory_length,
                                    err
                                );
                            }
                            continue;
                        }
                    }
                    Err(_) => {
                        log::error!(
                            "Failed to get memory descriptor for address {address:#x?} in GCD specified in Memory Allocation HOB:\n{hob:#x?}. Cannot allocate memory."
                        );
                        continue;
                    }
                }
            }
            Hob::FirmwareVolume(hob::FirmwareVolume { header: _, base_address, length })
            | Hob::FirmwareVolume2(hob::FirmwareVolume2 {
                header: _,
                base_address,
                length,
                fv_name: _,
                file_name: _,
            })
            | Hob::FirmwareVolume3(hob::FirmwareVolume3 {
                header: _,
                base_address,
                length,
                authentication_status: _,
                extracted_fv: _,
                fv_name: _,
                file_name: _,
            }) => {
                log::trace!("[{}] Processing Firmware Volume HOB:\n{:#x?}\n\n", function!(), hob);

                //The EDK2 C reference core maps FVs to MMIO space, but many implementations don't declare the
                //corresponding resource descriptor. Check the current region in the GCD to see whether a resource
                //descriptor of the appropriate type has been reported. If not, print a warning and skip attempting
                //to reserve it in the GCD.
                if let Ok(existing_desc) = GCD.get_memory_descriptor_for_address(*base_address)
                    && (existing_desc.memory_type != dxe_services::GcdMemoryType::MemoryMappedIo
                        || existing_desc.image_handle != INVALID_HANDLE)
                {
                    log::info!(
                        "Skipping FV HOB at {base_address:#x?} of length {length:#x?}. Containing region is not MMIO."
                    );
                    continue;
                }

                //The 4K granularity rule does not apply to FV hobs, so allocate_pages cannot be used.
                //This means they must be direct-allocated in the GCD, and no stats will be tracked for them.
                let _ = GCD.allocate_memory_space(
                    AllocationStrategy::Address(*base_address as usize),
                    dxe_services::GcdMemoryType::MemoryMappedIo,
                    0,
                    *length as usize,
                    protocol_db::DXE_CORE_HANDLE,
                    None)
                    .inspect_err(|err|{
                        log::error!(
                            "Failed to allocate memory space for firmware volume HOB at {base_address:#x?} of length {length:#x?}. Error: {err:#x?}",
                        );
                    });
            }
            _ => continue,
        };
    }

    // Find the stack hob and set attributes.
    if let Some(stack_hob) = hob_list.iter().find_map(|x| match x {
        patina::pi::hob::Hob::MemoryAllocation(hob::MemoryAllocation { header: _, alloc_descriptor: desc })
            if desc.name == HOB_MEMORY_ALLOC_STACK =>
        {
            Some(desc)
        }
        _ => None,
    }) {
        log::trace!("Found stack hob {:#X?} of length {:#X?}", stack_hob.memory_base_address, stack_hob.memory_length);
        let stack_address = stack_hob.memory_base_address;
        let stack_length = stack_hob.memory_length;

        if (stack_address == 0) || (stack_length == 0) {
            log::error!("Stack base address {:#X} for len {:#X}", stack_address, stack_length);
            debug_assert!(false);
        } else {
            match GCD.get_memory_descriptor_for_address(stack_address) {
                Ok(gcd_desc) => {
                    let attributes: u64 = gcd_desc.attributes;
                    log::trace!("Current Attributes for the stack {:#X?} \n\n", attributes);
                    // Set Stack region to execute protect.
                    match GCD.set_memory_space_attributes(
                        stack_address as usize,
                        stack_length as usize,
                        attributes | efi::MEMORY_XP,
                    ) {
                        Ok(_) | Err(EfiError::NotReady) => (),
                        Err(e) => {
                            log::error!(
                                "Could not set NX for memory address {:#X} for len {:#X} with error {:?}",
                                stack_address,
                                stack_length,
                                e
                            );
                            debug_assert!(false);
                        }
                    }
                    // Set Guard page to read protect.
                    match GCD.set_memory_space_attributes(
                        stack_address as usize,
                        UEFI_PAGE_SIZE,
                        attributes | efi::MEMORY_RP,
                    ) {
                        Ok(_) | Err(EfiError::NotReady) => (),
                        Err(e) => {
                            log::error!(
                                "Could not set RP for memory address {:#X} for len {:#X} with error {:?}",
                                stack_address,
                                UEFI_PAGE_SIZE,
                                e
                            );
                            debug_assert!(false);
                        }
                    }
                }
                Err(_) => {
                    log::error!("Failed to get memory descriptor for address {:#x?} in GCD", stack_address);
                }
            }
        }
    } else {
        debug_assert!(false, "No stack hob found\n");
    }

    // now that we've processed HOBs, lets allocate page 0 because we are going to use it for null pointer detection
    // if we don't allocate it, bootloaders may try to allocate it (as they often allocate by address from what the
    // EFI_MEMORY_MAP reports as EfiConventionalMemory), which will cause a failure that is unnecessary. We do this
    // after HOB processing because we want to ensure that the GCD is fully populated with the memory map
    // before we allocate page 0, as it may not live in system memory, in which case we cannot allocate it.
    match GCD.get_memory_descriptor_for_address(0) {
        Ok(desc) if desc.memory_type == GcdMemoryType::SystemMemory => {
            let mut address: efi::PhysicalAddress = 0;
            if core_allocate_pages(
                efi::ALLOCATE_ADDRESS,
                efi::BOOT_SERVICES_DATA,
                1,
                &mut address as *mut efi::PhysicalAddress,
                None,
            )
            .is_err()
            {
                // if we failed, we should just continue, we will still unmap page 0, but it will be possible to
                // allocate by another entity, which is dangerous.
                log::warn!(
                    "Failed to allocate page 0 for null pointer detection. It will still be unmapped but something may attempt to allocate it by address."
                );
            }
        }
        _ => {
            // if we got here, then page 0 is not part of system memory, it may be absent entirely, or the platform may
            // have this reserved or marked as MMIO. That is a dangerous configuration, because we will unmap it
            // regardless of being able to allocate it.
            log::info!(
                "Page 0 is not part of system memory, it cannot be allocated. It will still be unmapped to use for null pointer detection."
            );
        }
    }
}

/// Initializes memory support
///
/// This routine sets the boot services routines for memory allocation and does initial configuration of the allocators.
/// In particular, this includes reserving a block of pages for each allocator according to the configuration specified
/// by the platform in the form of the MEMORY_TYPE_INFO HOB. This allows the platform to reserve blocks of memory for
/// memory types that must be stable across S4 resume flows. By reserving additional space beyond what is required, the
/// memory map reported to the OS can be stable even in the face of small variations in memory from boot-to-boot, which
/// helps to avoid S4 failure due to memory map change.
///
pub fn init_memory_support(hob_list: &HobList) {
    // Add the rest of the system resources to the GCD.
    // Caution: care must be taken to ensure no allocations occur after this call but before the allocation hobs are
    // processed - otherwise they could occupy space corresponding to a pre-DXE memory allocation that has not yet been
    // reserved.
    gcd::add_hob_resource_descriptors_to_gcd(hob_list);

    // process pre-DXE allocations from the Hob list
    process_hob_allocations(hob_list);

    // After this point the GCD and existing allocations are fully processed and it is safe to arbitrarily allocate.

    // If memory type info HOB is available, then pre-allocate the corresponding buckets.
    if let Some(memory_type_info) = hob_list.iter().find_map(|x| {
        match x {
            patina::pi::hob::Hob::GuidHob(hob, data) if hob.name == MEMORY_TYPE_INFO_HOB_GUID => {
                let memory_type_slice_ptr = data.as_ptr() as *const EFiMemoryTypeInformation;
                let memory_type_slice_len = data.len() / mem::size_of::<EFiMemoryTypeInformation>();

                // Safety: this structure comes from the hob list, so it must be 8-byte aligned (meets alignment
                // requirement for EfiMemoryTypeInformation), and length is calculated above to fit within the
                // Guid HOB data. Assert if alignment is not as expected.
                assert_eq!(memory_type_slice_ptr.align_offset(mem::align_of::<EFiMemoryTypeInformation>()), 0);
                let memory_type_info = unsafe { slice::from_raw_parts(memory_type_slice_ptr, memory_type_slice_len) };

                Some(memory_type_info)
            }
            _ => None,
        }
    }) {
        for bucket in memory_type_info {
            if bucket.number_of_pages == 0 {
                continue;
            }
            log::info!(
                "Allocating memory bucket for memory type: {:#x?}, {:#x?} pages.",
                bucket.memory_type,
                bucket.number_of_pages
            );
            let handle = match AllocatorMap::handle_for_memory_type(bucket.memory_type) {
                Ok(handle) => handle,
                Err(err) => {
                    log::error!("failed to get a handle for memory type {:#x?}: {:#x?}", bucket.memory_type, err);
                    continue;
                }
            };

            match ALLOCATORS.lock().get_or_create_allocator(bucket.memory_type, handle) {
                Ok(allocator) => {
                    if let Err(err) = allocator.reserve_memory_pages(bucket.number_of_pages as usize) {
                        log::error!("failed to reserve pages for memory type {:#x?}: {:#x?}", bucket.memory_type, err);
                        continue;
                    }
                }
                Err(err) => {
                    log::error!("failed to get an allocator for memory type {:#x?}: {:#x?}", bucket.memory_type, err);
                    continue;
                }
            }
        }
    }
}

pub fn install_memory_services(bs: &mut efi::BootServices) {
    bs.allocate_pages = allocate_pages;
    bs.free_pages = free_pages;
    bs.allocate_pool = allocate_pool;
    bs.free_pool = free_pool;
    bs.copy_mem = copy_mem;
    bs.set_mem = set_mem;
    bs.get_memory_map = get_memory_map;
}

// Resets the ALLOCATOR map to empty and resets the static allocators for test purposes.
#[cfg(test)]
pub(crate) unsafe fn reset_allocators() {
    unsafe { ALLOCATORS.lock().reset() };
}

#[cfg(test)]
#[coverage(off)]
mod tests {

    use crate::{
        gcd,
        test_support::{self, build_test_hob_list},
    };

    use super::*;
    use patina::pi::hob::{GUID_EXTENSION, GuidHob, Hob, header};
    use r_efi::efi;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(gcd_size: usize, f: F) {
        test_support::with_global_lock(|| {
            unsafe {
                test_support::init_test_gcd(Some(gcd_size));
                test_support::init_test_protocol_db();
                test_support::reset_allocators();
            }
            f();
        })
        .unwrap();
    }

    #[test]
    #[allow(unpredictable_function_pointer_comparisons)]
    fn install_memory_support_should_populate_boot_services_ptrs() {
        let boot_services = core::mem::MaybeUninit::zeroed();
        let mut boot_services: efi::BootServices = unsafe { boot_services.assume_init() };
        install_memory_services(&mut boot_services);
        assert!(boot_services.allocate_pages == allocate_pages);
        assert!(boot_services.free_pages == free_pages);
        assert!(boot_services.allocate_pool == allocate_pool);
        assert!(boot_services.free_pool == free_pool);
        assert!(boot_services.copy_mem == copy_mem);
        assert!(boot_services.get_memory_map == get_memory_map);
    }

    #[test]
    fn init_memory_support_should_process_memory_bucket_hobs() {
        test_support::with_global_lock(|| {
            let physical_hob_list = build_test_hob_list(0x1000000);
            unsafe {
                GCD.reset();
                gcd::init_gcd(physical_hob_list);
                test_support::init_test_protocol_db();
                ALLOCATORS.lock().reset();
            }

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            hob_list.push(Hob::GuidHob(
                &GuidHob {
                    header: header::Hob { r#type: GUID_EXTENSION, length: 48, reserved: 0 },
                    name: MEMORY_TYPE_INFO_HOB_GUID,
                },
                &[
                    // for test, pick dynamic allocators, since state is easier to clean up for those.
                    0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, //0x0100 pages of LOADER_DATA
                    0x09, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, //0x0200 pages of ACPI_RECLAIM_MEMORY
                    0x0a, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, //0x0300 pages of ACPI_MEMORY_NVS
                ],
            ));

            // Required memory allocation hob for stack
            let mut stack_base_address = 0xEB000;
            stack_base_address = (physical_hob_list as u64).wrapping_add(stack_base_address);
            let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
                header: patina::pi::hob::header::Hob {
                    r#type: hob::MEMORY_ALLOCATION,
                    length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                    reserved: 0x00000000,
                },
                alloc_descriptor: patina::pi::hob::header::MemoryAllocation {
                    name: HOB_MEMORY_ALLOC_STACK,
                    memory_base_address: stack_base_address,
                    memory_length: 0x2000,
                    memory_type: efi::BOOT_SERVICES_DATA,
                    reserved: Default::default(),
                },
            });
            hob_list.push(stack_hob);

            init_memory_support(&hob_list);

            let loader_range = ALLOCATORS.lock().get_allocator(efi::LOADER_DATA).unwrap().reserved_range().unwrap();
            assert_eq!(loader_range.end - loader_range.start, 0x100 * 0x1000);

            let reclaim_range =
                ALLOCATORS.lock().get_allocator(efi::ACPI_RECLAIM_MEMORY).unwrap().reserved_range().unwrap();
            assert_eq!(reclaim_range.end - reclaim_range.start, 0x200 * 0x1000);

            let nvs_range = ALLOCATORS.lock().get_allocator(efi::ACPI_MEMORY_NVS).unwrap().reserved_range().unwrap();
            assert_eq!(nvs_range.end - nvs_range.start, 0x300 * 0x1000);
        })
        .unwrap();
    }

    #[test]
    fn init_memory_support_should_process_resource_allocations() {
        test_support::with_global_lock(|| {
            // 4 MiB of test memory is required because allocator expansion during initialization
            // may need to handle large allocations for memory buckets and HOBs.
            let physical_hob_list = build_test_hob_list(0x400000);
            unsafe {
                GCD.reset();
                gcd::init_gcd(physical_hob_list);
                test_support::init_test_protocol_db();
                ALLOCATORS.lock().reset();
            }

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            // Required memory allocation hob for stack
            let mut stack_base_address = 0xEB000;
            stack_base_address = (physical_hob_list as u64).wrapping_add(stack_base_address);

            let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
                header: patina::pi::hob::header::Hob {
                    r#type: hob::MEMORY_ALLOCATION,
                    length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                    reserved: 0x00000000,
                },
                alloc_descriptor: patina::pi::hob::header::MemoryAllocation {
                    name: HOB_MEMORY_ALLOC_STACK,
                    memory_base_address: stack_base_address,
                    memory_length: 0x2000,
                    memory_type: efi::BOOT_SERVICES_DATA,
                    reserved: Default::default(),
                },
            });
            hob_list.push(stack_hob);

            init_memory_support(&hob_list);

            let allocators = ALLOCATORS.lock();

            // Verify that the memory allocation HOBs resulted in claimed pages in the allocator.
            for memory_type in [
                efi::RESERVED_MEMORY_TYPE,
                efi::LOADER_CODE,
                efi::LOADER_DATA,
                efi::BOOT_SERVICES_CODE,
                efi::BOOT_SERVICES_DATA,
                efi::RUNTIME_SERVICES_CODE,
                efi::RUNTIME_SERVICES_DATA,
                efi::ACPI_RECLAIM_MEMORY,
                efi::ACPI_MEMORY_NVS,
                efi::PAL_CODE,
            ]
            .iter()
            {
                let allocator = allocators.get_allocator(*memory_type).unwrap();

                if *memory_type == efi::BOOT_SERVICES_DATA {
                    assert_eq!(allocator.stats().claimed_pages, 3);
                } else {
                    assert_eq!(allocator.stats().claimed_pages, 1);
                }
            }

            // Locate stack hob.
            let stack_hob = hob_list
                .iter()
                .find_map(|x| match x {
                    patina::pi::hob::Hob::MemoryAllocation(hob::MemoryAllocation {
                        header: _,
                        alloc_descriptor: desc,
                    }) if desc.name == HOB_MEMORY_ALLOC_STACK => Some(desc),
                    _ => None,
                })
                .unwrap();

            assert!(stack_hob.memory_base_address != 0);
            assert!(stack_hob.memory_length != 0);

            // Check Guard Page.
            let mut stack_desc = GCD.get_memory_descriptor_for_address(stack_hob.memory_base_address).unwrap();
            assert_eq!(stack_desc.memory_type, dxe_services::GcdMemoryType::SystemMemory);
            assert_eq!((stack_desc.attributes & efi::MEMORY_RP), efi::MEMORY_RP);

            // Check rest of the stack.
            stack_desc =
                GCD.get_memory_descriptor_for_address(stack_hob.memory_base_address + UEFI_PAGE_SIZE as u64).unwrap();
            assert_eq!((stack_desc.attributes & efi::MEMORY_XP), efi::MEMORY_XP);
            assert_eq!(stack_desc.memory_type, dxe_services::GcdMemoryType::SystemMemory);

            // confirm the MMIO memory allocation occurred in the GCD
            let mmio_desc = GCD.get_memory_descriptor_for_address(0x10000000).unwrap();
            assert_eq!(mmio_desc.memory_type, dxe_services::GcdMemoryType::MemoryMappedIo);
            assert_eq!(mmio_desc.base_address, 0x10000000);
            assert_eq!(mmio_desc.length, 0x2000);
            assert_eq!(mmio_desc.image_handle, protocol_db::DXE_CORE_HANDLE);

            // confirm the rest of the MMIO region is not allocated
            let mmio_desc = GCD.get_memory_descriptor_for_address(0x10002000).unwrap();
            assert_eq!(mmio_desc.memory_type, dxe_services::GcdMemoryType::MemoryMappedIo);
            assert_eq!(mmio_desc.base_address, 0x10002000);
            assert_eq!(mmio_desc.length, 0x1000000 - 0x2000);
            assert_eq!(mmio_desc.image_handle, INVALID_HANDLE);
        })
        .unwrap();
    }

    #[test]
    #[should_panic]
    fn should_have_stack_hob() {
        test_support::with_global_lock(|| {
            // 4 MiB of test memory is required because allocator expansion during initialization
            // may need to handle large allocations for memory buckets and HOBs.
            let physical_hob_list = build_test_hob_list(0x400000);
            unsafe {
                GCD.reset();
                gcd::init_gcd(physical_hob_list);
                test_support::init_test_protocol_db();
                ALLOCATORS.lock().reset();
            }

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            init_memory_support(&hob_list);
        })
        .unwrap();
    }

    #[test]
    #[should_panic]
    fn should_have_non_zero_stack_base_address_length() {
        test_support::with_global_lock(|| {
            // 4 MiB of test memory is required because allocator expansion during initialization
            // may need to handle large allocations for memory buckets and HOBs.
            let physical_hob_list = build_test_hob_list(0x400000);
            unsafe {
                GCD.reset();
                gcd::init_gcd(physical_hob_list);
                test_support::init_test_protocol_db();
                ALLOCATORS.lock().reset();
            }

            let mut hob_list = HobList::default();
            hob_list.discover_hobs(physical_hob_list);

            let stack_hob = Hob::MemoryAllocation(&patina::pi::hob::MemoryAllocation {
                header: patina::pi::hob::header::Hob {
                    r#type: hob::MEMORY_ALLOCATION,
                    length: core::mem::size_of::<hob::MemoryAllocation>() as u16,
                    reserved: 0x00000000,
                },
                alloc_descriptor: patina::pi::hob::header::MemoryAllocation {
                    name: HOB_MEMORY_ALLOC_STACK,
                    memory_base_address: 0,
                    memory_length: 0,
                    memory_type: efi::BOOT_SERVICES_DATA,
                    reserved: Default::default(),
                },
            });
            hob_list.push(stack_hob);

            init_memory_support(&hob_list);
        })
        .unwrap();
    }

    #[test]
    fn new_should_create_new_allocator_map() {
        let _map = AllocatorMap::new();
    }

    #[test]
    fn well_known_allocators_should_be_retrievable() {
        with_locked_state(0x4000000, || {
            let allocators = ALLOCATORS.lock();

            for (mem_type, handle) in [
                (efi::LOADER_CODE, protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE),
                (efi::BOOT_SERVICES_CODE, protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE),
                (efi::BOOT_SERVICES_DATA, protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE),
                (efi::RUNTIME_SERVICES_CODE, protocol_db::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR_HANDLE),
                (efi::RUNTIME_SERVICES_DATA, protocol_db::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR_HANDLE),
            ] {
                let allocator = allocators.get_allocator(mem_type).unwrap();
                assert_eq!(allocator.handle(), handle);
            }
        });
    }

    #[test]
    fn new_allocators_should_be_created_on_demand() {
        with_locked_state(0x4000000, || {
            for (mem_type, handle) in [
                (efi::RESERVED_MEMORY_TYPE, protocol_db::RESERVED_MEMORY_ALLOCATOR_HANDLE),
                (efi::LOADER_CODE, protocol_db::EFI_LOADER_CODE_ALLOCATOR_HANDLE),
                (efi::LOADER_DATA, protocol_db::EFI_LOADER_DATA_ALLOCATOR_HANDLE),
                (efi::BOOT_SERVICES_CODE, protocol_db::EFI_BOOT_SERVICES_CODE_ALLOCATOR_HANDLE),
                (efi::BOOT_SERVICES_DATA, protocol_db::EFI_BOOT_SERVICES_DATA_ALLOCATOR_HANDLE),
                (efi::RUNTIME_SERVICES_CODE, protocol_db::EFI_RUNTIME_SERVICES_CODE_ALLOCATOR_HANDLE),
                (efi::RUNTIME_SERVICES_DATA, protocol_db::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR_HANDLE),
                (efi::ACPI_RECLAIM_MEMORY, protocol_db::EFI_ACPI_RECLAIM_MEMORY_ALLOCATOR_HANDLE),
                (efi::ACPI_MEMORY_NVS, protocol_db::EFI_ACPI_MEMORY_NVS_ALLOCATOR_HANDLE),
            ] {
                let ptr = core_allocate_pool(mem_type, 0x1000).unwrap();
                assert!(!ptr.is_null());

                let allocators = ALLOCATORS.lock();

                let allocator = allocators.get_allocator(mem_type).unwrap();
                assert_eq!(allocator.handle(), handle);
                assert_eq!(allocators.memory_type_for_handle(handle), Some(mem_type));
                drop(allocators);
                assert_eq!(AllocatorMap::handle_for_memory_type(mem_type).unwrap(), handle);
            }

            // make sure invalid mem types throw an error.
            assert_eq!(core_allocate_pool(efi::PERSISTENT_MEMORY, 0x1000), Err(EfiError::InvalidParameter));
            assert_eq!(core_allocate_pool(efi::PERSISTENT_MEMORY + 0x1000, 0x1000), Err(EfiError::InvalidParameter));

            // check "OEM" and "OS" custom memory types.
            let ptr = core_allocate_pool(0x71234567, 0x1000).unwrap();
            assert!(!ptr.is_null());

            let ptr = core_allocate_pool(0x81234567, 0x1000).unwrap();
            assert!(!ptr.is_null());

            let allocators = ALLOCATORS.lock();
            let allocator = allocators.get_allocator(0x71234567).unwrap();
            let handle = allocator.handle();
            assert_eq!(allocators.memory_type_for_handle(handle), Some(0x71234567));
            drop(allocators);
            assert_eq!(AllocatorMap::handle_for_memory_type(0x71234567).unwrap(), handle);

            let allocators = ALLOCATORS.lock();
            let allocator = allocators.get_allocator(0x81234567).unwrap();
            let handle = allocator.handle();
            assert_eq!(allocators.memory_type_for_handle(handle), Some(0x81234567));
            drop(allocators);
            assert_eq!(AllocatorMap::handle_for_memory_type(0x81234567).unwrap(), handle);
        });
    }

    // This test uses an allocation request size of 0x2b2fa0 to test a specific edge case.
    //
    // When core_allocate_pool attempts to allocate 0x2b2fa0 bytes (aligned to 0x2b2fb8 bytes), the allocation
    // will initially fail and trigger a fallback to fallback_alloc, which requests 0x2b2ff8 bytes of additional
    // memory. The GCD then allocates memory with a page-aligned size of 0x2b3000 bytes. Following this, the system
    // calls expand() to add the newly allocated heap, which ends up being 0x2b2fc0 bytes in size. However, the
    // difference between the heap size (0x2b2fc0) and the expected memory size (0x2b2fb8) is only 8 bytes. This
    // small gap is insufficient to accommodate the metadata required by HoleList::new() from the
    // linked_list_allocator crate, which expects extra space for internal bookkeeping structures.
    //
    // This test ensures that the linked list hole list structure is accounted for correctly and that the allocation
    // succeeds without panicking due to insufficient space. For this particular test, the failing additional memory
    // size returned was 0x2b3000 bytes (panic) whereas now 0x2b3010 bytes is returned to account for the HoleList
    // metadata.
    #[test]
    fn linked_list_hole_list_struct_should_be_accounted_for() {
        with_locked_state(0x4000000, || {
            let ptr = core_allocate_pool(efi::BOOT_SERVICES_DATA, 0x2B2FA0).unwrap();
            assert!(!ptr.is_null());
        });
    }

    #[test]
    fn allocate_pool_should_allocate_pool() {
        with_locked_state(0x1000000, || {
            let mut buffer_ptr = core::ptr::null_mut();

            // test that disallowed types cannot be allocated
            assert_eq!(
                allocate_pool(efi::CONVENTIONAL_MEMORY, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pool(efi::PERSISTENT_MEMORY, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pool(efi::UNUSABLE_MEMORY, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::SUCCESS
            );

            assert_eq!(
                allocate_pool(efi::UNACCEPTED_MEMORY_TYPE, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pool(efi::BOOT_SERVICES_DATA, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::SUCCESS
            );

            assert_eq!(
                allocate_pool(efi::BOOT_SERVICES_DATA, 0x2000000, core::ptr::null_mut()),
                efi::Status::INVALID_PARAMETER
            );
        });
    }

    #[test]
    fn free_pool_should_free_pool() {
        with_locked_state(0x1000000, || {
            let mut buffer_ptr = core::ptr::null_mut();
            assert_eq!(
                allocate_pool(efi::BOOT_SERVICES_DATA, 0x1000, core::ptr::addr_of_mut!(buffer_ptr)),
                efi::Status::SUCCESS
            );

            assert_eq!(free_pool(buffer_ptr), efi::Status::SUCCESS);

            assert_eq!(free_pool(core::ptr::null_mut()), efi::Status::INVALID_PARAMETER);
            //TODO: these cause non-unwinding panic which crashes the test even with "#[should_panic]".
            //assert_eq!(free_pool(buffer_ptr), efi::Status::INVALID_PARAMETER);
            //assert_eq!(free_pool(((buffer_ptr as usize) + 10) as *mut c_void), efi::Status::INVALID_PARAMETER);
        });
    }

    #[test]
    fn allocate_pages_should_allocate_pages() {
        with_locked_state(0x1000000, || {
            //test test null memory pointer fails with invalid param.
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::BOOT_SERVICES_DATA,
                    0x4,
                    core::ptr::null_mut() as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            //test can't allocate un-allocatable types
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::CONVENTIONAL_MEMORY,
                    0x4,
                    core::ptr::null_mut() as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::PERSISTENT_MEMORY,
                    0x4,
                    core::ptr::null_mut() as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::UNUSABLE_MEMORY,
                    0x4,
                    core::ptr::null_mut() as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::UNACCEPTED_MEMORY_TYPE,
                    0x4,
                    core::ptr::null_mut() as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            //test successful allocate_any
            let mut buffer_ptr: *mut u8 = core::ptr::null_mut();
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::BOOT_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );
            free_pages(buffer_ptr as u64, 0x10);

            //test successful allocate_address at the address that was just freed
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ADDRESS,
                    efi::BOOT_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );
            free_pages(buffer_ptr as u64, 0x10);

            //test successful allocate_max where max is greater than the address that was just freed.
            buffer_ptr = buffer_ptr.wrapping_add(0x11 * 0x1000);
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_MAX_ADDRESS,
                    efi::BOOT_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );
            free_pages(buffer_ptr as u64, 0x10);

            //test invalid allocation type
            assert_eq!(
                allocate_pages(
                    0x12345,
                    efi::BOOT_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            //test creation of new allocator for OS/OEM defined allocator type.
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    0x71234567,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );
            free_pages(buffer_ptr as u64, 0x10);
            let allocators = ALLOCATORS.lock();
            let allocator = allocators.get_allocator(0x71234567).unwrap();
            let handle = allocator.handle();
            assert_eq!(allocators.memory_type_for_handle(handle), Some(0x71234567));
            drop(allocators);
            assert_eq!(AllocatorMap::handle_for_memory_type(0x71234567).unwrap(), handle);

            //test that creation of new allocator for illegal type fails.
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::PERSISTENT_MEMORY,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::INVALID_PARAMETER
            );

            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::UNUSABLE_MEMORY,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );
        })
    }

    #[test]
    fn free_pages_error_scenarios_should_be_handled_properly() {
        with_locked_state(0x1000000, || {
            assert_eq!(free_pages(0x12345000, usize::MAX & !0xFFF), efi::Status::INVALID_PARAMETER);
            assert_eq!(free_pages(u64::MAX & !0xFFF, 0x10), efi::Status::INVALID_PARAMETER);
            assert_eq!(free_pages(0x12345678, 1), efi::Status::INVALID_PARAMETER);
            assert_eq!(free_pages(0x12345000, 1), efi::Status::NOT_FOUND);
        });
    }

    #[test]
    fn copy_mem_should_copy_mem() {
        let mut dest = vec![0xa5u8; 0x10];
        let mut src = vec![0x5au8; 0x10];
        copy_mem(dest.as_mut_ptr() as *mut c_void, src.as_mut_ptr() as *mut c_void, 0x10);
        assert_eq!(dest, src);
    }

    #[test]
    fn set_mem_should_set_mem() {
        let mut dest = vec![0xa5u8; 0x10];
        set_mem(dest.as_mut_ptr() as *mut c_void, 0x10, 0x00);
        assert_eq!(dest, vec![0x00u8; 0x10]);
    }

    #[test]
    fn get_memory_map_should_return_a_memory_map() {
        with_locked_state(0x1000000, || {
            //reserve some pages in the runtime services data allocator.
            ALLOCATORS.lock().get_allocator(efi::RUNTIME_SERVICES_DATA).unwrap().reserve_memory_pages(0x100).unwrap();

            // allocate some "custom" type pages to create something interesting to find in the map.
            let mut buffer_ptr: *mut u8 = core::ptr::null_mut();
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    0x71234567,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );

            // allocate some "runtime" type pages to create something interesting to find in the map.
            let mut runtime_buffer_ptr: *mut u8 = core::ptr::null_mut();
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::RUNTIME_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(runtime_buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );

            let mut memory_map_size = 0;
            let mut map_key = 0;
            let mut descriptor_size = 0;
            let mut version = 0;
            let status = get_memory_map(
                core::ptr::addr_of_mut!(memory_map_size),
                core::ptr::null_mut(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::BUFFER_TOO_SMALL);
            assert_ne!(memory_map_size, 0);
            assert_eq!(descriptor_size, core::mem::size_of::<efi::MemoryDescriptor>());
            assert_eq!(version, 1);
            assert_eq!(map_key, 0);

            let mut memory_map_buffer: Vec<efi::MemoryDescriptor> = vec![
                efi::MemoryDescriptor {
                    r#type: 0,
                    physical_start: 0,
                    virtual_start: 0,
                    number_of_pages: 0,
                    attribute: 0
                };
                memory_map_size / descriptor_size
            ];

            let status = get_memory_map(
                core::ptr::addr_of_mut!(memory_map_size),
                memory_map_buffer.as_mut_ptr(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::SUCCESS);
            assert_eq!(memory_map_size, memory_map_buffer.len() * core::mem::size_of::<efi::MemoryDescriptor>());
            assert_eq!(descriptor_size, core::mem::size_of::<efi::MemoryDescriptor>());
            assert_eq!(version, 1);
            assert_ne!(map_key, 0);

            //make sure that the custom "allocate_pages" shows up in the map somewhere.
            memory_map_buffer
                .iter()
                .find(|x| {
                    x.physical_start <= buffer_ptr as efi::PhysicalAddress
                        && x.physical_start.checked_add(x.number_of_pages * UEFI_PAGE_SIZE as u64).unwrap()
                            > buffer_ptr as efi::PhysicalAddress
                        && x.r#type == 0x71234567
                })
                .expect("Failed to find custom allocation.");

            // make sure that the runtime "allocate_pages" shows up in the map somewhere.
            // This may be folded into oth ranges or the bucket.
            memory_map_buffer
                .iter()
                .find(|x| {
                    x.physical_start <= runtime_buffer_ptr as efi::PhysicalAddress
                        && x.physical_start.checked_add(x.number_of_pages * UEFI_PAGE_SIZE as u64).unwrap()
                            > runtime_buffer_ptr as efi::PhysicalAddress
                        && x.number_of_pages
                            >= (runtime_buffer_ptr as efi::PhysicalAddress)
                                .checked_sub(x.physical_start)
                                .unwrap()
                                .checked_div(UEFI_PAGE_SIZE as u64)
                                .unwrap()
                                + 0x10
                        && x.r#type == efi::RUNTIME_SERVICES_DATA
                        && (x.attribute & efi::MEMORY_RUNTIME) != 0
                })
                .expect("Failed to find runtime allocation.");

            //get_memory_map with null size should return invalid parameter
            let status = get_memory_map(
                core::ptr::null_mut(),
                memory_map_buffer.as_mut_ptr(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::INVALID_PARAMETER);

            //get_memory_map with non-null size but null map should return invalid parameter
            let status = get_memory_map(
                core::ptr::addr_of_mut!(memory_map_size),
                core::ptr::null_mut(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::INVALID_PARAMETER);
        })
    }

    #[test]
    fn terminate_map_should_validate_the_map_key() {
        with_locked_state(0x1000000, || {
            // allocate some "custom" type pages to create something interesting to find in the map.
            let mut buffer_ptr: *mut u8 = core::ptr::null_mut();
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    0x71234567,
                    0x10,
                    core::ptr::addr_of_mut!(buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );

            // allocate some "custom" type pages to create something interesting to find in the map.
            let mut runtime_buffer_ptr: *mut u8 = core::ptr::null_mut();
            assert_eq!(
                allocate_pages(
                    efi::ALLOCATE_ANY_PAGES,
                    efi::RUNTIME_SERVICES_DATA,
                    0x10,
                    core::ptr::addr_of_mut!(runtime_buffer_ptr) as *mut efi::PhysicalAddress
                ),
                efi::Status::SUCCESS
            );

            //get the map.
            let mut memory_map_size = 0;
            let mut map_key = 0;
            let mut descriptor_size = 0;
            let mut version = 0;
            let status = get_memory_map(
                core::ptr::addr_of_mut!(memory_map_size),
                core::ptr::null_mut(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::BUFFER_TOO_SMALL);

            let mut memory_map_buffer: Vec<efi::MemoryDescriptor> = vec![
                efi::MemoryDescriptor {
                    r#type: 0,
                    physical_start: 0,
                    virtual_start: 0,
                    number_of_pages: 0,
                    attribute: 0
                };
                memory_map_size / descriptor_size
            ];

            let status = get_memory_map(
                core::ptr::addr_of_mut!(memory_map_size),
                memory_map_buffer.as_mut_ptr(),
                core::ptr::addr_of_mut!(map_key),
                core::ptr::addr_of_mut!(descriptor_size),
                core::ptr::addr_of_mut!(version),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            assert!(terminate_memory_map(map_key).is_ok());
            assert_eq!(terminate_memory_map(map_key + 1), Err(EfiError::InvalidParameter));
        });
    }
}
