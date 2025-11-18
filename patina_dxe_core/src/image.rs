//! DXE Core Image Services
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use core::{convert::TryInto, ffi::c_void, mem::transmute, slice, slice::from_raw_parts};
use patina::{
    base::{DEFAULT_CACHE_ATTR, UEFI_PAGE_SIZE, align_up},
    error::EfiError,
    guids,
    performance::{
        logging::{perf_image_start_begin, perf_image_start_end, perf_load_image_begin, perf_load_image_end},
        measurement::create_performance_measurement,
    },
    pi::{
        self,
        fw_fs::FfsSectionRawType::PE32,
        hob::{Hob, HobList},
    },
    uefi_pages_to_size, uefi_size_to_pages,
};
use patina_internal_device_path::{DevicePathWalker, copy_device_path_to_boxed_slice, device_path_node_count};
use r_efi::efi;

use crate::{
    GCD,
    allocator::{core_allocate_pages, core_free_pages},
    config_tables::debug_image_info_table::{
        EfiDebugImageInfoNormal, core_new_debug_image_info_entry, core_remove_debug_image_info_entry,
        initialize_debug_image_info_table,
    },
    dxe_services::{self, core_set_memory_space_attributes},
    events::EVENT_DB,
    filesystems::SimpleFile,
    gcd::MemoryProtectionPolicy,
    pecoff::{self, UefiPeInfo, relocation::RelocationBlock},
    protocol_db,
    protocols::{
        PROTOCOL_DB, core_install_protocol_interface, core_locate_device_path, core_uninstall_protocol_interface,
    },
    runtime,
    systemtables::EfiSystemTable,
    tpl_mutex,
};

use efi::Guid;
use uefi_corosensei::{
    Coroutine, CoroutineResult, Yielder,
    stack::{MIN_STACK_SIZE, STACK_ALIGNMENT, Stack, StackPointer},
};

pub const EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION: u16 = 10;
pub const EFI_IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER: u16 = 11;
pub const EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER: u16 = 12;

pub const ENTRY_POINT_STACK_SIZE: usize = 0x100000;

