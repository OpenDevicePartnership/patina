//! DXE Core Image Services
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use core::convert::TryInto;
use core::{ffi::c_void, mem::transmute, slice::from_raw_parts};
use mu_pi::hob::{Hob, HobList};
use r_efi::efi;
use uefi_component_interface::DxeComponent;
use uefi_device_path::{copy_device_path_to_boxed_slice, device_path_node_count, DevicePathWalker};

use crate::{
    allocator::{core_allocate_pages, core_free_pages},
    component_interface, dxe_services,
    filesystems::SimpleFile,
    pecoff::{self, relocation::RelocationBlock, UefiPeInfo},
    protocol_db,
    protocols::{core_install_protocol_interface, core_locate_device_path, PROTOCOL_DB},
    runtime,
    systemtables::EfiSystemTable,
    tpl_lock,
};

use corosensei::{
    stack::{Stack, StackPointer, MIN_STACK_SIZE, STACK_ALIGNMENT},
    Coroutine, CoroutineResult, Yielder,
};

pub const EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION: u16 = 10;
pub const EFI_IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER: u16 = 11;
pub const EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER: u16 = 12;

pub const ENTRY_POINT_STACK_SIZE: usize = 0x100000;

// Todo: Move these to a centralized, permanent location
const UEFI_PAGE_SIZE: usize = 0x1000;
const UEFI_PAGE_MASK: usize = UEFI_PAGE_SIZE - 1;

macro_rules! uefi_size_to_pages {
    ($size:expr) => {
        (($size) + UEFI_PAGE_MASK) / UEFI_PAGE_SIZE
    };
}

// dummy function used to initialize PrivateImageData.entry_point.
#[cfg(not(tarpaulin_include))]
extern "efiapi" fn unimplemented_entry_point(
    _handle: efi::Handle,
    _system_table: *mut efi::SystemTable,
) -> efi::Status {
    unimplemented!()
}

// define a stack structure for coroutine support.
struct ImageStack {
    stack: *const [u8],
    len: usize,
    allocated_pages: usize,
}

impl ImageStack {
    fn new(size: usize) -> Result<Self, efi::Status> {
        let mut stack: efi::PhysicalAddress = 0;
        let len = align_up(size.max(MIN_STACK_SIZE) as u64, STACK_ALIGNMENT as u64) as usize;
        // allocate an extra page for the stack guard page.
        let allocated_pages = uefi_size_to_pages!(len) + 1;

        // allocate the stack, newly allocated memory will have efi::MEMORY_XP already set, so we don't need to set it
        // here
        core_allocate_pages(efi::ALLOCATE_ANY_PAGES, efi::BOOT_SERVICES_DATA, allocated_pages, &mut stack)?;

        // attempt to set the memory space attributes for the stack guard page.
        // if we fail, we should still try to continue to boot
        // the stack grows downwards, so stack here is the guard page
        let attributes = match dxe_services::core_get_memory_space_descriptor(stack) {
            Ok(descriptor) => descriptor.attributes,
            Err(_) => 0,
        };
        if let Err(err) =
            dxe_services::core_set_memory_space_attributes(stack, UEFI_PAGE_SIZE as u64, attributes | efi::MEMORY_RP)
        {
            log::error!("Failed to set memory space attributes for stack guard page: {:#x?}", err);
            debug_assert!(false);
        }

        // we have the guard page at the bottom, so we need to add a page to the stack pointer for the limit
        Ok(ImageStack {
            stack: core::ptr::slice_from_raw_parts_mut((stack + (UEFI_PAGE_SIZE as u64)) as *mut u8, len),
            len,
            allocated_pages,
        })
    }
}

impl Drop for ImageStack {
    fn drop(&mut self) {
        if !self.stack.is_null() {
            // we added a guard page, so we need to subtract a page from the stack pointer to free everything
            let stack_addr = self.stack as *const u64 as efi::PhysicalAddress - UEFI_PAGE_SIZE as u64;

            // we need to set the guard page back to XP so that the pages can be coalesced before we free them
            // preserve the caching attributes
            let mut attributes = match dxe_services::core_get_memory_space_descriptor(stack_addr) {
                Ok(descriptor) => descriptor.attributes & !efi::MEMORY_ATTRIBUTE_MASK,
                Err(_) => 0,
            };

            attributes |= efi::MEMORY_XP;
            if let Err(err) =
                dxe_services::core_set_memory_space_attributes(stack_addr, UEFI_PAGE_SIZE as u64, attributes)
            {
                log::error!("Failed to set memory space attributes for stack guard page: {:#x?}", err);
                debug_assert!(false);
                // if we failed, let's still try to free
            }

            if let Err(status) = core_free_pages(stack_addr, self.allocated_pages) {
                log::error!(
                    "core_free_pages returned error {:#x?} for image stack at {:#x} for num_pages {:#x}",
                    status,
                    stack_addr,
                    self.allocated_pages
                );
            }
        }
    }
}

unsafe impl Stack for ImageStack {
    fn base(&self) -> StackPointer {
        //stack grows downward, so "base" is the highest address, i.e. the ptr + size.
        self.limit().checked_add(self.len).expect("Stack base address overflow.")
    }
    fn limit(&self) -> StackPointer {
        //stack grows downward, so "limit" is the lowest address, i.e. the ptr.
        StackPointer::new(self.stack as *const u8 as usize)
            .expect("Stack pointer address was zero, but it should always be nonzero.")
    }
}

// This struct tracks private data associated with a particular image handle.
struct PrivateImageData {
    image_buffer: *mut [u8],
    image_info: Box<efi::protocols::loaded_image::Protocol>,
    hii_resource_section: Option<*mut [u8]>,
    hii_resource_section_base: Option<efi::PhysicalAddress>,
    hii_resource_section_num_pages: Option<usize>,
    entry_point: efi::ImageEntryPoint,
    started: bool,
    exit_data: Option<(usize, *mut efi::Char16)>,
    image_info_ptr: *mut c_void,
    image_device_path_ptr: *mut c_void,
    pe_info: UefiPeInfo,
    relocation_data: Vec<RelocationBlock>,
    image_base_page: efi::PhysicalAddress,
    image_num_pages: usize,
}

impl PrivateImageData {
    fn new(image_info: efi::protocols::loaded_image::Protocol, pe_info: &UefiPeInfo) -> Result<Self, efi::Status> {
        // Allocate pages for the image to be loaded into. We use pages here instead of a pool because we are going to
        // set memory attributes on this range and it is not valid to set attributes on pool backed memory.
        let mut image_base_page: efi::PhysicalAddress = 0;

        // if we have a unique alignment requirement, we need to overallocate the buffer to ensure we can align the base
        let num_pages: usize = if pe_info.section_alignment as usize > UEFI_PAGE_SIZE {
            if let Some(image_size) = image_info.image_size.checked_add(pe_info.section_alignment as u64) {
                match usize::try_from(image_size) {
                    Ok(size) => uefi_size_to_pages!(size),
                    Err(_) => return Err(efi::Status::LOAD_ERROR),
                }
            } else {
                return Err(efi::Status::LOAD_ERROR);
            }
        } else {
            match usize::try_from(image_info.image_size) {
                Ok(size) => uefi_size_to_pages!(size),
                Err(_) => return Err(efi::Status::LOAD_ERROR),
            }
        };

        core_allocate_pages(efi::ALLOCATE_ANY_PAGES, image_info.image_code_type, num_pages, &mut image_base_page)?;

        if image_base_page == 0 {
            return Err(efi::Status::OUT_OF_RESOURCES);
        }

        let aligned_image_start = align_up(image_base_page as u64, pe_info.section_alignment as u64);

        let mut image_data = PrivateImageData {
            image_buffer: core::ptr::slice_from_raw_parts_mut(
                aligned_image_start as *mut u8,
                image_info.image_size as usize,
            ),
            image_info: Box::new(image_info),
            hii_resource_section: None,
            hii_resource_section_base: None,
            hii_resource_section_num_pages: None,
            entry_point: unimplemented_entry_point,
            started: false,
            exit_data: None,
            image_info_ptr: core::ptr::null_mut(),
            image_device_path_ptr: core::ptr::null_mut(),
            pe_info: pe_info.clone(),
            relocation_data: Vec::new(),
            image_base_page,
            image_num_pages: num_pages,
        };

        image_data.image_info.image_base = image_data.image_buffer as *mut c_void;
        Ok(image_data)
    }

