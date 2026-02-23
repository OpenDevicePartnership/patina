//! DXE Core System Table Support
//!
//! Routines for creating and manipulating EFI System tables.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{ffi::c_void, mem::size_of, slice::from_raw_parts};

use alloc::boxed::Box;
use patina::{boot_services::BootServices, component::component, pi::error_codes::EFI_NOT_AVAILABLE_YET};
use r_efi::efi;

use crate::{allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR, tpl_mutex};

pub static SYSTEM_TABLE: tpl_mutex::TplMutex<Option<EfiSystemTable>> =
    tpl_mutex::TplMutex::new(efi::TPL_NOTIFY, None, "StLock");

pub struct EfiRuntimeServicesTable {
    runtime_services: *mut efi::RuntimeServices,
}

impl EfiRuntimeServicesTable {
    /// Allocates a new Runtime Services table initialized to default stub functions in the Runtime Services Data allocator.
    pub fn allocate_new_table() -> Self {
        let rt = Self::default_runtime_services_table();
        let (runtime_services, _alloc) =
            Box::into_raw_with_allocator(Box::new_in(rt, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR));
        let mut table = Self { runtime_services };
        table.checksum();
        table
    }

    /// Creates a new Runtime Services Table instance from the given raw pointer.
    /// # Safety
    /// The pointer must be valid and point to a properly initialized efi::RuntimeServices structure.
    pub unsafe fn from_raw_pointer(ptr: *mut efi::RuntimeServices) -> Self {
        Self { runtime_services: ptr }
    }

    // Checksums the Runtime Services table.
    fn checksum(&mut self) {
        // SAFETY: structure construction ensures pointer is valid.
        let mut table_copy = unsafe { self.runtime_services.read() };
        table_copy.hdr.crc32 = 0;

        let tbl_slice =
            unsafe { from_raw_parts(&table_copy as *const _ as *const u8, size_of::<efi::RuntimeServices>()) };
        table_copy.hdr.crc32 = crc32fast::hash(tbl_slice);

        // SAFETY: structure construction ensures pointer is valid.
        unsafe { self.runtime_services.write(table_copy) }
    }

    // Used in tests and included for API completness; may be used in future.
    #[allow(dead_code)]
    /// Returns a copy of the Runtime Services table.
    pub fn get(&self) -> efi::RuntimeServices {
        // SAFETY: structure construction ensures pointer is valid.
        // To be truly paranoid here the TPL should be raised to TPL_HIGH to kill interrupts to prevent any possibility
        // of external concurrent modification while the table is read out. The current anticipated usage patterns
        // for boot services (externally) don't require this level of paranoia, but noting this for future reference.
        unsafe { self.runtime_services.read() }
    }

    // Used in tests and included for API completness; may be used in future.
    #[allow(dead_code)]
    /// Writes the given Runtime Services table into the stored pointer and updates the checksum.
    pub fn set(&mut self, new_table: efi::RuntimeServices) {
        // SAFETY: structure construction ensures pointer is valid.
        // To be truly paranoid here the TPL should be raised to TPL_HIGH to kill interrupts to prevent any possibility
        // of external concurrent modification while the table is written. The current anticipated usage patterns
        // for runtime services (externally) don't require this level of paranoia, but noting this for future reference.
        unsafe {
            self.runtime_services.write(new_table);
        }
        self.checksum();
    }

    /// Returns the raw pointer to the Runtime Services table.
    pub fn as_mut_ptr(&self) -> *mut efi::RuntimeServices {
        self.runtime_services
    }