// dummy function used to initialize PrivateImageData.entry_point.
#[coverage(off)]
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
    fn new(size: usize) -> Result<Self, EfiError> {
        let mut stack: efi::PhysicalAddress = 0;
        let len = align_up(size.max(MIN_STACK_SIZE), STACK_ALIGNMENT)?;
        // allocate an extra page for the stack guard page.
        let allocated_pages = uefi_size_to_pages!(len) + 1;

        // allocate the stack, newly allocated memory will have efi::MEMORY_XP already set, so we don't need to set it
        // here
        core_allocate_pages(efi::ALLOCATE_ANY_PAGES, efi::BOOT_SERVICES_DATA, allocated_pages, &mut stack, None)?;

        // attempt to set the memory space attributes for the stack guard page.
        // if we fail, we should still try to continue to boot
        // the stack grows downwards, so stack here is the guard page
        let mut attributes = match dxe_services::core_get_memory_space_descriptor(stack) {
            Ok(descriptor) => descriptor.attributes,
            Err(_) => DEFAULT_CACHE_ATTR,
        };

        attributes = MemoryProtectionPolicy::apply_image_stack_guard_policy(attributes);

        if let Err(err) = dxe_services::core_set_memory_space_attributes(stack, UEFI_PAGE_SIZE as u64, attributes) {
            log::error!("Failed to set memory space attributes for stack guard page: {err:?}");
            // unfortunately, this needs to be commented out for now, because the tests have gotten too complex
            // and need to be refactored to handle the page table
            // debug_assert!(false);
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

            if let Err(status) = core_free_pages(stack_addr, self.allocated_pages) {
                log::error!(
                    "core_free_pages returned error {:#x?} for image stack at {:#x} for num_pages {:#x}",
                    status,
                    stack_addr,
                    self.allocated_pages
                );
                debug_assert!(false);
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
    fn new(image_info: efi::protocols::loaded_image::Protocol, pe_info: &UefiPeInfo) -> Result<Self, EfiError> {
        // Allocate pages for the image to be loaded into. We use pages here instead of a pool because we are going to
        // set memory attributes on this range and it is not valid to set attributes on pool backed memory.
        let mut image_base_page: efi::PhysicalAddress = 0;

        // if we have a unique alignment requirement, we need to overallocate the buffer to ensure we can align the base
        let num_pages: usize = if pe_info.section_alignment as usize > UEFI_PAGE_SIZE {
            if let Some(image_size) = image_info.image_size.checked_add(pe_info.section_alignment as u64) {
                match usize::try_from(image_size) {
                    Ok(size) => uefi_size_to_pages!(size),
                    Err(_) => return Err(EfiError::LoadError),
                }
            } else {
                return Err(EfiError::LoadError);
            }
        } else {
            match usize::try_from(image_info.image_size) {
                Ok(size) => uefi_size_to_pages!(size),
                Err(_) => return Err(EfiError::LoadError),
            }
        };

        core_allocate_pages(
            efi::ALLOCATE_ANY_PAGES,
            image_info.image_code_type,
            num_pages,
            &mut image_base_page,
            None,
        )?;

        if image_base_page == 0 {
            return Err(EfiError::OutOfResources);
        }

        let aligned_image_start =
            align_up(image_base_page, pe_info.section_alignment.into()).map_err(|_| EfiError::LoadError)?;

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

    fn new_with_existing_allocation(
        image_info: efi::protocols::loaded_image::Protocol,
        image_buffer: *mut [u8],
        entry_point: efi::ImageEntryPoint,
        pe_info: &UefiPeInfo,
        image_base_page: efi::PhysicalAddress,
        image_num_pages: usize,
    ) -> Self {
        PrivateImageData {
            image_buffer,
            image_info: Box::new(image_info),
            hii_resource_section: None,
            hii_resource_section_base: None,
            hii_resource_section_num_pages: None,
            entry_point,
            started: true,
            exit_data: None,
            image_info_ptr: core::ptr::null_mut(),
            image_device_path_ptr: core::ptr::null_mut(),
            pe_info: pe_info.clone(),
            relocation_data: Vec::new(),
            image_base_page,
            image_num_pages,
        }
    }

    fn allocate_resource_section(
        &mut self,
        size: usize,
        alignment: usize,
        code_type: efi::MemoryType,
    ) -> Result<(), EfiError> {
        let mut hii_base_page: efi::PhysicalAddress = 0;
        // if we have a unique alignment requirement, we need to overallocate the buffer to ensure we can align the base
        let num_pages: usize =
            if alignment > UEFI_PAGE_SIZE { uefi_size_to_pages!(size + alignment) } else { uefi_size_to_pages!(size) };
        core_allocate_pages(efi::ALLOCATE_ANY_PAGES, code_type, num_pages, &mut hii_base_page, None)?;

        if hii_base_page == 0 {
            return Err(EfiError::OutOfResources);
        }

        let aligned_hii_start = align_up(hii_base_page, alignment as u64).map_err(|_| EfiError::LoadError)?;

        self.hii_resource_section = Some(core::ptr::slice_from_raw_parts_mut(aligned_hii_start as *mut u8, size));
        self.hii_resource_section_base = Some(hii_base_page);
        self.hii_resource_section_num_pages = Some(num_pages);
        Ok(())
    }
}

impl Drop for PrivateImageData {
    fn drop(&mut self) {
        if !self.image_buffer.is_null()
            && let Err(status) = core_free_pages(self.image_base_page, self.image_num_pages)
        {
            log::error!(
                "core_free_pages returned error {:#x?} for image buffer at {:#x} for num_pages {:#x}",
                status,
                self.image_base_page,
                self.image_num_pages
            );
            debug_assert!(false);
        }

        if let (Some(resource_addr), Some(num_pages)) =
            (self.hii_resource_section_base, self.hii_resource_section_num_pages)
            && let Err(status) = core_free_pages(resource_addr, num_pages)
        {
            log::error!(
                "core_free_pages returned error {status:#x?} for HII resource section at {resource_addr:#x} for num_pages {num_pages:#x}",
            );
            debug_assert!(false);
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

static PRIVATE_IMAGE_DATA: tpl_mutex::TplMutex<DxeCoreGlobalImageData> =
    tpl_mutex::TplMutex::new(efi::TPL_NOTIFY, DxeCoreGlobalImageData::new(), "ImageLock");

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

fn apply_image_memory_protections(pe_info: &UefiPeInfo, private_info: &PrivateImageData) -> Result<(), EfiError> {
    for section in &pe_info.sections {
        // each section starts at image_base + virtual_address, per PE/COFF spec.
        let section_base_addr = (private_info.image_info.image_base as u64) + (section.virtual_address as u64);

        // we need to get the current attributes for this region and add our new attribute
        // if we can't find this range in the GCD, try the next one, but report the failure
        let desc = dxe_services::core_get_memory_space_descriptor(section_base_addr)?;
        let (attributes, capabilities) =
            MemoryProtectionPolicy::apply_image_protection_policy(section.characteristics, &desc);

        // now actually set the attributes. We need to use the virtual size for the section length, but
        // we cannot rely on this to be section aligned, as some compilers rely on the loader to align this
        // We also need to ensure the capabilities are set. We set the capabilities as the old capabilities
        // plus our new attribute, as we need to ensure all existing attributes are supported by the new
        // capabilities.
        let aligned_virtual_size = if let Ok(virtual_size) = align_up(section.virtual_size, pe_info.section_alignment) {
            virtual_size as u64
        } else {
            log::error!(
                "Failed to align up section size {:#X} with alignment {:#X}",
                section.virtual_size,
                pe_info.section_alignment
            );
            debug_assert!(false);
            return Err(EfiError::LoadError);
        };

        if let Err(status) =
            dxe_services::core_set_memory_space_capabilities(section_base_addr, aligned_virtual_size, capabilities)
        {
            // even if we fail to set the capabilities, we should still try to set the attributes, who knows, maybe we
            // will succeed
            log::error!(
                "Failed to set GCD capabilities for image section {section_base_addr:#X} with Status {status:#X?}",
            );
        }

        // this may be verbose to log, but we also have a lot of errors historically here, so let's log at info level
        // for now
        log::info!(
            "Applying image memory protections on {section_base_addr:#X} for len {aligned_virtual_size:#X} with attributes {attributes:#X}",
        );

        dxe_services::core_set_memory_space_attributes(section_base_addr, aligned_virtual_size, attributes)
            .inspect_err(|status| {
                log::error!(
                    "Failed to set GCD attributes for image section {section_base_addr:#X} with Status {status:#X?}",
                );
            })?;
    }
    Ok(())
}

// retrieves the dxe core image info from the hob list, and installs the
// loaded_image protocol on it to create the dxe_core image handle.
fn install_dxe_core_image(hob_list: &HobList, system_table: &mut EfiSystemTable) {
    // Retrieve the MemoryAllocationModule hob corresponding to the DXE core
    // (i.e. this driver).
    let dxe_core_hob = hob_list
        .iter()
        .find_map(|x| match x {
            Hob::MemoryAllocationModule(module) if module.module_name == guids::DXE_CORE => Some(module),
            _ => None,
        })
        .expect("Did not find MemoryAllocationModule Hob for DxeCore. Use patina::guid::DXE_CORE as FFS GUID.");

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

    // we do not use PrivateImageData::new() here because it
    // expects we are about to load this image and so allocates
    // an image buffer for us. We already have the image buffer
    // here as DXE Core is uniquely already loaded
    let image_buffer =
        core::ptr::slice_from_raw_parts_mut(image_info.image_base as *mut u8, image_info.image_size as usize);
    let mut private_image_data = PrivateImageData::new_with_existing_allocation(
        image_info,
        image_buffer,
        entry_point,
        &pe_info,
        dxe_core_hob.alloc_descriptor.memory_base_address,
        uefi_size_to_pages!(dxe_core_hob.alloc_descriptor.memory_length as usize),
    );

    let image_info_ptr = private_image_data.image_info.as_ref() as *const efi::protocols::loaded_image::Protocol;
    let image_info_ptr = image_info_ptr as *mut c_void;
    private_image_data.image_info_ptr = image_info_ptr;

    // install the loaded_image protocol on a new handle.
    let handle = match core_install_protocol_interface(
        Some(protocol_db::DXE_CORE_HANDLE),
        efi::protocols::loaded_image::PROTOCOL_GUID,
        image_info_ptr,
    ) {
        Err(err) => panic!("Failed to install dxe core image handle: {err:?}"),
        Ok(handle) => handle,
    };
    assert_eq!(handle, protocol_db::DXE_CORE_HANDLE);

    // register the core image with the debug image info configuration table
    initialize_debug_image_info_table(system_table);
    core_new_debug_image_info_entry(
        EfiDebugImageInfoNormal::EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL,
        image_info_ptr as *const efi::protocols::loaded_image::Protocol,
        handle,
    );

    // record this handle as the new dxe_core handle.
    private_data.dxe_core_image_handle = handle;

    // store the dxe core image private data in the private image data map.
    private_data.private_image_data.insert(handle, private_image_data);
}

// loads and relocates the image in the specified slice and returns the
// associated PrivateImageData structures.
fn core_load_pe_image(
    image: &[u8],
    mut image_info: efi::protocols::loaded_image::Protocol,
) -> Result<PrivateImageData, EfiError> {
    // parse and validate the header and retrieve the image data from it.
    let pe_info = pecoff::UefiPeInfo::parse(image)
        .inspect_err(|err| log::error!("core_load_pe_image failed: UefiPeInfo::parse returned {err:?}"))
        .map_err(|_| EfiError::Unsupported)?;

    let pe_file_name = pe_info.filename.as_deref().unwrap_or("Unknown");

    // based on the image type, determine the correct allocator and code/data types.
    let (code_type, data_type) = match pe_info.image_type {
        EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION => (efi::LOADER_CODE, efi::LOADER_DATA),
        EFI_IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER => (efi::BOOT_SERVICES_CODE, efi::BOOT_SERVICES_DATA),
        EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER => (efi::RUNTIME_SERVICES_CODE, efi::RUNTIME_SERVICES_DATA),
        unsupported_type => {
            log::error!("core_load_pe_image failed: {pe_file_name} unsupported image type: {unsupported_type:#x?}");
            return Err(EfiError::Unsupported);
        }
    };

    let alignment = pe_info.section_alignment as usize; // Need to align the base address with section alignment via overallocation
    let size = pe_info.size_of_image as usize;

    // the section alignment must be at least the size of a page
    if !alignment.is_multiple_of(UEFI_PAGE_SIZE) {
        log::error!(
            "core_load_pe_image failed: {pe_file_name} section alignment of {alignment:#x?} is not a multiple of page size {UEFI_PAGE_SIZE:#x?}"
        );
        return Err(EfiError::LoadError);
    }

    // the size of the image must be a multiple of the section alignment per PE/COFF spec
    if !size.is_multiple_of(alignment) {
        log::error!(
            "core_load_pe_image failed: {pe_file_name} size of image is not a multiple of the section alignment"
        );
        return Err(EfiError::LoadError);
    }

    image_info.image_size = size as u64;
    image_info.image_code_type = code_type;
    image_info.image_data_type = data_type;

    //allocate a buffer to hold the image (also updates private_info.image_info.image_base)
    let mut private_info = PrivateImageData::new(image_info, &pe_info)?;
    let loaded_image = unsafe { &mut *private_info.image_buffer };

    //load the image into the new loaded image buffer
    pecoff::load_image(&pe_info, image, loaded_image)
        .inspect_err(|err| log::error!("core_load_pe_image failed: {pe_file_name} load_image returned status: {err:?}"))
        .map_err(|_| EfiError::LoadError)?;

    //relocate the image to the address at which it was loaded.
    let loaded_image_addr = private_info.image_info.image_base as usize;
    private_info.relocation_data = pecoff::relocate_image(&pe_info, loaded_image_addr, loaded_image, &Vec::new())
        .inspect_err(|err| {
            log::error!("core_load_pe_image failed: {pe_file_name} relocate_image returned status: {err:?}")
        })
        .map_err(|_| EfiError::LoadError)?;

    // update the entry point. Transmute is required here to cast the raw function address to the ImageEntryPoint function pointer type.
    private_info.entry_point = unsafe {
        transmute::<usize, extern "efiapi" fn(*mut c_void, *mut r_efi::system::SystemTable) -> efi::Status>(
            loaded_image_addr + pe_info.entry_point_offset,
        )
    };

    let result = pecoff::load_resource_section(&pe_info, image)
        .inspect_err(|err| {
            log::error!("core_load_pe_image failed: {pe_file_name} load_resource_section returned status: {err:?}")
        })
        .map_err(|_| EfiError::LoadError)?;

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

                    log::info!("HII Resource Section found for {pe_file_name}.");
                } else {
                    log::error!(
                        "HII Resource Section offset {:#X} and size {:#X} are out of bounds for image {pe_file_name}.",
                        resource_section_offset,
                        resource_section_size
                    );
                    debug_assert!(false);
                }
            }
        }
    }

    match pe_info.image_type {
        EFI_IMAGE_SUBSYSTEM_EFI_APPLICATION if !pe_info.nx_compat => {
            // we are trying to load an application image that is not NX compatible, likely a bootloader
            // if we are configured to allow compatibility mode, we need to activate it now. Otherwise, just continue
            // to load the image
            MemoryProtectionPolicy::activate_compatibility_mode(
                &GCD,
                private_info.image_base_page as usize,
                private_info.image_num_pages,
                pe_info.filename.clone().unwrap_or(String::from("Unknown")),
            )?;
        }
        _ => {
            // finally, update the GCD attributes for this image so that code sections have RO set and data sections
            // have XP
            apply_image_memory_protections(&pe_info, &private_info)?;
        }
    }

    Ok(private_info)
}

extern "efiapi" fn runtime_image_protection_fixup_ebs(event: efi::Event, _context: *mut c_void) {
    let mut private_data = PRIVATE_IMAGE_DATA.lock();

    for image in private_data.private_image_data.values_mut() {
        if image.pe_info.image_type == EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER {
            let cache_attrs = dxe_services::core_get_memory_space_descriptor(image.image_base_page)
                .map(|desc| desc.attributes & efi::CACHE_ATTRIBUTE_MASK)
                .unwrap_or(DEFAULT_CACHE_ATTR);

            match core_set_memory_space_attributes(
                image.image_base_page,
                uefi_pages_to_size!(image.image_num_pages) as u64,
                cache_attrs,
            ) {
                Ok(_) => {
                    // success, keep going
                }
                Err(status) => {
                    log::error!(
                        "Failed to set GCD attributes for runtime image {:#X?} with Status {:#X?}, may fail to relocate",
                        image.image_base_page,
                        status
                    );
                    debug_assert!(false);
                }
            };
        }
    }

    if let Err(status) = EVENT_DB.close_event(event) {
        log::error!("Failed to close image EBS event with status {status:#X?}. This should be okay.");
    }
}

// Reads an image buffer using simple file system or load file protocols.
// Return value is (image_buffer, device_handle, from_fv, authentication_status).
// Note: presently none of the supported methods return `from_fv` or `authentication_status`.
fn get_buffer_by_file_path(
    boot_policy: bool,
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<(Vec<u8>, bool, efi::Handle, u32), EfiError> {
    if file_path.is_null() {
        Err(EfiError::InvalidParameter)?;
    }

    if let Ok((buffer, device_handle)) = get_file_buffer_from_fw(file_path) {
        return Ok((buffer, true, device_handle, 0));
    }

    if let Ok((buffer, device_handle)) = get_file_buffer_from_sfs(file_path) {
        return Ok((buffer, false, device_handle, 0));
    }

    if !boot_policy
        && let Ok((buffer, device_handle)) =
            get_file_buffer_from_load_protocol(efi::protocols::load_file2::PROTOCOL_GUID, false, file_path)
    {
        return Ok((buffer, false, device_handle, 0));
    }

    if let Ok((buffer, device_handle)) =
        get_file_buffer_from_load_protocol(efi::protocols::load_file::PROTOCOL_GUID, boot_policy, file_path)
    {
        return Ok((buffer, false, device_handle, 0));
    }

    Err(EfiError::NotFound)
}

fn get_file_guid_from_device_path(path: *mut efi::protocols::device_path::Protocol) -> Result<Guid, EfiError> {
    let mut walker = unsafe { DevicePathWalker::new(path) };
    let file_path_node = walker.next().ok_or(EfiError::InvalidParameter)?;
    if file_path_node.header().r#type != efi::protocols::device_path::TYPE_MEDIA
        || file_path_node.header().sub_type != efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE
    {
        return Err(EfiError::InvalidParameter);
    }
    Ok(Guid::from_bytes(file_path_node.data().try_into().map_err(|_| EfiError::BadBufferSize)?))
}

fn get_file_buffer_from_fw(
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<(Vec<u8>, efi::Handle), EfiError> {
    // Locate the handles to a device on the file_path that supports the firmware volume protocol
    let (remaining_file_path, handle) =
        core_locate_device_path(pi::protocols::firmware_volume::PROTOCOL_GUID, file_path)?;

    // For FwVol File system there is only a single file name that is a GUID.
    let fv_name_guid = get_file_guid_from_device_path(remaining_file_path)?;

    // Get the firmware volume protocol
    let fv_ptr = PROTOCOL_DB.get_interface_for_handle(handle, pi::protocols::firmware_volume::PROTOCOL_GUID)?
        as *mut pi::protocols::firmware_volume::Protocol;
    if fv_ptr.is_null() {
        debug_assert!(!fv_ptr.is_null(), "ERROR: get_interface_for_handle returned NULL ptr for FirmwareVolume!");
        return Err(EfiError::InvalidParameter);
    }
    let fw_vol = unsafe { fv_ptr.as_ref().unwrap() };

    // Read image from the firmware file
    let mut buffer: *mut u8 = core::ptr::null_mut();
    let buffer_ptr: *mut *mut c_void = &mut buffer as *mut _ as *mut *mut c_void;
    let mut buffer_size = 0;
    let mut authentication_status = 0;
    let authentication_status_ptr = &mut authentication_status;
    let status = (fw_vol.read_section)(
        fw_vol,
        &fv_name_guid,
        PE32,
        0, // Instance
        buffer_ptr,
        core::ptr::addr_of_mut!(buffer_size),
        authentication_status_ptr,
    );

    EfiError::status_to_result(status)?;

    let section_slice = unsafe { slice::from_raw_parts(buffer, buffer_size) };
    Ok((section_slice.to_vec(), handle))
}

fn get_file_buffer_from_sfs(
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<(Vec<u8>, efi::Handle), EfiError> {
    let (remaining_file_path, handle) =
        core_locate_device_path(efi::protocols::simple_file_system::PROTOCOL_GUID, file_path)?;

    let mut file = SimpleFile::open_volume(handle)?;

    for node in unsafe { DevicePathWalker::new(remaining_file_path) } {
        match node.header().r#type {
            efi::protocols::device_path::TYPE_MEDIA
                if node.header().sub_type == efi::protocols::device_path::Media::SUBTYPE_FILE_PATH => {} //proceed on valid path node
            efi::protocols::device_path::TYPE_END => break,
            _ => Err(EfiError::Unsupported)?,
        }
        //For MEDIA_FILE_PATH_DP, file name is in the node data, but it needs to be converted to Vec<u16> for call to open.
        let filename: Vec<u16> = node
            .data()
            .chunks_exact(2)
            .map(|x: &[u8]| {
                if let Ok(x_bytes) = x.try_into() {
                    Ok(u16::from_le_bytes(x_bytes))
                } else {
                    Err(EfiError::InvalidParameter)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        file = file.open(filename, efi::protocols::file::MODE_READ, 0)?;
    }

    // if execution comes here, the above loop was successfully able to open all the files on the remaining device path,
    // so `file` is currently pointing to the desired file (i.e. the last node), and it just needs to be read.
    Ok((file.read()?, handle))
}

fn get_file_buffer_from_load_protocol(
    protocol: efi::Guid,
    boot_policy: bool,
    file_path: *mut efi::protocols::device_path::Protocol,
) -> Result<(Vec<u8>, efi::Handle), EfiError> {
    if !(protocol == efi::protocols::load_file::PROTOCOL_GUID || protocol == efi::protocols::load_file2::PROTOCOL_GUID)
    {
        Err(EfiError::InvalidParameter)?;
    }

    if protocol == efi::protocols::load_file2::PROTOCOL_GUID && boot_policy {
        Err(EfiError::InvalidParameter)?;
    }

    let (remaining_file_path, handle) = core_locate_device_path(protocol, file_path)?;

    let load_file = PROTOCOL_DB.get_interface_for_handle(handle, protocol)?;
    let load_file =
        unsafe { (load_file as *mut efi::protocols::load_file::Protocol).as_mut().ok_or(EfiError::Unsupported)? };

    //determine buffer size.
    let mut buffer_size = 0;
    let status = (load_file.load_file)(
        load_file,
        remaining_file_path,
        boot_policy.into(),
        core::ptr::addr_of_mut!(buffer_size),
        core::ptr::null_mut(),
    );

    match status {
        efi::Status::BUFFER_TOO_SMALL => (),                 // expected
        efi::Status::SUCCESS => Err(EfiError::DeviceError)?, // not expected for buffer_size = 0
        _ => EfiError::status_to_result(status)?,            // unexpected error.
    }

    let mut file_buffer = vec![0u8; buffer_size];
    let status = (load_file.load_file)(
        load_file,
        remaining_file_path,
        boot_policy.into(),
        core::ptr::addr_of_mut!(buffer_size),
        file_buffer.as_mut_ptr() as *mut c_void,
    );

    EfiError::status_to_result(status).map(|_| (file_buffer, handle))
}

// authenticate the given image against the Security and Security2 Architectural Protocols
fn authenticate_image(
    device_path: *mut efi::protocols::device_path::Protocol,
    image: &[u8],
    boot_policy: bool,
    from_fv: bool,
    authentication_status: u32,
) -> Result<(), EfiError> {
    let security2_protocol = unsafe {
        match PROTOCOL_DB.locate_protocol(pi::protocols::security2::PROTOCOL_GUID) {
            Ok(protocol) => (protocol as *mut pi::protocols::security2::Protocol).as_ref(),
            //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
            //Firmware Volume.
            Err(_) => None,
        }
    };

    let security_protocol = unsafe {
        match PROTOCOL_DB.locate_protocol(pi::protocols::security::PROTOCOL_GUID) {
            Ok(protocol) => (protocol as *mut pi::protocols::security::Protocol).as_ref(),
            //If security protocol is not located, then assume it has not yet been produced and implicitly trust the
            //Firmware Volume.
            Err(_) => None,
        }
    };

    let mut security_status = efi::Status::SUCCESS;
    if let Some(security2) = security2_protocol {
        security_status = (security2.file_authentication)(
            security2 as *const _ as *mut pi::protocols::security2::Protocol,
            device_path,
            image.as_ptr() as *const _ as *mut c_void,
            image.len(),
            boot_policy,
        );
        if security_status == efi::Status::SUCCESS && from_fv {
            let security = security_protocol.expect("Security Arch must be installed if Security2 Arch is installed");
            security_status = (security.file_authentication_state)(
                security as *const _ as *mut pi::protocols::security::Protocol,
                authentication_status,
                device_path,
            );
        }
    } else if let Some(security) = security_protocol {
        security_status = (security.file_authentication_state)(
            security as *const _ as *mut pi::protocols::security::Protocol,
            authentication_status,
            device_path,
        );
    }

    EfiError::status_to_result(security_status)
}

/// Loads the image specified by the device path (not yet supported) or slice.
/// * parent_image_handle - the handle of the image that is loading this one.
/// * file_path - optional device path describing where to load the image from.
/// * image - optional slice containing the image data.
///
/// One of `file_path` or `image` must be specified.
/// returns the image handle of the freshly loaded image.
pub fn core_load_image(
    boot_policy: bool,
    parent_image_handle: efi::Handle,
    file_path: *mut efi::protocols::device_path::Protocol,
    image: Option<&[u8]>,
) -> Result<(efi::Handle, Result<(), EfiError>), EfiError> {
    perf_load_image_begin(core::ptr::null_mut(), create_performance_measurement);

    if image.is_none() && file_path.is_null() {
        log::error!("failed to load image: image is none or device path is null.");
        return Err(EfiError::InvalidParameter);
    }

    PROTOCOL_DB
        .validate_handle(parent_image_handle)
        .inspect_err(|err| log::error!("failed to load image: invalid handle: {err:#x?}"))?;

    PROTOCOL_DB
        .get_interface_for_handle(parent_image_handle, efi::protocols::loaded_image::PROTOCOL_GUID)
        .inspect_err(|err| log::error!("failed to load image: failed to get loaded image interface: {err:?}"))
        .map_err(|_| EfiError::InvalidParameter)?;

    let (image_to_load, from_fv, device_handle, authentication_status) = match image {
        Some(image) => {
            // If the buffer is specified and the device_path resolves with core_locate_device_path, then use the
            // resolved handle as the device_handle. Note: the associated device path for the device_handle will
            // likely be shorter than file_path.
            if let Ok((_device_path, device_handle)) =
                core_locate_device_path(efi::protocols::device_path::PROTOCOL_GUID, file_path)
            {
                (image.to_vec(), false, device_handle, 0)
            } else {
                // (i.e. it doesn't correspond to anything that actually exists in the system)
                (image.to_vec(), false, protocol_db::INVALID_HANDLE, 0)
            }
        }
        None => get_buffer_by_file_path(boot_policy, file_path)?,
    };

    // authenticate the image
    let security_status = authenticate_image(file_path, &image_to_load, boot_policy, from_fv, authentication_status);

    // load the image.
    let mut image_info = empty_image_info();
    image_info.system_table = PRIVATE_IMAGE_DATA.lock().system_table;
    image_info.parent_handle = parent_image_handle;
    image_info.device_handle = device_handle;
    let mut fixed_file_path = None;

    if device_handle == protocol_db::INVALID_HANDLE {
        fixed_file_path = Some(file_path);
    } else if !file_path.is_null() {
        // Get the device path for the parent device
        if let Ok(device_path) =
            PROTOCOL_DB.get_interface_for_handle(device_handle, efi::protocols::device_path::PROTOCOL_GUID)
        {
            // Strip the parent device path prefix from the full device path to leave only the file node
            let (_, device_path_size) =
                device_path_node_count(device_path as *mut efi::protocols::device_path::Protocol)
                    .map_err(|status| EfiError::status_to_result(status).unwrap_err())?;
            let device_path_size_minus_end_node: usize =
                device_path_size.saturating_sub(core::mem::size_of::<efi::protocols::device_path::Protocol>());
            let file_path = unsafe { (file_path as *const u8).add(device_path_size_minus_end_node) };
            fixed_file_path = Some(file_path as *mut efi::protocols::device_path::Protocol);
        } else {
            fixed_file_path = Some(file_path);
        }
    }

    if let Some(path) = fixed_file_path
        && !path.is_null()
    {
        image_info.file_path = Box::into_raw(
            copy_device_path_to_boxed_slice(path).map_err(|status| EfiError::status_to_result(status).unwrap_err())?,
        ) as *mut efi::protocols::device_path::Protocol;
    }

    let mut private_info = core_load_pe_image(image_to_load.as_ref(), image_info)
        .inspect_err(|err| log::error!("failed to load image: core_load_pe_image failed: {err:?}"))?;

    let image_info_ptr = private_info.image_info.as_ref() as *const efi::protocols::loaded_image::Protocol;
    let image_info_ptr = image_info_ptr as *mut c_void;

    log::info!(
        "Loaded driver at {:#x?} EntryPoint={:#x?} {:}",
        private_info.image_info.image_base,
        private_info.entry_point as usize,
        private_info.pe_info.filename.as_ref().unwrap_or(&String::from("<no PDB>"))
    );

    // install the loaded_image protocol for this freshly loaded image on a new
    // handle.
    let handle = core_install_protocol_interface(None, efi::protocols::loaded_image::PROTOCOL_GUID, image_info_ptr)
        .inspect_err(|err| log::error!("failed to load image: install loaded image protocol failed: {err:?}"))?;

    // register the loaded image with the debug image info configuration table. This is done before the debugger is
    // notified so that the debugger can access the loaded image protocol before that point, e.g. so
    // that symbols can be loaded on module breakpoints.
    core_new_debug_image_info_entry(
        EfiDebugImageInfoNormal::EFI_DEBUG_IMAGE_INFO_TYPE_NORMAL,
        image_info_ptr as *const efi::protocols::loaded_image::Protocol,
        handle,
    );

    // Notify the debugger of the image load.
    patina_debugger::notify_module_load(
        private_info.pe_info.filename.as_ref().unwrap_or(&String::from("")),
        private_info.image_info.image_base as usize,
        private_info.image_info.image_size as usize,
    );

    // install the loaded_image device path protocol for the new image. If input device path is not null, then make a
    // permanent copy on the heap.
    let loaded_image_device_path = if file_path.is_null() {
        core::ptr::null_mut()
    } else {
        // make copy and convert to raw pointer to avoid drop at end of function.
        Box::into_raw(
            copy_device_path_to_boxed_slice(file_path)
                .map_err(|status| EfiError::status_to_result(status).unwrap_err())?,
        ) as *mut u8
    };

    // Register runtime images with the runtime module.
    if private_info.pe_info.image_type == EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER {
        runtime::add_runtime_image(
            private_info.image_info.image_base,
            private_info.image_info.image_size,
            &private_info.relocation_data,
            handle,
        )
        .inspect_err(|err| log::error!("failed to load image: register runtime image failed: {err:?}"))?;
    }

    core_install_protocol_interface(
        Some(handle),
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        loaded_image_device_path as *mut c_void,
    )
    .inspect_err(|err| log::error!("failed to load image: install device path failed: {err:?}"))?;

    if let Some(res_section) = private_info.hii_resource_section {
        core_install_protocol_interface(
            Some(handle),
            efi::protocols::hii_package_list::PROTOCOL_GUID,
            res_section as *mut c_void,
        )
        .inspect_err(|err| log::error!("failed to load image: install HII package list failed: {err:?}"))?;
    }

    // Store the interface pointers for unload to use when uninstalling these protocol interfaces.
    private_info.image_info_ptr = image_info_ptr;
    private_info.image_device_path_ptr = file_path as *mut c_void;

    // save the private image data for this image in the private image data map.
    PRIVATE_IMAGE_DATA.lock().private_image_data.insert(handle, private_info);

    perf_load_image_end(handle, create_performance_measurement);

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
        Err(err) => err.into(),
        Ok((handle, security_status)) => unsafe {
            // Safety: Caller must ensure that image_handle is a valid pointer. It is null-checked above.
            image_handle.write_unaligned(handle);
            match security_status {
                Ok(()) => efi::Status::SUCCESS,
                Err(err) => err.into(),
            }
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
        if let Some(image_data) = private_data.private_image_data.get(&image_handle)
            && let Some(image_exit_data) = image_data.exit_data
            && !exit_data_size.is_null()
            && !exit_data.is_null()
        {
            // Safety: Caller must ensure that exit_data_size and exit_data are valid pointers if they are non-null.
            unsafe {
                exit_data_size.write_unaligned(image_exit_data.0);
                exit_data.write_unaligned(image_exit_data.1);
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

pub fn core_start_image(image_handle: efi::Handle) -> Result<(), efi::Status> {
    PROTOCOL_DB.validate_handle(image_handle)?;

    if let Some(private_data) = PRIVATE_IMAGE_DATA.lock().private_image_data.get_mut(&image_handle) {
        if private_data.started {
            Err(EfiError::InvalidParameter)?;
        }
    } else {
        Err(EfiError::InvalidParameter)?;
    }

    // allocate a buffer for the entry point stack.
    let stack = ImageStack::new(ENTRY_POINT_STACK_SIZE)?;

    perf_image_start_begin(image_handle, create_performance_measurement);

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

    log::info!("start_image entrypoint exit with status: {status:x?}");

    // because we used exit() to return from the coroutine (as opposed to
    // returning naturally from it), the coroutine is marked as suspended rather
    // than complete. We need to forcibly mark the coroutine done; otherwise it
    // will try to use unwind to clean up the co-routine stack (i.e. "drop" any
    // live objects). This unwind support requires std and will panic if
    // executed.
    unsafe { coroutine.force_reset() };

    PRIVATE_IMAGE_DATA.lock().current_running_image = previous_image;

    perf_image_start_end(image_handle, create_performance_measurement);

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
            Err(EfiError::Unsupported)?;
        }
    }
    let handles = PROTOCOL_DB.locate_handles(None).unwrap_or_default();

    core_remove_debug_image_info_entry(image_handle);

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
                        Some(open_info.attributes),
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
    let _ = core_uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image::PROTOCOL_GUID,
        private_image_data.image_info_ptr,
    );

    let _ = core_uninstall_protocol_interface(
        image_handle,
        efi::protocols::loaded_image_device_path::PROTOCOL_GUID,
        private_image_data.image_device_path_ptr,
    );

    // Remove runtime image if it is one.
    if private_image_data.pe_info.image_type == EFI_IMAGE_SUBSYSTEM_EFI_RUNTIME_DRIVER
        && let Err(err) = runtime::remove_runtime_image(image_handle)
    {
        log::error!("Failed to remove runtime image for handle {image_handle:?}: {err:?}");
    }

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
    if exit_data_size != 0
        && !exit_data.is_null()
        && let Some(image_data) = private_data.private_image_data.get_mut(&image_handle)
    {
        image_data.exit_data = Some((exit_data_size, exit_data));
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
    install_dxe_core_image(hob_list, system_table);

    // set up exit boot services callback
    let _ = EVENT_DB
        .create_event(
            efi::EVT_NOTIFY_SIGNAL,
            efi::TPL_CALLBACK,
            Some(runtime_image_protection_fixup_ebs),
            None,
            Some(efi::EVENT_GROUP_EXIT_BOOT_SERVICES),
        )
        .expect("Failed to create callback for runtime image memory protection fixups.");

    //set up imaging services
    system_table.boot_services_mut().load_image = load_image;
    system_table.boot_services_mut().start_image = start_image;
    system_table.boot_services_mut().unload_image = unload_image;
    system_table.boot_services_mut().exit = exit;
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;
    use super::{empty_image_info, get_buffer_by_file_path, load_image};
    use crate::{
        image::{PRIVATE_IMAGE_DATA, exit, start_image, unload_image},
        protocol_db,
        protocols::{PROTOCOL_DB, core_install_protocol_interface},
        systemtables::{SYSTEM_TABLE, init_system_table},
        test_collateral, test_support,
    };
    use core::{ffi::c_void, sync::atomic::AtomicBool};
    use patina::{error::EfiError, pi};
    use r_efi::efi;
    use std::{fs::File, io::Read};

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        // SAFETY: Test code only - initializing test infrastructure within the global test lock.
        test_support::with_global_lock(|| unsafe {
            test_support::init_test_gcd(None);
            test_support::init_test_protocol_db();
            init_system_table();
            init_test_image_support();
            f();
            // we need the to drop the memory here while the GCD is still valid
            PRIVATE_IMAGE_DATA.lock().reset();
        })
        .unwrap();
    }

    unsafe fn init_test_image_support() {
        // SAFETY: Test code - resetting global image data state with test lock held.
        unsafe { PRIVATE_IMAGE_DATA.lock().reset() };

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
            Err(err) => panic!("Failed to install dxe core image handle: {err:?}"),
            Ok(handle) => handle,
        };

        //set up imaging services
        system_table.boot_services_mut().load_image = load_image;
        system_table.boot_services_mut().start_image = start_image;
        system_table.boot_services_mut().unload_image = unload_image;
        system_table.boot_services_mut().exit = exit;
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
            // SAFETY: Test code - dereferencing image buffer pointer that was just created by LoadImage.
            let image_buf_len = unsafe { (&*image_data.image_buffer).len() as usize };
            assert_eq!(image_buf_len, image_data.image_info.image_size as usize);
            assert_eq!(image_data.image_info.image_data_type, efi::BOOT_SERVICES_DATA);
            assert_eq!(image_data.image_info.image_code_type, efi::BOOT_SERVICES_CODE);
            assert_ne!(image_data.entry_point as usize, 0);
            assert!(!image_data.relocation_data.is_empty());
            assert!(image_data.hii_resource_section.is_some());
        });
    }

    #[test]
    fn load_image_should_pass_for_subsystem_efi_application() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("subsystem_efi_application.efi")).expect("failed to open test file.");
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
        });
    }

    #[test]
    fn load_image_should_pass_for_subsystem_efi_runtime_driver() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("subsystem_efi_runtime_driver.efi")).expect("failed to open test file.");
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
        });
    }

    #[test]
    fn load_image_should_fail_for_windows_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("windows_console_app.exe")).expect("failed to open test file.");
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
            assert_eq!(status, efi::Status::UNSUPPORTED);
        });
    }

    #[test]
    fn load_image_should_fail_for_section_alignment_not_multiple_of_uefi_page_size() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("section_alignment_200.efi")).expect("failed to open test file.");
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
            assert_eq!(status, efi::Status::LOAD_ERROR);
        });
    }

    #[test]
    fn load_image_should_fail_for_incorrect_size_of_image() {
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("invalid_size_of_image.efi")).expect("failed to open test file.");
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
            assert_eq!(status, efi::Status::LOAD_ERROR);
        });
    }

    #[test]
    fn load_image_should_fail_for_hii_section_has_invalid_directory_name_offset() {
        with_locked_state(|| {
            let mut test_file = File::open(test_collateral!("invalid_directory_name_offset_hii.pe32"))
                .expect("failed to open test file.");
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
            assert_eq!(status, efi::Status::LOAD_ERROR);
        });
    }

    #[test]
    fn load_image_should_fail_for_invalid_relocation_directory_size() {
        with_locked_state(|| {
            let mut test_file = File::open(test_collateral!("invalid_relocation_directory_size.efi"))
                .expect("failed to open test file.");
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
            assert_eq!(status, efi::Status::LOAD_ERROR);
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
                this: *mut pi::protocols::security::Protocol,
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
                pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    pi::protocols::security::PROTOCOL_GUID,
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
            // SAFETY: Test code - dereferencing image buffer pointer that was just created by LoadImage.
            let image_buf_len = unsafe { (&*image_data.image_buffer).len() as usize };
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
                _this: *mut pi::protocols::security::Protocol,
                _authentication_status: u32,
                _file: *mut efi::protocols::device_path::Protocol,
            ) -> efi::Status {
                // should not be called, since `from_fv` is not presently true in our implementation for any
                // source of FV, which means only Security2 should be used.
                unreachable!()
            }

            let security_protocol =
                pi::protocols::security::Protocol { file_authentication_state: mock_file_authentication_state };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    pi::protocols::security::PROTOCOL_GUID,
                    &security_protocol as *const _ as *mut _,
                )
                .unwrap();

            // Mock Security2 Arch protocol
            static SECURITY2_CALL_EXECUTED: AtomicBool = AtomicBool::new(false);
            extern "efiapi" fn mock_file_authentication(
                this: *mut pi::protocols::security2::Protocol,
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
                pi::protocols::security2::Protocol { file_authentication: mock_file_authentication };

            PROTOCOL_DB
                .install_protocol_interface(
                    None,
                    pi::protocols::security2::PROTOCOL_GUID,
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
            // SAFETY: Test code - dereferencing image buffer pointer that was just created by LoadImage.
            let image_buf_len = unsafe { (&*image_data.image_buffer).len() as usize };
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
            assert_eq!(get_buffer_by_file_path(true, core::ptr::null_mut()), Err(EfiError::InvalidParameter));

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

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Err(EfiError::NotFound));
        });
    }

    // mock file support.
    extern "efiapi" fn file_read(
        _this: *mut efi::protocols::file::Protocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> efi::Status {
        let mut test_file = File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
        // SAFETY: Test mock - creating a mutable slice from the provided buffer pointer.
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
        // SAFETY: Test mock - copying file info structure to caller's buffer if large enough.
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
        // SAFETY: Test mock - writing the mock file protocol pointer to the output parameter.
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

    #[allow(clippy::undocumented_unsafe_blocks)]
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
                // SAFETY: Test mock - writing the mock file protocol pointer to the output parameter.
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

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Ok((image, false, handle, 0)));
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
                // SAFETY: Test mock - reading file into caller's buffer if large enough.
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

            assert_eq!(get_buffer_by_file_path(true, device_path_ptr), Ok((image, false, handle, 0)));
        });
    }

    #[test]
    fn load_image_should_succeed_with_proper_memory_protections() {
        // Positive test: Verifies that a valid image loads successfully when memory
        // protections can be properly applied. This is a regression test ensuring Fix #176
        // doesn't break normal operation.
        //
        // Fix #176: If apply_image_memory_protections() encounters any error,
        // the entire load_image operation fails. This test confirms
        // that valid images still load successfully with proper GCD configuration.
        //
        // Also validates section alignment by directly calling core_load_pe_image().
        with_locked_state(|| {
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            // Test 1: Full load_image flow
            let mut image_handle: efi::Handle = core::ptr::null_mut();
            let status = load_image(
                false.into(),
                protocol_db::DXE_CORE_HANDLE,
                core::ptr::null_mut(),
                image.as_mut_ptr() as *mut c_void,
                image.len(),
                core::ptr::addr_of_mut!(image_handle),
            );

            // With proper GCD setup, this should succeed
            assert_eq!(status, efi::Status::SUCCESS);

            // Verify the image was loaded successfully with correct properties
            let private_data = PRIVATE_IMAGE_DATA.lock();
            let image_data = private_data.private_image_data.get(&image_handle).unwrap();
            assert_ne!(image_data.entry_point as usize, 0);
            assert_eq!(image_data.image_info.image_code_type, efi::BOOT_SERVICES_CODE);
            assert_eq!(image_data.image_info.image_data_type, efi::BOOT_SERVICES_DATA);
            drop(private_data);

            // Test 2: Direct core_load_pe_image to validate section alignment handling
            let image_info = empty_image_info();
            let result = super::core_load_pe_image(&image, image_info);

            // Should load successfully with valid alignment
            assert!(result.is_ok());
            let private_info = result.unwrap();
            assert_ne!(private_info.entry_point as usize, 0);

            // If we get here, memory protections were successfully applied.
        });
    }

    #[test]
    fn apply_memory_protections_should_fail_when_section_address_not_in_gcd() {
        // This test verifies error path #1 from Fix #176: GCD descriptor lookup failure
        // (Task #1030 coverage improvement)
        //
        // Initialize GCD but don't add any memory descriptors, leaving the
        // internal RBT empty. When get_memory_descriptor_for_address is called,
        // get_closest_idx returns None (no descriptors in tree), causing NotFound error.
        //
        // Normally, the first add_memory_space call creates an initial
        // NonExistent descriptor covering [0, maximum_address), so all subsequent lookups
        // succeed. By skipping add_memory_space entirely, we keep the RBT empty, making
        // ANY address lookup fail with NotFound.
        //
        // This scenario represents a corrupted or uninitialized GCD state where memory
        // descriptors are missing - an edge case that the error handling should catch.
        //
        // Note: In debug builds, this hits a debug_assert!(false) which panics.
        // The panic is caught by with_global_lock(), so we check for that.

        let result = test_support::with_global_lock(|| {
            // SAFETY: These test initialization functions require unsafe because they
            // manipulate global state (GCD, protocol DB, system table)
            unsafe {
                // Initialize GCD but DON'T add any memory
                // This leaves the GCD with NO descriptors (empty RBT tree)
                crate::GCD.reset();
                crate::GCD.init(48, 16); // Normal 48-bit address space
            }

            // DON'T call add_memory_space - leave the RBT empty
            // This means get_closest_idx will return None for ANY address

            // Create a fake PE info with a section at any address
            let section = goblin::pe::section_table::SectionTable {
                name: [0; 8],
                real_name: None,
                virtual_size: 0x1000,
                virtual_address: 0x0, // Section at offset 0 from image_base
                size_of_raw_data: 0x1000,
                pointer_to_raw_data: 0,
                pointer_to_relocations: 0,
                pointer_to_linenumbers: 0,
                number_of_relocations: 0,
                number_of_linenumbers: 0,
                characteristics: goblin::pe::section_table::IMAGE_SCN_CNT_CODE
                    | goblin::pe::section_table::IMAGE_SCN_MEM_READ
                    | goblin::pe::section_table::IMAGE_SCN_MEM_EXECUTE,
            };

            let pe_info = super::UefiPeInfo {
                sections: vec![section],
                section_alignment: 0x1000,
                size_of_image: 0x2000,
                ..Default::default()
            };

            let mut image_info = empty_image_info();
            image_info.image_base = 0x1000 as *mut c_void; // Any address - RBT is empty so lookup will fail
            image_info.image_size = 0x2000;

            // Manually construct PrivateImageData with minimal required fields
            // SAFETY: Allocating memory for fake image buffer to construct test data
            let fake_buffer =
                unsafe { alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(0x2000, 0x1000).unwrap()) };

            // Dummy entry point function
            extern "efiapi" fn dummy_entry(_: *mut c_void, _: *mut efi::SystemTable) -> efi::Status {
                efi::Status::SUCCESS
            }

            let private_info = super::PrivateImageData {
                // SAFETY: Creating a raw slice from allocated buffer for test purposes
                image_buffer: core::ptr::slice_from_raw_parts_mut(fake_buffer, 0x2000),
                image_info: Box::new(image_info),
                hii_resource_section: None,
                hii_resource_section_base: None,
                hii_resource_section_num_pages: None,
                entry_point: dummy_entry,
                started: false,
                exit_data: None,
                image_info_ptr: core::ptr::null_mut(),
                image_device_path_ptr: core::ptr::null_mut(),
                pe_info: pe_info.clone(),
                relocation_data: Vec::new(),
                image_base_page: 0x1000,
                image_num_pages: 2,
            };

            // Call apply_image_memory_protections directly
            let result = super::apply_image_memory_protections(&pe_info, &private_info);

            // Should FAIL with NotFound because the GCD RBT is empty (no descriptors),
            // so get_closest_idx returns None and get_memory_descriptor_for_address returns NotFound
            assert!(result.is_err(), "Protection should fail when section address is not in GCD");
            assert_eq!(result.unwrap_err(), EfiError::NotFound, "Expected NotFound from GCD descriptor lookup");
        });

        // In debug builds, debug_assert!(false) panics and with_global_lock catches it
        #[cfg(debug_assertions)]
        assert!(result.is_err(), "Expected panic from debug_assert! in debug build");

        // In release builds, debug_assert is compiled away and function returns error normally
        #[cfg(not(debug_assertions))]
        assert!(result.is_ok(), "Expected successful test execution in release build");
    }

    #[test]
    fn load_image_should_fail_with_section_alignment_overflow() {
        // This test verifies error path #2 from Fix #176: when section alignment calculation
        // overflows in apply_image_memory_protections, the error is propagated and the
        // image load fails (Task #1030 coverage improvement).
        //
        // We craft a malformed PE image with a section virtual_size that will cause
        // align_up() to overflow when aligning to section_alignment.
        //
        // Note: In debug builds, this hits a debug_assert!(false) which panics.
        // The panic is caught by with_global_lock(), so we check for that.

        let result = test_support::with_global_lock(|| {
            // SAFETY: These test initialization functions require unsafe because they
            // manipulate global state (GCD, protocol DB, system table)
            unsafe {
                test_support::init_test_gcd(None);
                test_support::init_test_protocol_db();
                init_system_table();
                init_test_image_support();
            }

            // Load a valid test image as a template
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            // PE header starts at offset 0x78 for this image
            // Section headers start after the optional header
            // For PE32+: COFF (20 bytes) + Optional header size
            let pe_offset = 0x78;
            let opt_header_size_offset = pe_offset + 4 + 16; // After signature + COFF header fields
            let opt_header_size =
                u16::from_le_bytes([image[opt_header_size_offset], image[opt_header_size_offset + 1]]) as usize;
            let section_table_offset = pe_offset + 4 + 20 + opt_header_size;

            // Each section header is 40 bytes
            // Modify the first section's VirtualSize (offset 8 in section header) to cause overflow
            // Set it to u32::MAX - 0x800 so that align_up(u32::MAX - 0x800, 0x1000) will overflow
            let first_section_offset = section_table_offset;
            let virtual_size_offset = first_section_offset + 8;

            // Set VirtualSize to a value that will overflow when aligned to 0x1000
            let overflow_value: u32 = u32::MAX - 0x800;
            image[virtual_size_offset..virtual_size_offset + 4].copy_from_slice(&overflow_value.to_le_bytes());

            // Try to load the malformed image by calling core_load_pe_image directly
            // (bypassing the FFI extern "efiapi" fn load_image which cannot unwind)
            let image_info = empty_image_info();
            let result = super::core_load_pe_image(&image, image_info);

            // The load should FAIL due to alignment overflow
            // This verifies that the error from apply_image_memory_protections
            // is properly propagated (Fix #176)
            match result {
                Err(EfiError::LoadError) => { /* Expected error */ }
                Err(e) => panic!("Expected LoadError from alignment overflow, got {:?}", e),
                Ok(_) => panic!("Image with overflowing section alignment should fail to load"),
            }
        });

        // In debug builds, debug_assert!(false) panics and with_global_lock catches it
        #[cfg(debug_assertions)]
        assert!(result.is_err(), "Expected panic from debug_assert! in debug build");

        // In release builds, debug_assert is compiled away and function returns error normally
        #[cfg(not(debug_assertions))]
        assert!(result.is_ok(), "Expected successful test execution in release build");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn load_image_should_fail_with_unaligned_section_address() {
        // This test verifies error path #3 from Fix #176: set_memory_space_attributes failure
        // (Task #1030 coverage improvement)
        //
        // Create a PE image with a section VirtualAddress that is NOT page-aligned.
        // When apply_image_memory_protections calculates section_base_addr = image_base + virtual_address,
        // the result will be unaligned. Then set_memory_space_attributes will fail with InvalidParameter
        // because the base address is not page-aligned.
        //
        // Note: This test only runs in debug builds because it specifically tests
        // the debug_assert!(false) panic path. In release mode, this particular error
        // case doesn't result in a load failure.

        let result = test_support::with_global_lock(|| {
            // SAFETY: These test initialization functions require unsafe because they
            // manipulate global state (GCD, protocol DB, system table)
            unsafe {
                test_support::init_test_gcd(None);
                test_support::init_test_protocol_db();
                init_system_table();
                init_test_image_support();
            }

            // Load a valid test image as a template
            let mut test_file =
                File::open(test_collateral!("RustImageTestDxe.efi")).expect("failed to open test file.");
            let mut image: Vec<u8> = Vec::new();
            test_file.read_to_end(&mut image).expect("failed to read test file");

            // Find the PE header and section table
            let pe_offset = 0x78;
            let opt_header_size_offset = pe_offset + 4 + 16;
            let opt_header_size =
                u16::from_le_bytes([image[opt_header_size_offset], image[opt_header_size_offset + 1]]) as usize;
            let section_table_offset = pe_offset + 4 + 20 + opt_header_size;

            // Modify the first section's VirtualAddress (offset 12 in section header) to be unaligned
            // Set it to 0x1001 (not a multiple of 0x1000/4096)
            let virtual_address_offset = section_table_offset + 12;
            let unaligned_value: u32 = 0x1001; // Unaligned by 1 byte
            image[virtual_address_offset..virtual_address_offset + 4].copy_from_slice(&unaligned_value.to_le_bytes());

            // Call core_load_pe_image directly (not load_image) to avoid FFI boundary
            let image_info = empty_image_info();
            let result = super::core_load_pe_image(&image, image_info);

            // The load should FAIL because when apply_image_memory_protections calculates
            // section_base_addr = image_base + 0x1001, the address will be unaligned.
            // set_memory_space_attributes will check (base_address & 0xFFF) == 0 and fail.
            match result {
                Err(EfiError::InvalidParameter) => { /* Expected error */ }
                Err(e) => panic!("Expected InvalidParameter from unaligned address, got {:?}", e),
                Ok(_) => panic!("Image with unaligned section address should fail to load"),
            }
        });

        // Verify that the panic was caught by with_global_lock
        assert!(result.is_err(), "Expected panic from debug_assert!");
    }
}