    fn allocate_resource_section(
        &mut self,
        size: usize,
        alignment: usize,
        code_type: efi::MemoryType,
    ) -> Result<(), efi::Status> {
        let mut hii_base_page: efi::PhysicalAddress = 0;
        // if we have a unique alignment requirement, we need to overallocate the buffer to ensure we can align the base
        let num_pages: usize =
            if alignment > UEFI_PAGE_SIZE { uefi_size_to_pages!(size + alignment) } else { uefi_size_to_pages!(size) };
        core_allocate_pages(efi::ALLOCATE_ANY_PAGES, code_type, num_pages, &mut hii_base_page)?;

        if hii_base_page == 0 {
            return Err(efi::Status::OUT_OF_RESOURCES);
        }

        let aligned_hii_start = align_up(hii_base_page as u64, alignment as u64);

        self.hii_resource_section = Some(core::ptr::slice_from_raw_parts_mut(aligned_hii_start as *mut u8, size));
        self.hii_resource_section_base = Some(hii_base_page);
        self.hii_resource_section_num_pages = Some(num_pages);
        Ok(())
    }
}

impl Drop for PrivateImageData {
    fn drop(&mut self) {
        if !self.image_buffer.is_null() {
            if let Err(status) = core_free_pages(self.image_base_page, self.image_num_pages) {
                log::error!(
                    "core_free_pages returned error {:#x?} for image buffer at {:#x} for num_pages {:#x}",
                    status,
                    self.image_base_page,
                    self.image_num_pages
                );
            }
        }

        if let (Some(resource_addr), Some(num_pages)) =
            (self.hii_resource_section_base, self.hii_resource_section_num_pages)
        {
            if let Err(status) = core_free_pages(resource_addr, num_pages) {
                log::error!(
                    "core_free_pages returned error {:#x?} for HII resource section at {:#x} for num_pages {:#x}",
                    status,
                    resource_addr,
                    num_pages
                );
            }
        }
    }
}

// This struct tracks global data used by the imaging subsystem.
struct DxeCoreGlobalImageData {
    dxe_core_image_handle: efi::Handle,
    system_table: *mut efi::SystemTable,
    private_image_data: BTreeMap<efi::Handle, PrivateImageData>,
    current_running_image: Option<efi::Handle>,
    image_start_contexts: Vec<*const Yielder<efi::Handle, efi::Status>>,
}

impl DxeCoreGlobalImageData {
    const fn new() -> Self {
        DxeCoreGlobalImageData {
            dxe_core_image_handle: core::ptr::null_mut(),
            system_table: core::ptr::null_mut(),
            private_image_data: BTreeMap::new(),
            current_running_image: None,
            image_start_contexts: Vec::new(),
        }
    }

    #[cfg(test)]
    unsafe fn reset(&mut self) {
        self.dxe_core_image_handle = core::ptr::null_mut();
        self.system_table = core::ptr::null_mut();
        self.private_image_data = BTreeMap::new();
        self.current_running_image = None;
        self.image_start_contexts = Vec::new();
    }
}

// DxeCoreGlobalImageData is accessed through a mutex guard, so it is safe to
// mark it sync/send.
unsafe impl Sync for DxeCoreGlobalImageData {}
unsafe impl Send for DxeCoreGlobalImageData {}

static PRIVATE_IMAGE_DATA: tpl_lock::TplMutex<DxeCoreGlobalImageData> =
    tpl_lock::TplMutex::new(efi::TPL_NOTIFY, DxeCoreGlobalImageData::new(), "ImageLock");

// helper routine that returns an empty loaded_image::Protocol struct.
fn empty_image_info() -> efi::protocols::loaded_image::Protocol {
    efi::protocols::loaded_image::Protocol {
        revision: efi::protocols::loaded_image::REVISION,
        parent_handle: core::ptr::null_mut(),
        system_table: core::ptr::null_mut(),
        device_handle: core::ptr::null_mut(),
        file_path: core::ptr::null_mut(),
        reserved: core::ptr::null_mut(),
        load_options_size: 0,
        load_options: core::ptr::null_mut(),
        image_base: core::ptr::null_mut(),
        image_size: 0,
        image_code_type: efi::BOOT_SERVICES_CODE,
        image_data_type: efi::BOOT_SERVICES_DATA,
        unload: None,
    }
}

fn apply_image_memory_protections(pe_info: &UefiPeInfo, private_info: &PrivateImageData) {
    for section in &pe_info.sections {
        let mut attributes = efi::MEMORY_XP;
        if section.characteristics & pecoff::IMAGE_SCN_CNT_CODE == pecoff::IMAGE_SCN_CNT_CODE {
            attributes = efi::MEMORY_RO;
        }

        // each section starts at image_base + virtual_address, per PE/COFF spec.
        let section_base_addr = (private_info.image_info.image_base as u64) + (section.virtual_address as u64);

        let mut capabilities = attributes;

        // we need to get the current attributes for this region and add our new attribute
        // if we can't find this range in the GCD, try the next one, but report the failure
        match dxe_services::core_get_memory_space_descriptor(section_base_addr) {
            // in the Ok case, keep the cache attributes, but remove the existing memory attributes
            // all new memory has efi::MEMORY_XP set, so we need to remove this if this is becoming a code
            // section
            Ok(desc) => {
                attributes |= desc.attributes & !efi::MEMORY_ATTRIBUTE_MASK;
                capabilities |= desc.capabilities;
            }
            Err(status) => {
                log::error!(
                    "Failed to find GCD desc for image section {:#X} with Status {:#X?}",
                    section_base_addr,
                    status
                );
                continue;
            }
        }

        // now actually set the attributes. We need to use the virtual size for the section length, but
        // we cannot rely on this to be section aligned, as some compilers rely on the loader to align this
        // while we are still relying on the C CpuDxe for page table mgmt, we expect failures here before CpuDxe is
        // loaded as core_set_memory_space_attributes will attempt to call the Cpu Arch protocol to set the page table
        // attributes. We also need to ensure the capabilities are set. We set the capabilities as the old capabilities
        // plus our new attribute, as we need to ensure all existing attributes are supported by the new
        // capabilities.
        let aligned_virtual_size = align_up(section.virtual_size as u64, pe_info.section_alignment as u64);
        if let Err(status) =
            dxe_services::core_set_memory_space_capabilities(section_base_addr, aligned_virtual_size, capabilities)
        {
            // even if we fail to set the capabilities, we should still try to set the attributes, who knows, maybe we
            // will succeed
            log::error!(
                "Failed to set GCD capabilities for image section {:#X} with Status {:#X?}",
                section_base_addr,
                status
            )
        }

        // this may be verbose to log, but we also have a lot of errors historically here, so let's log at info level
        // for now
        log::info!(
            "Applying image memory protections on {:#X} for len {:#X} with attributes {:#X}",
            section_base_addr,
            aligned_virtual_size,
            attributes
        );

        match dxe_services::core_set_memory_space_attributes(section_base_addr, aligned_virtual_size, attributes) {
            Ok(_) => continue,
            Err(status) => log::error!(
                "Failed to set GCD attributes for image section {:#X} with Status {:#X?}",
                section_base_addr,
                status
            ),
        }
    }
}

fn remove_image_memory_protections(pe_info: &UefiPeInfo, private_info: &PrivateImageData) {
    for section in &pe_info.sections {
        // each section starts at image_base + virtual_address, per PE/COFF spec.
        let section_base_addr = (private_info.image_info.image_base as u64) + (section.virtual_address as u64);

        // we need to get the current attributes for this region and remove our attributes
        // we need to reset this to efi::MEMORY_XP so that we can merge all of the pages allocated for this image
        // together. Any unaligned memory will still have efi::MEMORY_XP set
        match dxe_services::core_get_memory_space_descriptor(section_base_addr) {
            Ok(desc) => {
                let attributes = desc.attributes & !efi::MEMORY_ATTRIBUTE_MASK | efi::MEMORY_XP;

                // now set the attributes back to only caching attrs.
                let aligned_virtual_size = align_up(section.virtual_size as u64, pe_info.section_alignment as u64);
                if let Err(status) =
                    dxe_services::core_set_memory_space_attributes(section_base_addr, aligned_virtual_size, attributes)
                {
                    log::error!(
                        "Failed to remove GCD attributes for image section {:#X} with Status {:#X?}",
                        section_base_addr,
                        status
                    );
                }
            }
            Err(status) => {
                log::error!(
                    "Failed to find GCD desc for image section {:#X} with Status {:#X?}, cannot remove memory protections",
                    section_base_addr,
                    status
                );
            }
        }
    }
}