    // Create a default table populated with stub functions that return `EFI_NOT_AVAILABLE_YET` and which is not
    // checksummed.
    fn default_runtime_services_table() -> efi::RuntimeServices {
        //private unimplemented stub functions used to initialize the table.
        #[coverage(off)]
        extern "efiapi" fn get_time_unimplemented(_: *mut efi::Time, _: *mut efi::TimeCapabilities) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_time_unimplemented(_: *mut efi::Time) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_wakeup_time_unimplemented(
            _: *mut efi::Boolean,
            _: *mut efi::Boolean,
            _: *mut efi::Time,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_wakeup_time_unimplemented(_: efi::Boolean, _: *mut efi::Time) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_virtual_address_map_unimplemented(
            _: usize,
            _: usize,
            _: u32,
            _: *mut efi::MemoryDescriptor,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn convert_pointer_unimplemented(_: usize, _: *mut *mut c_void) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_variable_unimplemented(
            _: *mut efi::Char16,
            _: *mut efi::Guid,
            _: *mut u32,
            _: *mut usize,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_next_variable_name_unimplemented(
            _: *mut usize,
            _: *mut efi::Char16,
            _: *mut efi::Guid,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_variable_unimplemented(
            _: *mut efi::Char16,
            _: *mut efi::Guid,
            _: u32,
            _: usize,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_next_high_mono_count_unimplemented(_: *mut u32) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn reset_system_unimplemented(_: efi::ResetType, _: efi::Status, _: usize, _: *mut c_void) {}

        #[coverage(off)]
        extern "efiapi" fn update_capsule_unimplemented(
            _: *mut *mut efi::CapsuleHeader,
            _: usize,
            _: efi::PhysicalAddress,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn query_capsule_capabilities_unimplemented(
            _: *mut *mut efi::CapsuleHeader,
            _: usize,
            _: *mut u64,
            _: *mut efi::ResetType,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn query_variable_info_unimplemented(
            _: u32,
            _: *mut u64,
            _: *mut u64,
            _: *mut u64,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }
        efi::RuntimeServices {
            hdr: efi::TableHeader {
                signature: efi::RUNTIME_SERVICES_SIGNATURE,
                revision: efi::RUNTIME_SERVICES_REVISION,
                header_size: size_of::<efi::RuntimeServices>() as u32,
                crc32: 0,
                reserved: 0,
            },
            get_time: get_time_unimplemented,
            set_time: set_time_unimplemented,
            get_wakeup_time: get_wakeup_time_unimplemented,
            set_wakeup_time: set_wakeup_time_unimplemented,
            set_virtual_address_map: set_virtual_address_map_unimplemented,
            convert_pointer: convert_pointer_unimplemented,
            get_variable: get_variable_unimplemented,
            get_next_variable_name: get_next_variable_name_unimplemented,
            set_variable: set_variable_unimplemented,
            get_next_high_mono_count: get_next_high_mono_count_unimplemented,
            reset_system: reset_system_unimplemented,
            update_capsule: update_capsule_unimplemented,
            query_capsule_capabilities: query_capsule_capabilities_unimplemented,
            query_variable_info: query_variable_info_unimplemented,
        }
    }
}

pub struct EfiBootServicesTable {
    boot_services: *mut efi::BootServices,
}

impl EfiBootServicesTable {
    /// Allocates a new Boot Services table initialized to default stub functions in the Boot Services Data allocator.
    pub fn allocate_new_table() -> Self {
        let bs = Self::default_boot_services_table();
        let boot_services = Box::into_raw(Box::new(bs));
        let mut table = Self { boot_services };
        table.checksum();
        table
    }

    /// Creates a new Boot Services Table instance from the given raw pointer.
    /// # Safety
    /// The pointer must be valid and point to a properly initialized efi::BootServices structure.
    pub unsafe fn from_raw_pointer(ptr: *mut efi::BootServices) -> Self {
        Self { boot_services: ptr }
    }

    fn checksum(&mut self) {
        // SAFETY: structure construction ensures pointer is valid.
        let mut table_copy = unsafe { self.boot_services.read() };
        table_copy.hdr.crc32 = 0;

        let tbl_slice = unsafe { from_raw_parts(&table_copy as *const _ as *const u8, size_of::<efi::BootServices>()) };
        table_copy.hdr.crc32 = crc32fast::hash(tbl_slice);

        // SAFETY: structure construction ensures pointer is valid.
        unsafe { self.boot_services.write(table_copy) }
    }

    /// Returns a copy of the Boot Services table
    pub fn get(&self) -> efi::BootServices {
        // SAFETY: structure construction ensures pointer is valid.
        // To be truly paranoid here the TPL should be raised to TPL_HIGH to kill interrupts to prevent any possibility
        // of external concurrent modification while the table is read out. The current anticipated usage patterns
        // for boot services (externally) don't require this level of paranoia, but noting this for future reference.
        unsafe { self.boot_services.read() }
    }

    /// Sets the Boot Services table to a new value and updates the checksum.
    pub fn set(&mut self, new_table: efi::BootServices) {
        // SAFETY: structure construction ensures pointer is valid.
        // To be truly paranoid here the TPL should be raised to TPL_HIGH to kill interrupts to prevent any possibility
        // of external concurrent modification while the table is written. The current anticipated usage patterns
        // for runtime services (externally) don't require this level of paranoia, but noting this for future reference.
        unsafe {
            self.boot_services.write(new_table);
        }
        self.checksum();
    }

    /// Returns the raw pointer to the Boot Services table.
    pub fn as_mut_ptr(&self) -> *mut efi::BootServices {
        self.boot_services
    }

    // Create a default table populated with stub functions that return `EFI_NOT_AVAILABLE_YET` and which is not
    // checksummed.
    fn default_boot_services_table() -> efi::BootServices {
        //private unimplemented stub functions used to initialize the table.
        #[coverage(off)]
        extern "efiapi" fn raise_tpl_unimplemented(_: efi::Tpl) -> efi::Tpl {
            efi::TPL_APPLICATION
        }

        #[coverage(off)]
        extern "efiapi" fn restore_tpl_unimplemented(_: efi::Tpl) {}

        #[coverage(off)]
        extern "efiapi" fn allocate_pages_unimplemented(
            _: efi::AllocateType,
            _: efi::MemoryType,
            _: usize,
            _: *mut efi::PhysicalAddress,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn free_pages_unimplemented(_: efi::PhysicalAddress, _: usize) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_memory_map_unimplemented(
            _: *mut usize,
            _: *mut efi::MemoryDescriptor,
            _: *mut usize,
            _: *mut usize,
            _: *mut u32,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn allocate_pool_unimplemented(
            _: efi::MemoryType,
            _: usize,
            _: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn free_pool_unimplemented(_: *mut c_void) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn create_event_unimplemented(
            _: u32,
            _: efi::Tpl,
            _: Option<efi::EventNotify>,
            _: *mut c_void,
            _: *mut efi::Event,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_timer_unimplemented(_: efi::Event, _: efi::TimerDelay, _: u64) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn wait_for_event_unimplemented(_: usize, _: *mut efi::Event, _: *mut usize) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn signal_event_unimplemented(_: efi::Event) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn close_event_unimplemented(_: efi::Event) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn check_event_unimplemented(_: efi::Event) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn install_protocol_interface_unimplemented(
            _: *mut efi::Handle,
            _: *mut efi::Guid,
            _: efi::InterfaceType,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn reinstall_protocol_interface_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: *mut c_void,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn uninstall_protocol_interface_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn handle_protocol_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn register_protocol_notify_unimplemented(
            _: *mut efi::Guid,
            _: efi::Event,
            _: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn locate_handle_unimplemented(
            _: efi::LocateSearchType,
            _: *mut efi::Guid,
            _: *mut c_void,
            _: *mut usize,
            _: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn locate_device_path_unimplemented(
            _: *mut efi::Guid,
            _: *mut *mut efi::protocols::device_path::Protocol,
            _: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn install_configuration_table_unimplemented(_: *mut efi::Guid, _: *mut c_void) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn load_image_unimplemented(
            _: efi::Boolean,
            _: efi::Handle,
            _: *mut efi::protocols::device_path::Protocol,
            _: *mut c_void,
            _: usize,
            _: *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn start_image_unimplemented(
            _: efi::Handle,
            _: *mut usize,
            _: *mut *mut efi::Char16,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn exit_unimplemented(
            _: efi::Handle,
            _: efi::Status,
            _: usize,
            _: *mut efi::Char16,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn unload_image_unimplemented(_: efi::Handle) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn exit_boot_services_unimplemented(_: efi::Handle, _: usize) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn get_next_monotonic_count_unimplemented(_: *mut u64) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn stall_unimplemented(_: usize) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn set_watchdog_timer_unimplemented(
            _: usize,
            _: u64,
            _: usize,
            _: *mut efi::Char16,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn connect_controller_unimplemented(
            _: efi::Handle,
            _: *mut efi::Handle,
            _: *mut efi::protocols::device_path::Protocol,
            _: efi::Boolean,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn disconnect_controller_unimplemented(
            _: efi::Handle,
            _: efi::Handle,
            _: efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn open_protocol_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: *mut *mut c_void,
            _: efi::Handle,
            _: efi::Handle,
            _: u32,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn close_protocol_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: efi::Handle,
            _: efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn open_protocol_information_unimplemented(
            _: efi::Handle,
            _: *mut efi::Guid,
            _: *mut *mut efi::OpenProtocolInformationEntry,
            _: *mut usize,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn protocols_per_handle_unimplemented(
            _: efi::Handle,
            _: *mut *mut *mut efi::Guid,
            _: *mut usize,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn locate_handle_buffer_unimplemented(
            _: efi::LocateSearchType,
            _: *mut efi::Guid,
            _: *mut c_void,
            _: *mut usize,
            _: *mut *mut efi::Handle,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn locate_protocol_unimplemented(
            _: *mut efi::Guid,
            _: *mut c_void,
            _: *mut *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn install_multiple_protocol_interfaces_unimplemented(
            _: *mut efi::Handle,
            _: *mut c_void,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn uninstall_multiple_protocol_interfaces_unimplemented(
            _: efi::Handle,
            _: *mut c_void,
            _: *mut c_void,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn calculate_crc32_unimplemented(_: *mut c_void, _: usize, _: *mut u32) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }

        #[coverage(off)]
        extern "efiapi" fn copy_mem_unimplemented(_: *mut c_void, _: *mut c_void, _: usize) {}

        #[coverage(off)]
        extern "efiapi" fn set_mem_unimplemented(_: *mut c_void, _: usize, _: u8) {}

        #[coverage(off)]
        extern "efiapi" fn create_event_ex_unimplemented(
            _: u32,
            _: efi::Tpl,
            _: Option<efi::EventNotify>,
            _: *const c_void,
            _: *const efi::Guid,
            _: *mut efi::Event,
        ) -> efi::Status {
            efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
        }
        efi::BootServices {
            hdr: efi::TableHeader {
                signature: efi::BOOT_SERVICES_SIGNATURE,
                revision: efi::BOOT_SERVICES_REVISION,
                header_size: size_of::<efi::BootServices>() as u32,
                crc32: 0,
                reserved: 0,
            },
            raise_tpl: raise_tpl_unimplemented,
            restore_tpl: restore_tpl_unimplemented,
            allocate_pages: allocate_pages_unimplemented,
            free_pages: free_pages_unimplemented,
            get_memory_map: get_memory_map_unimplemented,
            allocate_pool: allocate_pool_unimplemented,
            free_pool: free_pool_unimplemented,
            create_event: create_event_unimplemented,
            set_timer: set_timer_unimplemented,
            wait_for_event: wait_for_event_unimplemented,
            signal_event: signal_event_unimplemented,
            close_event: close_event_unimplemented,
            check_event: check_event_unimplemented,
            install_protocol_interface: install_protocol_interface_unimplemented,
            reinstall_protocol_interface: reinstall_protocol_interface_unimplemented,
            uninstall_protocol_interface: uninstall_protocol_interface_unimplemented,
            handle_protocol: handle_protocol_unimplemented,
            reserved: core::ptr::null_mut(),
            register_protocol_notify: register_protocol_notify_unimplemented,
            locate_handle: locate_handle_unimplemented,
            locate_device_path: locate_device_path_unimplemented,
            install_configuration_table: install_configuration_table_unimplemented,
            load_image: load_image_unimplemented,
            start_image: start_image_unimplemented,
            exit: exit_unimplemented,
            unload_image: unload_image_unimplemented,
            exit_boot_services: exit_boot_services_unimplemented,
            get_next_monotonic_count: get_next_monotonic_count_unimplemented,
            stall: stall_unimplemented,
            set_watchdog_timer: set_watchdog_timer_unimplemented,
            connect_controller: connect_controller_unimplemented,
            disconnect_controller: disconnect_controller_unimplemented,
            open_protocol: open_protocol_unimplemented,
            close_protocol: close_protocol_unimplemented,
            open_protocol_information: open_protocol_information_unimplemented,
            protocols_per_handle: protocols_per_handle_unimplemented,
            locate_handle_buffer: locate_handle_buffer_unimplemented,
            locate_protocol: locate_protocol_unimplemented,
            install_multiple_protocol_interfaces: install_multiple_protocol_interfaces_unimplemented,
            uninstall_multiple_protocol_interfaces: uninstall_multiple_protocol_interfaces_unimplemented,
            calculate_crc32: calculate_crc32_unimplemented,
            copy_mem: copy_mem_unimplemented,
            set_mem: set_mem_unimplemented,
            create_event_ex: create_event_ex_unimplemented,
        }
    }
}

pub struct EfiSystemTable {
    system_table: *mut efi::SystemTable,
}

// SAFETY: EfiSystemTable implementation below takes care to ensure that the underlying raw pointer is used in a
// copy/modify/write manner that strives for consistency in the face of external mutation. This implementation assumes
// that external code may modify the underlying structure outside of the Patina context. Some assumptions are made on
// the behavior of external code: in particular,  that the system table structures are modified by external drivers in
// event callbacks that might interrupt core use of the table (i.e. between a get and a set).
//
// Within Patina, access to the system table is synchronized via SYSTEM_TABLE TplMutex to prevent data races.
unsafe impl Send for EfiSystemTable {}

impl EfiSystemTable {
    /// Allocates a new EFI System Table with default contents in the Runtime Services Data allocator. Includes creation
    /// of default Runtime and Boot services tables in the Runtime Services Data allocator and Boot Services Data
    /// allocator respectively.
    pub fn allocate_new_table() -> Self {
        let mut st = Self::default_system_table();

        st.runtime_services = EfiRuntimeServicesTable::allocate_new_table().as_mut_ptr();
        st.boot_services = EfiBootServicesTable::allocate_new_table().as_mut_ptr();

        let (system_table, _alloc) =
            Box::into_raw_with_allocator(Box::new_in(st, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR));
        let mut table = Self { system_table };

        table.checksum();
        table
    }

    // Included for API completness; may be used in future.
    #[allow(dead_code)]
    /// Creates a new EFI System Table instance from the given raw pointer.
    /// # Safety
    /// The pointer must be valid and point to a properly initialized efi::SystemTable structure.
    pub unsafe fn from_raw_pointer(ptr: *mut efi::SystemTable) -> Self {
        unsafe {
            assert!(!ptr.is_null(), "Attempted to create EfiSystemTable with null System Table pointer");
            assert!(
                !(*ptr).boot_services.is_null(),
                "Attempted to create EfiSystemTable with null Boot Services pointer"
            );
            assert!(
                !(*ptr).runtime_services.is_null(),
                "Attempted to create EfiSystemTable with null Runtime Services pointer"
            );
        }
        Self { system_table: ptr }
    }

    // Checksums the System Table
    fn checksum(&mut self) {
        // SAFETY: structure construction ensures pointer is valid.
        let mut table_copy = unsafe { self.system_table.read() };
        table_copy.hdr.crc32 = 0;

        let st_slice = unsafe { from_raw_parts(&table_copy as *const _ as *const u8, size_of::<efi::SystemTable>()) };
        table_copy.hdr.crc32 = crc32fast::hash(st_slice);

        // SAFETY: structure construction ensures pointer is valid.
        unsafe { self.system_table.write(table_copy) }
    }

    /// Returns a copy of the System Table
    pub fn get(&self) -> efi::SystemTable {
        // SAFETY: structure construction ensures pointer is valid.
        unsafe { self.system_table.read() }
    }

    /// Writes the given System Table into the stored pointer and updates the checksum.
    pub fn set(&mut self, new_table: efi::SystemTable) {
        assert!(!new_table.boot_services.is_null(), "Attempted to set System Table with null Boot Services pointer");
        assert!(
            !new_table.runtime_services.is_null(),
            "Attempted to set System Table with null Runtime Services pointer"
        );
        // SAFETY: structure construction ensures pointer is valid.
        unsafe {
            self.system_table.write(new_table);
        }
        self.checksum();
    }

    /// Writes the given System Table into the stored pointer without validation and updates the checksum.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the new table is has valid pointers for runtime_services and boot_services.
    /// Boot services pointer may be null if the table is being updated for use after ExitBootServices.
    pub unsafe fn set_unchecked(&mut self, new_table: efi::SystemTable) {
        // SAFETY: caller must ensure that the new_table is valid.
        unsafe {
            self.system_table.write(new_table);
        }
        self.checksum();
    }

    /// Returns the raw pointer to the System Table.
    pub fn as_mut_ptr(&self) -> *mut efi::SystemTable {
        self.system_table
    }

    /// Returns the Runtime Services table from the system table.
    pub fn runtime_services(&self) -> EfiRuntimeServicesTable {
        // SAFETY: structure construction ensures System Table pointer is valid.
        // Self::set ensures runtime_services pointer is not null.
        unsafe {
            let st = self.system_table.read();
            assert!(!st.runtime_services.is_null(), "RuntimeServices pointer is null");
            EfiRuntimeServicesTable::from_raw_pointer(st.runtime_services)
        }
    }

    /// Returns the Boot Services table from the system table.
    pub fn boot_services(&self) -> EfiBootServicesTable {
        // SAFETY: structure construction ensures System Table pointer is valid.
        // Self::set ensures boot_services pointer is not null.
        unsafe {
            let st = self.system_table.read();
            assert!(!st.boot_services.is_null(), "BootServices pointer is null");
            EfiBootServicesTable::from_raw_pointer(st.boot_services)
        }
    }

    /// Clears the Boot Services and console pointers from the System Table.
    ///
    /// # Safety
    ///
    /// This should only be called after ExitBootServices has been invoked.
    pub unsafe fn clear_boot_time_services(&mut self) {
        let mut st = self.get();

        st.boot_services = core::ptr::null_mut();
        st.con_in = core::ptr::null_mut();
        st.console_in_handle = core::ptr::null_mut();
        st.con_out = core::ptr::null_mut();
        st.console_out_handle = core::ptr::null_mut();
        st.std_err = core::ptr::null_mut();
        st.standard_error_handle = core::ptr::null_mut();

        // SAFETY: set unchecked is used here because the table being set violates the normal invariant that the
        // boot_services pointer is non-null. The caller must ensure that this is only called after ExitBootServices
        // which is the case here.
        unsafe {
            self.set_unchecked(st);
        }
    }

    /// Updates the checksum of the System Table, Runtime Services Table, and Boot Services Table.
    pub fn checksum_all(&mut self) {
        let mut rt = self.runtime_services();
        rt.checksum();

        let mut bs = self.boot_services();
        bs.checksum();

        self.checksum();
    }

    // Create a default System Table populated with null Runtime and Boot Services pointers and which is not checksummed.
    fn default_system_table() -> efi::SystemTable {
        efi::SystemTable {
            hdr: efi::TableHeader {
                signature: efi::SYSTEM_TABLE_SIGNATURE,
                revision: efi::SYSTEM_TABLE_REVISION,
                header_size: size_of::<efi::SystemTable>() as u32,
                crc32: 0,
                reserved: 0,
            },
            firmware_vendor: core::ptr::null_mut(),
            firmware_revision: 0,
            console_in_handle: core::ptr::null_mut(),
            con_in: core::ptr::null_mut(),
            console_out_handle: core::ptr::null_mut(),
            con_out: core::ptr::null_mut(),
            standard_error_handle: core::ptr::null_mut(),
            std_err: core::ptr::null_mut(),
            runtime_services: core::ptr::null_mut(),
            boot_services: core::ptr::null_mut(),
            number_of_table_entries: 0,
            configuration_table: core::ptr::null_mut(),
        }
    }
}

pub fn init_system_table() {
    *SYSTEM_TABLE.lock() = Some(EfiSystemTable::allocate_new_table());
}

/// A component to register a callback that recalculates the CRC32 checksum of the system table
/// when certain protocols are installed.
#[derive(Default)]
pub(crate) struct SystemTableChecksumInstaller;

#[component]
impl SystemTableChecksumInstaller {
    fn entry_point(self, bs: patina::boot_services::StandardBootServices) -> patina::error::Result<()> {
        extern "efiapi" fn callback(_event: efi::Event, _: *mut c_void) {
            SYSTEM_TABLE.lock().as_mut().expect("System Table is initialized").checksum_all();
        }

        const GUIDS: [efi::Guid; 16] = [
            efi::Guid::from_bytes(&uuid::uuid!("1DA97072-BDDC-4B30-99F1-72A0B56FFF2A").to_bytes_le()), // gEfiMonotonicCounterArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("1E5668E2-8481-11D4-BCF1-0080C73C8881").to_bytes_le()), // gEfiVariableArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("26BACCB1-6F42-11D4-BC7E-0080C73C8881").to_bytes_le()), // gEfiCpuArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("26BACCB2-6F42-11D4-BCE7-0080C73C8881").to_bytes_le()), // gEfiMetronomeArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("26BACCB3-6F42-11D4-BCE7-0080C73C8881").to_bytes_le()), // gEfiTimerArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("27CFAC87-46CC-11D4-9A38-0090273FC14D").to_bytes_le()), // gEfiRealTimeClockArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("27CFAC88-46CC-11D4-9A38-0090273FC14D").to_bytes_le()), // gEfiResetArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("5053697E-2CBC-4819-90D9-0580DEEE5754").to_bytes_le()), // gEfiCapsuleArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("55198405-26c0-4765-8b7d-be1df5f99712").to_bytes_le()), // gEfiCpu2ProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("6441F818-6362-4E44-B570-7DBA31DD2453").to_bytes_le()), // gEfiVariableWriteArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("665E3FF5-46CC-11D4-9A38-0090273FC14D").to_bytes_le()), // gEfiWatchdogTimerArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("665E3FF6-46CC-11D4-9A38-0090273FC14D").to_bytes_le()), // gEfiBdsArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("94AB2F58-1438-4EF1-9152-18941894A3A0").to_bytes_le()), // gEfiSecurity2ArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("A46423E3-4617-49F1-B9FF-D1BFA9115839").to_bytes_le()), // gEfiSecurityArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("B7DFB4E1-052F-449F-87BE-9818FC91B733").to_bytes_le()), // gEfiRuntimeArchProtocolGuid
            efi::Guid::from_bytes(&uuid::uuid!("F4CCBFB7-F6E0-47FD-9DD4-10A8F150C191").to_bytes_le()), // gEfiSmmBase2ProtocolGuid
        ];

        for guid in &GUIDS {
            let event = bs.create_event(
                patina::boot_services::event::EventType::NOTIFY_SIGNAL,
                patina::boot_services::tpl::Tpl::CALLBACK,
                Some(callback),
                core::ptr::null_mut(),
            )?;

            bs.register_protocol_notify(guid, event)?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::test_support;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            test_support::init_test_logger();
            // SAFETY: Test code only - initializing the test GCD with the test lock held
            // prevents concurrent access during initialization.
            unsafe { test_support::init_test_gcd(Some(0x4000000)) };
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_checksum_changes_on_edit() {
        with_locked_state(|| {
            let mut table = EfiSystemTable::allocate_new_table();
            table.checksum();

            let system_table_crc32 = table.get().hdr.crc32;
            let boot_services_crc32 = table.boot_services().get().hdr.crc32;
            let runtime_services_crc32 = table.runtime_services().get().hdr.crc32;

            // Update a boot_services function
            extern "efiapi" fn raise_tpl(_: efi::Tpl) -> efi::Tpl {
                efi::TPL_APPLICATION
            }
            let mut bs = table.boot_services().get();
            bs.raise_tpl = raise_tpl;
            table.boot_services().set(bs);

            // Update a runtime_services function
            extern "efiapi" fn get_variable(
                _: *mut efi::Char16,
                _: *mut efi::Guid,
                _: *mut u32,
                _: *mut usize,
                _: *mut c_void,
            ) -> efi::Status {
                efi::Status::SUCCESS
            }
            let mut rt = table.runtime_services().get();
            rt.get_variable = get_variable;
            table.runtime_services().set(rt);

            // Update a system_table field
            let mut st = table.get();
            st.hdr.revision = 0x100;
            table.set(st);

            // Checksums should be different.
            let new_system_table_crc32 = table.get().hdr.crc32;
            let new_boot_services_crc32 = table.boot_services().get().hdr.crc32;
            let new_runtime_services_crc32 = table.runtime_services().get().hdr.crc32;

            assert_ne!(system_table_crc32, new_system_table_crc32);
            assert_ne!(boot_services_crc32, new_boot_services_crc32);
            assert_ne!(runtime_services_crc32, new_runtime_services_crc32);

            // Check that clearing boot time services changes the checksum
            // SAFETY: this is test code modeling exit boot services behavior.
            unsafe {
                table.clear_boot_time_services();
                assert_eq!((*table.system_table).boot_services, core::ptr::null_mut());
            };
        })
    }
}