// retrieves the dxe core image info from the hob list, and installs the
// loaded_image protocol on it to create the dxe_core image handle.
fn install_dxe_core_image(hob_list: &HobList) {
    // Retrieve the MemoryAllocationModule hob corresponding to the DXE core
    // (i.e. this driver).
    let dxe_core_hob = hob_list
        .iter()
        .find_map(|x| if let Hob::MemoryAllocationModule(module) = x { Some(module) } else { None })
        .expect("Did not find MemoryAllocationModule Hob for DxeCore");

    // get exclusive access to the global private data.
    let mut private_data = PRIVATE_IMAGE_DATA.lock();

    // convert the entry point from the hob into the appropriate function
    // pointer type and save it in the private_image_data structure for the core.
    // Safety: dxe_core_hob.entry_point must be the correct and actual entry
    // point for the core.
    let entry_point = unsafe {
        transmute::<u64, extern "efiapi" fn(*mut c_void, *mut r_efi::system::SystemTable) -> r_efi::base::Status>(
            dxe_core_hob.entry_point,
        )
    };

    // create the loaded_image structure for the core and populate it with data
    // from the hob.
    let mut image_info = empty_image_info();
    image_info.system_table = private_data.system_table;
    image_info.image_base = dxe_core_hob.alloc_descriptor.memory_base_address as *mut c_void;
    image_info.image_size = dxe_core_hob.alloc_descriptor.memory_length;

    let pe_info = unsafe {
        UefiPeInfo::parse(core::slice::from_raw_parts(
            dxe_core_hob.alloc_descriptor.memory_base_address as *const u8,
            dxe_core_hob.alloc_descriptor.memory_length as usize,
        ))
        .expect("Failed to parse PE info for DXE Core")
    };

    let mut private_image_data =
        PrivateImageData::new(image_info, &pe_info).expect("Failed to create PrivateImageData for dxe_core");
    private_image_data.entry_point = entry_point;

    let image_info_ptr = private_image_data.image_info.as_ref() as *const efi::protocols::loaded_image::Protocol;
    let image_info_ptr = image_info_ptr as *mut c_void;
    private_image_data.image_info_ptr = image_info_ptr;

    // install the loaded_image protocol on a new handle.
    let handle = match core_install_protocol_interface(
        Some(protocol_db::DXE_CORE_HANDLE),
        efi::protocols::loaded_image::PROTOCOL_GUID,
        image_info_ptr,
    ) {
        Err(err) => panic!("Failed to install dxe core image handle: {:?}", err),
        Ok(handle) => handle,
    };
    assert_eq!(handle, protocol_db::DXE_CORE_HANDLE);
    // record this handle as the new dxe_core handle.
    private_data.dxe_core_image_handle = handle;

    let dxe_core_ptr = dxe_core_hob.alloc_descriptor.memory_base_address as *mut c_void;
    if dxe_core_ptr.is_null() {
        log::error!("DXE Core ptr is null. Cannot apply DXE Core memory protections");
    } else {
        // now apply memory protections
        apply_image_memory_protections(&pe_info, &private_image_data);
    }

    // store the dxe_core image private data in the private image data map.
    private_data.private_image_data.insert(handle, private_image_data);
}

/// Align address upwards.
///
/// Returns the smallest `x` with alignment `align` so that `x >= addr`.
///
/// Panics if the alignment is not a power of two or if an overflow occurs.
#[inline]
const fn align_up(addr: u64, align: u64) -> u64 {
    assert!(align.is_power_of_two(), "`align` must be a power of two");
    let align_mask = align - 1;
    if addr & align_mask == 0 {
        addr // already aligned
    } else {
        // FIXME: Replace with .expect, once `Option::expect` is const.
        if let Some(aligned) = (addr | align_mask).checked_add(1) {
            aligned
        } else {
            panic!("attempt to add with overflow")
        }
    }
}

// loads and relocates the image in the specified slice and returns the
// associated PrivateImageData structures.
fn core_load_pe_image(
    image: &[u8],
    mut image_info: efi::protocols::loaded_image::Protocol,
) -> Result<PrivateImageData, efi::Status> {
    // parse and validate the header and retrieve the image data from it.
    let pe_info = pecoff::UefiPeInfo::parse(image)
        .inspect_err(|err| log::error!("core_load_pe_image failed: UefiPeInfo::parse returned {:#x?}", err))
        .map_err(|_| efi::Status::UNSUPPORTED)?;

    // based on the image type, determine the correct allocator and code/data types.
    let (code_type, data_type) = match pe_info.image_type {
        EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION => (efi::LOADER_CODE, efi::LOADER_DATA),
        EFI_IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER => (efi::BOOT_SERVICES_CODE, efi::BOOT_SERVICES_DATA),
        EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER => (efi::RUNTIME_SERVICES_CODE, efi::RUNTIME_SERVICES_DATA),
        unsupported_type => {
            log::error!("core_load_pe_image_failed: unsupported image type: {:#x?}", unsupported_type);
            return Err(efi::Status::UNSUPPORTED);
        }
    };

    let alignment = pe_info.section_alignment as usize; // Need to align the base address with section alignment via overallocation
    let size = pe_info.size_of_image as usize;

    // the size of the image must be a multiple of the section alignment per PE/COFF spec
    if size % alignment != 0 {
        log::error!("core_load_pe_image_failed: size of image is not a multiple of the section alignment");
        debug_assert!(false);
        return Err(efi::Status::LOAD_ERROR);
    }

    image_info.image_size = size as u64;
    image_info.image_code_type = code_type;
    image_info.image_data_type = data_type;

    //allocate a buffer to hold the image (also updates private_info.image_info.image_base)
    let mut private_info = PrivateImageData::new(image_info, &pe_info)?;
    let loaded_image = unsafe { &mut *private_info.image_buffer };

    //load the image into the new loaded image buffer
    pecoff::load_image(&pe_info, image, loaded_image)
        .inspect_err(|err| log::error!("core_load_pe_image_failed: load_image returned status: {:#x?}", err))
        .map_err(|_| efi::Status::LOAD_ERROR)?;

    //relocate the image to the address at which it was loaded.
    let loaded_image_addr = private_info.image_info.image_base as usize;
    private_info.relocation_data = pecoff::relocate_image(&pe_info, loaded_image_addr, loaded_image, &Vec::new())
        .inspect_err(|err| log::error!("core_load_pe_image_failed: relocate_image returned status: {:#x?}", err))
        .map_err(|_| efi::Status::LOAD_ERROR)?;

    // update the entry point. Transmute is required here to cast the raw function address to the ImageEntryPoint function pointer type.
    private_info.entry_point = unsafe {
        transmute::<usize, extern "efiapi" fn(*mut c_void, *mut r_efi::system::SystemTable) -> efi::Status>(
            loaded_image_addr + pe_info.entry_point_offset,
        )
    };

    let result = pecoff::load_resource_section(&pe_info, image)
        .inspect_err(|err| log::error!("core_load_pe_image_failed: load_resource_section returned status: {:#x?}", err))
        .map_err(|_| efi::Status::LOAD_ERROR)?;

    if let Some((resource_section_offset, resource_section_size)) = result {
        private_info.allocate_resource_section(resource_section_size, alignment, code_type)?;
        if let Some(resource_slice) = private_info.hii_resource_section {
            unsafe {
                let image_buf_ref = &mut *private_info.image_buffer;
                let resource_slice = &mut *resource_slice;
                if resource_section_offset + resource_section_size <= image_buf_ref.len() {
                    resource_slice.copy_from_slice(
                        &image_buf_ref[resource_section_offset..resource_section_offset + resource_section_size],
                    );

                    log::info!("HII Resource Section found for {}.", pe_info.filename.as_deref().unwrap_or("Unknown"));
                } else {
                    log::error!(
                        "HII Resource Section offset {:#X} and size {:#X} are out of bounds for image {:?}.",
                        resource_section_offset,
                        resource_section_size,
                        pe_info.filename.as_deref().unwrap_or("Unknown")
                    );
                    debug_assert!(false);
                }
            }
        }
    }

    // finally, update the GCD attributes for this image so that code sections have RO set and data sections have XP
    apply_image_memory_protections(&pe_info, &private_info);

    Ok(private_info)
}

// Reads an image buffer using simple file system or load file protocols.
// Return value is (image_buffer, from_fv, authentication_status).
// Note: presently none of the supported methods return `from_fv` or `authentication_status`.
fn get_buffer_by_file_path(
    boot_policy: bool,
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<(Vec<u8>, bool, u32), efi::Status> {
    if file_path.is_null() {
        Err(efi::Status::INVALID_PARAMETER)?;
    }

    //TODO: EDK2 core has support for loading an image from an FV device path which is not presently supported here.
    //this is the only case that Ok((buffer, true, authentication_status)) would be returned.

    if let Ok(buffer) = get_file_buffer_from_sfs(file_path) {
        return Ok((buffer, false, 0));
    }

    if !boot_policy {
        if let Ok(buffer) =
            get_file_buffer_from_load_protocol(efi::protocols::load_file2::PROTOCOL_GUID, false, file_path)
        {
            return Ok((buffer, false, 0));
        }
    }

    if let Ok(buffer) =
        get_file_buffer_from_load_protocol(efi::protocols::load_file::PROTOCOL_GUID, boot_policy, file_path)
    {
        return Ok((buffer, false, 0));
    }

    Err(efi::Status::NOT_FOUND)
}

fn get_file_buffer_from_sfs(file_path: *mut efi::protocols::device_path::Protocol) -> Result<Vec<u8>, efi::Status> {
    let (remaining_file_path, handle) =
        core_locate_device_path(efi::protocols::simple_file_system::PROTOCOL_GUID, file_path)?;

    let mut file = SimpleFile::open_volume(handle)?;

    for node in unsafe { DevicePathWalker::new(remaining_file_path) } {
        match node.header.r#type {
            efi::protocols::device_path::TYPE_MEDIA
                if node.header.sub_type == efi::protocols::device_path::Media::SUBTYPE_FILE_PATH => {} //proceed on valid path node
            efi::protocols::device_path::TYPE_END => break,
            _ => Err(efi::Status::UNSUPPORTED)?,
        }
        //For MEDIA_FILE_PATH_DP, file name is in the node data, but it needs to be converted to Vec<u16> for call to open.
        let filename: Vec<u16> = node
            .data
            .chunks_exact(2)
            .map(|x: &[u8]| {
                if let Ok(x_bytes) = x.try_into() {
                    Ok(u16::from_le_bytes(x_bytes))
                } else {
                    Err(efi::Status::INVALID_PARAMETER)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        file = file.open(filename, efi::protocols::file::MODE_READ, 0)?;
    }

    // if execution comes here, the above loop was successfully able to open all the files on the remaining device path,
    // so `file` is currently pointing to the desired file (i.e. the last node), and it just needs to be read.
    file.read()
}

fn get_file_buffer_from_load_protocol(
    protocol: efi::Guid,
    boot_policy: bool,
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<Vec<u8>, efi::Status> {
    if !(protocol == efi::protocols::load_file::PROTOCOL_GUID || protocol == efi::protocols::load_file2::PROTOCOL_GUID)
    {
        Err(efi::Status::INVALID_PARAMETER)?;
    }

    if protocol == efi::protocols::load_file2::PROTOCOL_GUID && boot_policy {
        Err(efi::Status::INVALID_PARAMETER)?;
    }

    let (remaining_file_path, handle) = core_locate_device_path(protocol, file_path)?;

    let load_file = PROTOCOL_DB.get_interface_for_handle(handle, protocol)?;
    let load_file =
        unsafe { (load_file as *mut efi::protocols::load_file::Protocol).as_mut().ok_or(efi::Status::UNSUPPORTED)? };

    //determine buffer size.
    let mut buffer_size = 0;
    match (load_file.load_file)(
        load_file,
        remaining_file_path,
        boot_policy.into(),
        core::ptr::addr_of_mut!(buffer_size),
        core::ptr::null_mut(),
    ) {
        efi::Status::BUFFER_TOO_SMALL => (),                     //expected
        efi::Status::SUCCESS => Err(efi::Status::DEVICE_ERROR)?, //not expected for buffer_size = 0
        err => Err(err)?,                                        //unexpected error.
    }

    let mut file_buffer = vec![0u8; buffer_size];
    match (load_file.load_file)(
        load_file,
        remaining_file_path,
        boot_policy.into(),
        core::ptr::addr_of_mut!(buffer_size),
        file_buffer.as_mut_ptr() as *mut c_void,
    ) {
        efi::Status::SUCCESS => Ok(file_buffer),
        err => Err(err),
    }
}

/// Relocates all runtime images to their virtual memory address. This function must only be called
/// after the Runtime Service SetVirtualAddressMap() has been called by the OS.
pub fn core_relocate_runtime_images() {
    let mut private_data = PRIVATE_IMAGE_DATA.lock();

    for image in private_data.private_image_data.values_mut() {
        if image.pe_info.image_type == EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER {
            let loaded_image = unsafe { image.image_buffer.as_mut().unwrap() };
            let loaded_image_addr = image.image_info.image_base as usize;
            let mut loaded_image_virt_addr = loaded_image_addr;

            let _ = runtime::convert_pointer(0, core::ptr::addr_of_mut!(loaded_image_virt_addr) as *mut *mut c_void);
            let _ =
                pecoff::relocate_image(&image.pe_info, loaded_image_virt_addr, loaded_image, &image.relocation_data);
        }
    }
}

// authenticate the given image against the Security and Security2 Architectural Protocols
fn authenticate_image(
    device_path: *mut efi::protocols::device_path::Protocol,
    image: &[u8],
    boot_policy: bool,
    from_fv: bool,
    authentication_status: u32,
) -> Result<(), efi::Status> {
    let security2_protocol = unsafe {
        match PROTOCOL_DB.locate_protocol(mu_pi::protocols::security2::PROTOCOL_GUID) {
            Ok(protocol) => (protocol as *mut mu_pi::protocols::security2::Protocol).as_ref(),
            //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
            //Firmware Volume.
            Err(_) => None,
        }
    };

    let security_protocol = unsafe {
        match PROTOCOL_DB.locate_protocol(mu_pi::protocols::security::PROTOCOL_GUID) {
            Ok(protocol) => (protocol as *mut mu_pi::protocols::security::Protocol).as_ref(),
            //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
            //Firmware Volume.
            Err(_) => None,
        }
    };

    let mut security_status = efi::Status::SUCCESS;
    if let Some(security2) = security2_protocol {
        security_status = (security2.file_authentication)(
            security2 as *const _ as *mut mu_pi::protocols::security2::Protocol,
            device_path,
            image.as_ptr() as *const _ as *mut c_void,
            image.len(),
            boot_policy,
        );
        if security_status == efi::Status::SUCCESS && from_fv {
            let security = security_protocol.expect("Security Arch must be installed if Security2 Arch is installed");
            security_status = (security.file_authentication_state)(
                security as *const _ as *mut mu_pi::protocols::security::Protocol,
                authentication_status,
                device_path,
            );
        }
    } else if let Some(security) = security_protocol {
        security_status = (security.file_authentication_state)(
            security as *const _ as *mut mu_pi::protocols::security::Protocol,
            authentication_status,
            device_path,
        );
    }

    if security_status != efi::Status::SUCCESS {
        return Err(security_status);
    }
    Ok(())
}

/// Loads the image specified by the device path (not yet supported) or slice.
/// * parent_image_handle - the handle of the image that is loading this one.
/// * device_path - optional device path describing where to load the image from.
/// * image - optional slice containing the image data.
///
/// One of `device_path` or `image` must be specified.
/// returns the image handle of the freshly loaded image.
pub fn core_load_image(
    boot_policy: bool,
    parent_image_handle: efi::Handle,
    device_path: *mut efi::protocols::device_path::Protocol,
    image: Option<&[u8]>,
) -> Result<(efi::Handle, efi::Status), efi::Status> {
    if image.is_none() && device_path.is_null() {
        log::error!("failed to load image: image is none or device path is null.");
        return Err(efi::Status::INVALID_PARAMETER);
    }

    PROTOCOL_DB
        .validate_handle(parent_image_handle)
        .inspect_err(|err| log::error!("failed to load image: invalid handle: {:#x?}", err))?;

    PROTOCOL_DB
        .get_interface_for_handle(parent_image_handle, efi::protocols::loaded_image::PROTOCOL_GUID)
        .inspect_err(|err| log::error!("failed to load image: failed to get loaded image interface: {:#x?}", err))
        .map_err(|_| efi::Status::INVALID_PARAMETER)?;

    let (image_to_load, from_fv, authentication_status) = match image {
        Some(image) => (image.to_vec(), false, 0),
        None => get_buffer_by_file_path(boot_policy, device_path)?,
    };

    // authenticate the image
    let security_status =
        match authenticate_image(device_path, &image_to_load, boot_policy, from_fv, authentication_status) {
            Ok(_) => efi::Status::SUCCESS,
            Err(efi::Status::SECURITY_VIOLATION) => efi::Status::SECURITY_VIOLATION,
            Err(err) => return Err(err),
        };

    // load the image.
    let mut image_info = empty_image_info();
    image_info.system_table = PRIVATE_IMAGE_DATA.lock().system_table;
    image_info.parent_handle = parent_image_handle;

    if !device_path.is_null() {
        if let Ok((_, handle)) = core_locate_device_path(efi::protocols::device_path::PROTOCOL_GUID, device_path) {
            image_info.device_handle = handle;
        }

        // extract file path here and set in image_info
        let (_, device_path_size) = device_path_node_count(device_path)?;
        let file_path_size: usize =
            device_path_size.saturating_sub(core::mem::size_of::<efi::protocols::device_path::Protocol>());
        let file_path = unsafe { (device_path as *const u8).add(file_path_size) };
        image_info.file_path = file_path as *mut efi::protocols::device_path::Protocol;
    }

    let mut private_info = core_load_pe_image(image_to_load.as_ref(), image_info)
        .inspect_err(|err| log::error!("failed to load image: core_load_pe_image failed: {:#x?}", err))?;

    let image_info_ptr = private_info.image_info.as_ref() as *const efi::protocols::loaded_image::Protocol;
    let image_info_ptr = image_info_ptr as *mut c_void;

    log::info!(
        "Loaded driver at {:#x?} EntryPoint={:#x?} {:}",
        private_info.image_info.image_base,
        private_info.entry_point as usize,
        private_info.pe_info.filename.as_ref().unwrap_or(&String::from("<no PDB>"))
    );

    // Notify the debugger of the image load.
    uefi_debugger::notify_module_load(
        private_info.pe_info.filename.as_ref().unwrap_or(&String::from("")),
        private_info.image_info.image_base as usize,
        private_info.image_info.image_size as usize,
    );

    // install the loaded_image protocol for this freshly loaded image on a new
    // handle.
    let handle = core_install_protocol_interface(None, efi::protocols::loaded_image::PROTOCOL_GUID, image_info_ptr)
        .inspect_err(|err| log::error!("failed to load image: install loaded image protocol failed: {:#x?}", err))?;

    // install the loaded_image device path protocol for the new image. If input device path is not null, then make a
    // permanent copy on the heap.
    let loaded_image_device_path = if device_path.is_null() {
        core::ptr::null_mut()
    } else {
        // make copy and convert to raw pointer to avoid drop at end of function.
        Box::into_raw(copy_device_path_to_boxed_slice(device_path)?) as *mut u8
    };

    core_install_protocol_interface(
        Some(handle),
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        loaded_image_device_path as *mut c_void,
    )
    .inspect_err(|err| log::error!("failed to load image: install device path failed: {:#x?}", err))?;

    if let Some(res_section) = private_info.hii_resource_section {
        core_install_protocol_interface(
            Some(handle),
            efi::protocols::hii_package_list::PROTOCOL_GUID,
            res_section as *mut c_void,
        )
        .inspect_err(|err| log::error!("failed to load image: install HII package list failed: {:#x?}", err))?;
    }

    // Store the interface pointers for unload to use when uninstalling these protocol interfaces.
    private_info.image_info_ptr = image_info_ptr;
    private_info.image_device_path_ptr = device_path as *mut c_void;

    // save the private image data for this image in the private image data map.
    PRIVATE_IMAGE_DATA.lock().private_image_data.insert(handle, private_info);

    // return the new handle.
    Ok((handle, security_status))
}

// Loads the image specified by the device_path (not yet supported) or
// source_buffer argument. See EFI_BOOT_SERVICES::LoadImage() API definition
// in UEFI spec for usage details.
// * boot_policy - indicates whether the image is being loaded by the boot
//                 manager from the specified device path. ignored if
//                 source_buffer is not null.
// * parent_image_handle - the caller's image handle.
// * device_path - the file path from which the image is loaded.
// * source_buffer - if not null, pointer to the memory location containing the
//                   image to be loaded.
//  * source_size - size in bytes of source_buffer. ignored if source_buffer is
//                  null.
//  * image_handle - pointer to the returned image handle that is created on
//                   successful image load.
extern "efiapi" fn load_image(
    boot_policy: efi::Boolean,
    parent_image_handle: efi::Handle,
    device_path: *mut efi::protocols::device_path::Protocol,
    source_buffer: *mut c_void,
    source_size: usize,
    image_handle: *mut efi::Handle,
) -> efi::Status {
    if image_handle.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    let image = if source_buffer.is_null() {
        None
    } else {
        if source_size == 0 {
            return efi::Status::LOAD_ERROR;
        }
        Some(unsafe { from_raw_parts(source_buffer as *const u8, source_size) })
    };

    match core_load_image(boot_policy.into(), parent_image_handle, device_path, image) {
        Err(err) => err,
        Ok((handle, security_status)) => unsafe {
            image_handle.write(handle);
            security_status
        },
    }
}

// Transfers control to the entry point of an image that was loaded by
// load_image. See EFI_BOOT_SERVICES::StartImage() API definition in UEFI spec
// for usage details.
// * image_handle - handle of the image to be started.
// * exit_data_size - pointer to receive the size, in bytes, of exit_data.
//                    if exit_data is null, this is parameter is ignored.
// * exit_data - pointer to receive a data buffer with exit data, if any.
extern "efiapi" fn start_image(
    image_handle: efi::Handle,
    exit_data_size: *mut usize,
    exit_data: *mut *mut efi::Char16,
) -> efi::Status {
    let status = core_start_image(image_handle);

    // retrieve any exit data that was provided by the entry point.
    if !exit_data_size.is_null() && !exit_data.is_null() {
        let private_data = PRIVATE_IMAGE_DATA.lock();
        if let Some(image_data) = private_data.private_image_data.get(&image_handle) {
            if let Some(image_exit_data) = image_data.exit_data {
                unsafe {
                    exit_data_size.write(image_exit_data.0);
                    exit_data.write(image_exit_data.1);
                }
            }
        }
    }

    let image_type = PRIVATE_IMAGE_DATA.lock().private_image_data.get(&image_handle).map(|x| x.pe_info.image_type);

    if status.is_err() || image_type == Some(EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION) {
        let _result = core_unload_image(image_handle, true);
    }

    match status {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

pub fn core_start_local_image(component: &'static dyn DxeComponent) -> Result<(), efi::Status> {
    // we get an NX stack for "free" because new pages area allocated efi::MEMORY_XP by default
    let stack = ImageStack::new(ENTRY_POINT_STACK_SIZE)?;

    let mut coroutine =
        Coroutine::with_stack(stack, move |_: &Yielder<&dyn DxeComponent, crate::error::Result<()>>, component| {
            component.entry_point(&component_interface::ComponentInterface)?;
            Ok::<(), crate::error::EfiError>(())
        });

    let status = match coroutine.resume(component) {
        CoroutineResult::Yield(status) => status,
        // Note: `CoroutineResult::Return` is unexpected, since it would imply
        // that exit() failed. TODO: should panic here?
        CoroutineResult::Return(status) => status,
    };

    match status {
        Ok(()) => Ok(()),
        Err(_) => Err(efi::Status::LOAD_ERROR),
    }
}

pub fn core_start_image(image_handle: efi::Handle) -> Result<(), efi::Status> {
    PROTOCOL_DB.validate_handle(image_handle)?;

    if let Some(private_data) = PRIVATE_IMAGE_DATA.lock().private_image_data.get_mut(&image_handle) {
        if private_data.started {
            Err(efi::Status::INVALID_PARAMETER)?;
        }
    } else {
        Err(efi::Status::INVALID_PARAMETER)?;
    }

    // allocate a buffer for the entry point stack.
    let stack = ImageStack::new(ENTRY_POINT_STACK_SIZE)?;

    // define a co-routine that wraps the entry point execution. this doesn't
    // run until the coroutine.resume() call below.
    let mut coroutine = Coroutine::with_stack(stack, move |yielder, image_handle| {
        let mut private_data = PRIVATE_IMAGE_DATA.lock();

        // mark the image as started and grab a copy of the private info.
        let status;
        if let Some(private_info) = private_data.private_image_data.get_mut(&image_handle) {
            private_info.started = true;
            let entry_point = private_info.entry_point;

            // save a pointer to the yielder so that exit() can use it.
            private_data.image_start_contexts.push(yielder as *const Yielder<_, _>);

            // get a copy of the system table pointer to pass to the entry point.
            let system_table = private_data.system_table;
            // drop our reference to the private data (i.e. release the lock).
            drop(private_data);

            // invoke the entry point. Code on the other side of this pointer is
            // FFI, which is inherently unsafe, but it's not  "technically" unsafe
            // from a rust standpoint since r_efi doesn't define the ImageEntryPoint
            // pointer type as "pointer to unsafe function"
            status = entry_point(image_handle, system_table);

            //safety note: any variables with "Drop" routines that need to run
            //need to be explicitly dropped before calling exit(). Since exit()
            //effectively "longjmp"s back to StartImage(), rust automatic
            //drops will not be triggered.
            exit(image_handle, status, 0, core::ptr::null_mut());
        } else {
            status = efi::Status::NOT_FOUND;
        }
        status
    });

    // Save the handle of the previously running image and update the currently
    // running image to the one we are about to invoke. In the event of nested
    // calls to StartImage(), the chain of previously running images will
    // be preserved on the stack of the various StartImage() instances.
    let mut private_data = PRIVATE_IMAGE_DATA.lock();
    let previous_image = private_data.current_running_image;
    private_data.current_running_image = Some(image_handle);
    drop(private_data);

    // switch stacks and execute the above defined coroutine to start the image.
    let status = match coroutine.resume(image_handle) {
        CoroutineResult::Yield(status) => status,
        // Note: `CoroutineResult::Return` is unexpected, since it would imply
        // that exit() failed. TODO: should panic here?
        CoroutineResult::Return(status) => status,
    };

    log::info!("start_image entrypoint exit with status: {:x?}", status);

    // because we used exit() to return from the coroutine (as opposed to
    // returning naturally from it), the coroutine is marked as suspended rather
    // than complete. We need to forcibly mark the coroutine done; otherwise it
    // will try to use unwind to clean up the co-routine stack (i.e. "drop" any
    // live objects). This unwind support requires std and will panic if
    // executed.
    unsafe { coroutine.force_reset() };

    PRIVATE_IMAGE_DATA.lock().current_running_image = previous_image;
    match status {
        efi::Status::SUCCESS => Ok(()),
        err => Err(err),
    }
}

pub fn core_unload_image(image_handle: efi::Handle, force_unload: bool) -> Result<(), efi::Status> {
    PROTOCOL_DB.validate_handle(image_handle)?;
    let private_data = PRIVATE_IMAGE_DATA.lock();
    let private_image_data =
        private_data.private_image_data.get(&image_handle).ok_or(efi::Status::INVALID_PARAMETER)?;
    let unload_function = private_image_data.image_info.unload;
    let started = private_image_data.started;
    drop(private_data); // release the image lock while unload logic executes as this function may be re-entrant.

    // if the image has been started, request that it unload, and don't unload it if
    // the unload function doesn't exist or returns an error.
    if started {
        if let Some(function) = unload_function {
            //Safety: this is unsafe (even though rust doesn't think so) because we are calling
            //into the "unload" function pointer that the image itself set. r_efi doesn't mark
            //the unload function type as unsafe - so rust reports an "unused_unsafe" since it
            //doesn't know it's unsafe. We suppress the warning and mark it unsafe anyway as a
            //warning to the future.
            #[allow(unused_unsafe)]
            unsafe {
                let status = (function)(image_handle);
                if status != efi::Status::SUCCESS {
                    Err(status)?;
                }
            }
        } else if !force_unload {
            Err(efi::Status::UNSUPPORTED)?;
        }
    }
    let handles = PROTOCOL_DB.locate_handles(None).unwrap_or_default();

    // close any protocols opened by this image.
    for handle in handles {
        let protocols = match PROTOCOL_DB.get_protocols_on_handle(handle) {
            Err(_) => continue,
            Ok(protocols) => protocols,
        };
        for protocol in protocols {
            let open_infos = match PROTOCOL_DB.get_open_protocol_information_by_protocol(handle, protocol) {
                Err(_) => continue,
                Ok(open_infos) => open_infos,
            };
            for open_info in open_infos {
                if Some(image_handle) == open_info.agent_handle {
                    let _result = PROTOCOL_DB.remove_protocol_usage(
                        handle,
                        protocol,
                        open_info.agent_handle,
                        open_info.controller_handle,
                    );
                }
            }
        }
    }

    // remove the private data for this image from the private_image_data map.
    // it will get dropped when it goes out of scope at the end of the function and the pages allocated for it
    // and the image_info box along with it.
    let private_image_data = PRIVATE_IMAGE_DATA.lock().private_image_data.remove(&image_handle).unwrap();
    // remove the image and device path protocols from the image handle.
    let _ = PROTOCOL_DB.uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image::PROTOCOL_GUID,
        private_image_data.image_info_ptr,
    );

    let _ = PROTOCOL_DB.uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        private_image_data.image_device_path_ptr,
    );

    // we have to remove the memory protections from the image sections before freeing the image buffer, because
    // core_free_pages expects the memory being freed to be in a single continuous memory descriptor, which is not
    // true when we've changed the attributes per section
    remove_image_memory_protections(&private_image_data.pe_info, &private_image_data);

    Ok(())
}

extern "efiapi" fn unload_image(image_handle: efi::Handle) -> efi::Status {
    match core_unload_image(image_handle, false) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => err,
    }
}

// Terminates a loaded EFI image and returns control to boot services.
// See EFI_BOOT_SERVICES::Exit() API definition in UEFI spec for usage details.
// * image_handle - the handle of the currently running image.
// * exit_status - the exit status for the image.
// * exit_data_size - the size of the exit_data buffer, if exit_data is not
//                    null.
// * exit_data - optional buffer of data provided by the caller.
extern "efiapi" fn exit(
    image_handle: efi::Handle,
    status: efi::Status,
    exit_data_size: usize,
    exit_data: *mut efi::Char16,
) -> efi::Status {
    let started = match PRIVATE_IMAGE_DATA.lock().private_image_data.get(&image_handle) {
        Some(image_data) => image_data.started,
        None => return efi::Status::INVALID_PARAMETER,
    };

    // if not started, just unload the image.
    if !started {
        return match core_unload_image(image_handle, true) {
            Ok(()) => efi::Status::SUCCESS,
            Err(_err) => efi::Status::INVALID_PARAMETER,
        };
    }

    // image has been started - check the currently running image.
    let mut private_data = PRIVATE_IMAGE_DATA.lock();
    if Some(image_handle) != private_data.current_running_image {
        return efi::Status::INVALID_PARAMETER;
    }

    // save the exit data, if present, into the private_image_data for this
    // image for start_image to retrieve and return.
    if (exit_data_size != 0) && !exit_data.is_null() {
        if let Some(image_data) = private_data.private_image_data.get_mut(&image_handle) {
            image_data.exit_data = Some((exit_data_size, exit_data));
        }
    }

    // retrieve the yielder that was saved in the start_image entry point
    // coroutine wrapper.
    // safety note: this assumes that the top of the image_start_contexts stack
    // is the currently running image.
    if let Some(yielder) = private_data.image_start_contexts.pop() {
        let yielder = unsafe { &*yielder };
        drop(private_data);

        // safety note: any variables with "Drop" routines that need to run
        // need to be explicitly dropped before calling suspend(). Since suspend()
        // effectively "longjmp"s back to StartImage(), rust automatic
        // drops will not be triggered.

        // transfer control back to start_image by calling the suspend function on
        // yielder. This will switch stacks back to the start_image that invoked
        // the entry point coroutine.
        yielder.suspend(status);
    }

    //should never reach here, but rust doesn't know that.
    efi::Status::ACCESS_DENIED
}

/// Initializes image services for the DXE core.
pub fn init_image_support(hob_list: &HobList, system_table: &mut EfiSystemTable) {
    // initialize system table entry in private global.
    let mut private_data = PRIVATE_IMAGE_DATA.lock();
    private_data.system_table = system_table.as_ptr() as *mut efi::SystemTable;
    drop(private_data);

    // install the image protocol for the dxe_core.
    install_dxe_core_image(hob_list);

    //set up imaging services
    system_table.boot_services().load_image = load_image;
    system_table.boot_services().start_image = start_image;
    system_table.boot_services().unload_image = unload_image;
    system_table.boot_services().exit = exit;
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::{empty_image_info, get_buffer_by_file_path, load_image};
    use crate::{
        image::{exit, start_image, unload_image, PRIVATE_IMAGE_DATA},
        protocol_db,
        protocols::{core_install_protocol_interface, PROTOCOL_DB},
        systemtables::{init_system_table, SYSTEM_TABLE},
        test_collateral, test_support,
    };
    use core::{ffi::c_void, sync::atomic::AtomicBool};
    use r_efi::efi;
    use std::{fs::File, io::Read};

    fn with_locked_state<F: Fn()>(f: F) {
        test_support::with_global_lock(|| unsafe {
            test_support::init_test_gcd(None);
            test_support::init_test_protocol_db();
            init_system_table();
            init_test_image_support();
            f();
        });
    }

    unsafe fn init_test_image_support() {
        PRIVATE_IMAGE_DATA.lock().reset();

        const DXE_CORE_MEMORY_SIZE: usize = 0x10000;
        let dxe_core_memory_base: Vec<u64> = Vec::with_capacity(DXE_CORE_MEMORY_SIZE);

        let mut private_data = PRIVATE_IMAGE_DATA.lock();
        let mut binding = SYSTEM_TABLE.lock();
        let system_table = binding.as_mut().unwrap();
        private_data.system_table = system_table.as_ptr() as *mut efi::SystemTable;

        let mut image_info = empty_image_info();
        image_info.system_table = private_data.system_table;
        image_info.image_base = dxe_core_memory_base.as_ptr() as *mut c_void;
        image_info.image_size = DXE_CORE_MEMORY_SIZE as u64;

        let image_info_ptr = &image_info as *const efi::protocols::loaded_image::Protocol;
        let image_info_ptr = image_info_ptr as *mut c_void;

        // install the loaded_image protocol on a new handle.
        let _ = match core_install_protocol_interface(
            Some(protocol_db::DXE_CORE_HANDLE),
            efi::protocols::loaded_image::PROTOCOL_GUID,
            image_info_ptr,
        ) {
            Err(err) => panic!("Failed to install dxe core image handle: {:?}", err),
            Ok(handle) => handle,
        };

        //set up imaging services
        system_table.boot_services().load_image = load_image;
        system_table.boot_services().start_image = start_image;
        system_table.boot_services().unload_image = unload_image;
        system_table.boot_services().exit = exit;
    }

    #[test]
    fn load_image_should_load_the_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("test_image_msvc_hii.pe32")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            let private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get(&image_handle).unwrap();
            let image_buf_len = unsafe { (*image_data.image_buffer).len() as usize };
            assert_eq!(image_buf_len, image_data.image_info.image_size as usize);
            assert_eq!(image_data.image_info.image_data_type, efi::BOOT_SERVICES_DATA);
            assert_eq!(image_data.image_info.image_code_type, efi::BOOT_SERVICES_CODE);
            assert_ne!(image_data.entry_point as usize, 0);
            assert!(!image_data.relocation_data.is_empty());
            assert!(image_data.hii_resource_section.is_some());
        });
    }

    #[test]
    fn load_image_should_authenticate_the_image_with_security_arch() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("test_image_msvc_hii.pe32")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            // Mock Security Arch protocol
            static SECURITY_CALL_EXECUTED: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn mock_file_authentication_state(
                this: *mut mu_pi::protocols::security::Protocol,
                authentication_status: u32,
                file: *mut efi::protocols::device_path::Protocol,
            ) -> efi::Status {
                assert!(!this.is_null());
                assert_eq!(authentication_status, 0);
                assert!(file.is_null()); //null device path passed to core_load_image, below.
                SECURITY_CALL_EXECUTED.store(true, core::sync::atomic::Ordering::SeqCst);
                efi::Status::SUCCESS
            }

            let security_protocol =
                mu_pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    mu_pi::protocols::security::PROTOCOL_GUID,
                    &security_protocol as *const _ as *mut _,
                )
                .unwrap();

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            assert!(SECURITY_CALL_EXECUTED.load(core::sync::atomic::Ordering::SeqCst));

            let private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get(&image_handle).unwrap();
            let image_buf_len = unsafe { (*image_data.image_buffer).len() as usize };
            assert_eq!(image_buf_len, image_data.image_info.image_size as usize);
            assert_eq!(image_data.image_info.image_data_type, efi::BOOT_SERVICES_DATA);
            assert_eq!(image_data.image_info.image_code_type, efi::BOOT_SERVICES_CODE);
            assert_ne!(image_data.entry_point as usize, 0);
            assert!(!image_data.relocation_data.is_empty());
            assert!(image_data.hii_resource_section.is_some());
        });
    }

    #[test]
    fn load_image_should_authenticate_the_image_with_security2_arch() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("test_image_msvc_hii.pe32")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            // Mock Security Arch protocol
            extern "efiapi" fn mock_file_authentication_state(
                _this: *mut mu_pi::protocols::security::Protocol,
                _authentication_status: u32,
                _file: *mut efi::protocols::device_path::Protocol,
            ) -> efi::Status {
                // should not be called, since `from_fv` is not presently true in our implementation for any
                // source of FV, which means only Security2 should be used.
                unreachable!()
            }

            let security_protocol =
                mu_pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    mu_pi::protocols::security::PROTOCOL_GUID,
                    &security_protocol as *const _ as *mut _,
                )
                .unwrap();

            // Mock Security2 Arch protocol
            static SECURITY2_CALL_EXECUTED: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn mock_file_authentication(
                this: *mut mu_pi::protocols::security2::Protocol,
                file: *mut efi::protocols::device_path::Protocol,
                file_buffer: *mut c_void,
                file_size: usize,
                boot_policy: bool,
            ) -> efi::Status {
                assert!(!this.is_null());
                assert!(file.is_null()); //null device path passed to core_load_image, below.
                assert!(!file_buffer.is_null());
                assert!(file_size > 0);
                assert!(!boot_policy);
                SECURITY2_CALL_EXECUTED.store(true, core::sync::atomic::Ordering::SeqCst);
                efi::Status::SUCCESS
            }

            let security2_protocol =
                mu_pi::protocols::security2::Protocol { file_authentication: mock_file_authentication };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    mu_pi::protocols::security2::PROTOCOL_GUID,
                    &security2_protocol as *const _ as *mut _,
                )
                .unwrap();

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            assert!(SECURITY2_CALL_EXECUTED.load(core::sync::atomic::Ordering::SeqCst));

            let private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get(&image_handle).unwrap();
            let image_buf_len = unsafe { (*image_data.image_buffer).len() as usize };
            assert_eq!(image_buf_len, image_data.image_info.image_size as usize);
            assert_eq!(image_data.image_info.image_data_type, efi::BOOT_SERVICES_DATA);
            assert_eq!(image_data.image_info.image_code_type, efi::BOOT_SERVICES_CODE);
            assert_ne!(image_data.entry_point as usize, 0);
            assert!(!image_data.relocation_data.is_empty());
            assert!(image_data.hii_resource_section.is_some());
        });
    }

    #[test]
    fn start_image_should_start_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            // Getting the image loaded into a buffer that is executable would require OS-specific interactions. This means that
            // all the memory backing our test GCD instance is likely to be marked "NX" - which makes it hard for start_image to
            // jump to it.
            // To allow testing of start_image, override the image entrypoint pointer so that it points to a stub routine
            // in this test - because it is part of the test executable and not part of the "load_image" buffer, it can be
            // executed.
            static ENTRY_POINT_RAN: AtomicBool = AtomicBool::new(false);
            pub extern "efiapi" fn test_entry_point(
                _image_handle: *mut core::ffi::c_void,
                _system_table: *mut r_efi::system::SystemTable,
            ) -> efi::Status {
                println!("test_entry_point executed.");
                ENTRY_POINT_RAN.store(true, core::sync::atomic::Ordering::Relaxed);
                efi::Status::SUCCESS
            }
            let mut private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get_mut(&image_handle).unwrap();
            image_data.entry_point = test_entry_point;
            drop(private_data);

            let mut exit_data_size = 0;
            let mut exit_data: *mut u16 = core::ptr::null_mut();
            let status =
                start_image(image_handle, core::ptr::addr_of_mut!(exit_data_size), core::ptr::addr_of_mut!(exit_data));
            assert_eq!(status, efi::Status::SUCCESS);
            assert!(ENTRY_POINT_RAN.load(core::sync::atomic::Ordering::Relaxed));

            let mut private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get_mut(&image_handle).unwrap();
            assert!(image_data.started);
            drop(private_data);
        });
    }

    #[test]
    fn start_image_error_status_should_unload_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            // Getting the image loaded into a buffer that is executable would require OS-specific interactions. This means that
            // all the memory backing our test GCD instance is likely to be marked "NX" - which makes it hard for start_image to
            // jump to it.
            // To allow testing of start_image, override the image entrypoint pointer so that it points to a stub routine
            // in this test - because it is part of the test executable and not part of the "load_image" buffer, it will not be
            // in memory marked NX and can be executed. Since this test is designed to test the load and start framework and not
            // the test driver, this will not reduce coverage of what is being tested here.
            static ENTRY_POINT_RAN: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn test_entry_point(
                _image_handle: *mut core::ffi::c_void,
                _system_table: *mut r_efi::system::SystemTable,
            ) -> efi::Status {
                log::info!("test_entry_point executed.");
                ENTRY_POINT_RAN.store(true, core::sync::atomic::Ordering::Relaxed);
                efi::Status::UNSUPPORTED
            }
            let mut private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get_mut(&image_handle).unwrap();
            image_data.entry_point = test_entry_point;
            drop(private_data);

            let mut exit_data_size = 0;
            let mut exit_data: *mut u16 = core::ptr::null_mut();
            let status =
                start_image(image_handle, core::ptr::addr_of_mut!(exit_data_size), core::ptr::addr_of_mut!(exit_data));
            assert_eq!(status, efi::Status::UNSUPPORTED);
            assert!(ENTRY_POINT_RAN.load(core::sync::atomic::Ordering::Relaxed));

            let private_data = PRIVATE_IMAGE_DATA.lock();
            assert!(!private_data.private_image_data.contains_key(&image_handle));
            drop(private_data);
        });
    }

    #[test]
    fn unload_non_started_image_should_unload_the_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );
            assert_eq!(status, efi::Status::SUCCESS);

            let status = unload_image(image_handle);
            assert_eq!(status, efi::Status::SUCCESS);

            let private_data = PRIVATE_IMAGE_DATA.lock();
            assert!(!private_data.private_image_data.contains_key(&image_handle));
        });
    }

    #[test]
    fn get_buffer_by_file_path_should_fail_if_no_file_support() {
        with_locked_state(|| {
            assert_eq!(get_buffer_by_file_path(true, core::ptr::null_mut()), Err(efi::Status::INVALID_PARAMETER));

            //build a device path as a byte array for the test.
            let mut device_path_bytes = [
                efi::protocols::device_path::TYPE_MEDIA,
                efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
                0x8, //length[0]
                0x0, //length[1]
                0x41,
                0x00, //'A' (as CHAR16)
                0x00,
                0x00, //NULL (as CHAR16)
                efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
                0x8, //length[0]
                0x0, //length[1]
                0x42,
                0x00, //'B' (as CHAR16)
                0x00,
                0x00, //NULL (as CHAR16)
                efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
                0x8, //length[0]
                0x0, //length[1]
                0x43,
                0x00, //'C' (as CHAR16)
                0x00,
                0x00, //NULL (as CHAR16)
                efi::protocols::device_path::TYPE_END,
                efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                0x4,  //length[0]
                0x00, //length[1]
            ];
            let device_path_ptr = device_path_bytes.as_mut_ptr() as *mut efi::protocols::device_path::Protocol;

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Err(efi::Status::NOT_FOUND));
        });
    }

    // mock file support.
    extern "efiapi" fn file_read(
        _this: *mut efi::protocols::file::Protocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> efi::Status {
        let mut test_file = File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
        unsafe {
            let slice = core::slice::from_raw_parts_mut(buffer as *mut u8, *buffer_size);
            let read_bytes = test_file.read(slice).unwrap();
            buffer_size.write(read_bytes);
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn file_close(_this: *mut efi::protocols::file::Protocol) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn file_info(
        _this: *mut efi::protocols::file::Protocol,
        _prot: *mut efi::Guid,
        size: *mut usize,
        buffer: *mut c_void,
    ) -> efi::Status {
        let test_file = File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
        let file_info = efi::protocols::file::Info {
            size: core::mem::size_of::<efi::protocols::file::Info>() as u64,
            file_size: test_file.metadata().unwrap().len(),
            physical_size: test_file.metadata().unwrap().len(),
            create_time: Default::default(),
            last_access_time: Default::default(),
            modification_time: Default::default(),
            attribute: 0,
            file_name: [0; 0],
        };
        let file_info_ptr = Box::into_raw(Box::new(file_info));

        let mut status = efi::Status::SUCCESS;
        unsafe {
            if *size >= (*file_info_ptr).size.try_into().unwrap() {
                core::ptr::copy(file_info_ptr, buffer as *mut efi::protocols::file::Info, 1);
            } else {
                status = efi::Status::BUFFER_TOO_SMALL;
            }
            size.write((*file_info_ptr).size.try_into().unwrap());
        }

        status
    }

    extern "efiapi" fn file_open(
        _this: *mut efi::protocols::file::Protocol,
        new_handle: *mut *mut efi::protocols::file::Protocol,
        _filename: *mut efi::Char16,
        _open_mode: u64,
        _attributes: u64,
    ) -> efi::Status {
        let file_ptr = get_file_protocol_mock();
        unsafe {
            new_handle.write(file_ptr);
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn file_set_position(_this: *mut efi::protocols::file::Protocol, _pos: u64) -> efi::Status {
        efi::Status::SUCCESS
    }

    extern "efiapi" fn unimplemented_extern() {
        unimplemented!();
    }

    fn get_file_protocol_mock() -> *mut efi::protocols::file::Protocol {
        // mock file interface
        #[allow(clippy::missing_transmute_annotations)]
        let file = efi::protocols::file::Protocol {
            revision: efi::protocols::file::LATEST_REVISION,
            open: file_open,
            close: file_close,
            delete: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            read: file_read,
            write: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            get_position: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            set_position: file_set_position,
            get_info: file_info,
            set_info: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            flush: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            open_ex: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            read_ex: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            write_ex: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
            flush_ex: unsafe { core::mem::transmute(unimplemented_extern as extern "efiapi" fn()) },
        };
        //deliberately leak for simplicity.
        Box::into_raw(Box::new(file))
    }

    //build a "root device path". Note that for simplicity, this doesn't model a typical device path which would be
    //more complex than this.
    const ROOT_DEVICE_PATH_BYTES: [u8; 12] = [
        efi::protocols::device_path::TYPE_MEDIA,
        efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
        0x8, //length[0]
        0x0, //length[1]
        0x41,
        0x00, //'A' (as CHAR16)
        0x00,
        0x00, //NULL (as CHAR16)
        efi::protocols::device_path::TYPE_END,
        efi::protocols::device_path::End::SUBTYPE_ENTIRE,
        0x4,  //length[0]
        0x00, //length[1]
    ];

    //build a full device path (note: not intended to be necessarily what would happen on a real system, which would
    //potentially have a larger device path e.g. with hardware nodes etc).
    const FULL_DEVICE_PATH_BYTES: [u8; 28] = [
        efi::protocols::device_path::TYPE_MEDIA,
        efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
        0x8, //length[0]
        0x0, //length[1]
        0x41,
        0x00, //'A' (as CHAR16)
        0x00,
        0x00, //NULL (as CHAR16)
        efi::protocols::device_path::TYPE_MEDIA,
        efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
        0x8, //length[0]
        0x0, //length[1]
        0x42,
        0x00, //'B' (as CHAR16)
        0x00,
        0x00, //NULL (as CHAR16)
        efi::protocols::device_path::TYPE_MEDIA,
        efi::protocols::device_path::Media::SUBTYPE_FILE_PATH,
        0x8, //length[0]
        0x0, //length[1]
        0x43,
        0x00, //'C' (as CHAR16)
        0x00,
        0x00, //NULL (as CHAR16)
        efi::protocols::device_path::TYPE_END,
        efi::protocols::device_path::End::SUBTYPE_ENTIRE,
        0x4,  //length[0]
        0x00, //length[1]
    ];

    #[test]
    fn get_buffer_by_file_path_should_work_over_sfs() {
        with_locked_state(|| {
            extern "efiapi" fn open_volume(
                _this: *mut efi::protocols::simple_file_system::Protocol,
                root: *mut *mut efi::protocols::file::Protocol,
            ) -> efi::Status {
                let file_ptr = get_file_protocol_mock();
                unsafe {
                    root.write(file_ptr);
                }
                efi::Status::SUCCESS
            }

            //build a mock SFS protocol.
            let protocol = efi::protocols::simple_file_system::Protocol {
                revision: efi::protocols::simple_file_system::REVISION,
                open_volume,
            };

            //Note: deliberate leak for simplicity.
            let protocol_ptr = Box::into_raw(Box::new(protocol));
            let handle = core_install_protocol_interface(
                None,
                efi::protocols::simple_file_system::PROTOCOL_GUID,
                protocol_ptr as *mut c_void,
            )
            .unwrap();

            //deliberate leak
            let root_device_path_ptr = Box::into_raw(Box::new(ROOT_DEVICE_PATH_BYTES)) as *mut u8
                as *mut efi::protocols::device_path::Protocol;

            core_install_protocol_interface(
                Some(handle),
                efi::protocols::device_path::PROTOCOL_GUID,
                root_device_path_ptr as *mut c_void,
            )
            .unwrap();

            let mut full_device_path_bytes = FULL_DEVICE_PATH_BYTES;

            let device_path_ptr = full_device_path_bytes.as_mut_ptr() as *mut efi::protocols::device_path::Protocol;

            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Ok((image, false, 0)));
        });
    }

    #[test]
    fn get_buffer_by_file_path_should_work_over_load_protocol() {
        with_locked_state(|| {
            extern "efiapi" fn load_file(
                _this: *mut efi::protocols::load_file::Protocol,
                _file_path: *mut efi::protocols::device_path::Protocol,
                _boot_policy: efi::Boolean,
                buffer_size: *mut usize,
                buffer: *mut c_void,
            ) -> efi::Status {
                let mut test_file =
                    File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
                let status;
                unsafe {
                    if *buffer_size < test_file.metadata().unwrap().len() as usize {
                        buffer_size.write(test_file.metadata().unwrap().len() as usize);
                        status = efi::Status::BUFFER_TOO_SMALL;
                    } else {
                        let slice = core::slice::from_raw_parts_mut(buffer as *mut u8, *buffer_size);
                        let read_bytes = test_file.read(slice).unwrap();
                        buffer_size.write(read_bytes);
                        status = efi::Status::SUCCESS;
                    }
                }
                status
            }

            let protocol = efi::protocols::load_file::Protocol { load_file };
            //Note: deliberate leak for simplicity.
            let protocol_ptr = Box::into_raw(Box::new(protocol));
            let handle = core_install_protocol_interface(
                None,
                efi::protocols::load_file::PROTOCOL_GUID,
                protocol_ptr as *mut c_void,
            )
            .unwrap();

            //deliberate leak
            let root_device_path_ptr = Box::into_raw(Box::new(ROOT_DEVICE_PATH_BYTES)) as *mut u8
                as *mut efi::protocols::device_path::Protocol;

            core_install_protocol_interface(
                Some(handle),
                efi::protocols::device_path::PROTOCOL_GUID,
                root_device_path_ptr as *mut c_void,
            )
            .unwrap();

            let mut full_device_path_bytes = FULL_DEVICE_PATH_BYTES;

            let device_path_ptr = full_device_path_bytes.as_mut_ptr() as *mut efi::protocols::device_path::Protocol;

            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Ok((image, false, 0)));
        });
    }
}
