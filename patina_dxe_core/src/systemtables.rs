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

use alloc::{alloc::Allocator, boxed::Box};
use patina::{boot_services::BootServices, component::IntoComponent, pi::error_codes::EFI_NOT_AVAILABLE_YET};
use r_efi::efi;

use crate::{allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR, tpl_mutex};

pub static SYSTEM_TABLE: tpl_mutex::TplMutex<Option<EfiSystemTable>> =
    tpl_mutex::TplMutex::new(efi::TPL_NOTIFY, None, "StLock");

pub struct EfiRuntimeServicesTable {
    runtime_services: Box<efi::RuntimeServices, &'static dyn Allocator>,
}

impl EfiRuntimeServicesTable {
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
    extern "efiapi" fn query_variable_info_unimplemented(_: u32, _: *mut u64, _: *mut u64, _: *mut u64) -> efi::Status {
        efi::Status::from_usize(EFI_NOT_AVAILABLE_YET)
    }

    pub fn init() -> EfiRuntimeServicesTable {
        let mut rt = efi::RuntimeServices {
            hdr: efi::TableHeader {
                signature: efi::RUNTIME_SERVICES_SIGNATURE,
                revision: efi::RUNTIME_SERVICES_REVISION,
                header_size: 0,
                crc32: 0,
                reserved: 0,
            },
            get_time: Self::get_time_unimplemented,
            set_time: Self::set_time_unimplemented,
            get_wakeup_time: Self::get_wakeup_time_unimplemented,
            set_wakeup_time: Self::set_wakeup_time_unimplemented,
            set_virtual_address_map: Self::set_virtual_address_map_unimplemented,
            convert_pointer: Self::convert_pointer_unimplemented,
            get_variable: Self::get_variable_unimplemented,
            get_next_variable_name: Self::get_next_variable_name_unimplemented,
            set_variable: Self::set_variable_unimplemented,
            get_next_high_mono_count: Self::get_next_high_mono_count_unimplemented,
            reset_system: Self::reset_system_unimplemented,
            update_capsule: Self::update_capsule_unimplemented,
            query_capsule_capabilities: Self::query_capsule_capabilities_unimplemented,
            query_variable_info: Self::query_variable_info_unimplemented,
        };

        rt.hdr.header_size = size_of::<efi::RuntimeServices>() as u32;

        let mut table =
            EfiRuntimeServicesTable { runtime_services: Box::new_in(rt, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR) };
        table.checksum();
        table
    }

    pub fn checksum(&mut self) {
        self.runtime_services.hdr.crc32 = 0;
        let rs_ptr = self.runtime_services.as_ref() as *const efi::RuntimeServices as *const u8;
        let rs_slice = unsafe { from_raw_parts(rs_ptr, size_of::<efi::RuntimeServices>()) };
        self.runtime_services.hdr.crc32 = crc32fast::hash(rs_slice);
    }
}

pub struct EfiBootServicesTable {
    boot_services: Box<efi::BootServices>, //Use the global allocator (EfiBootServicesData)
}

impl EfiBootServicesTable {
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
    extern "efiapi" fn allocate_pool_unimplemented(_: efi::MemoryType, _: usize, _: *mut *mut c_void) -> efi::Status {
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

    pub fn init() -> EfiBootServicesTable {
        let mut bs = efi::BootServices {
            hdr: efi::TableHeader {
                signature: efi::BOOT_SERVICES_SIGNATURE,
                revision: efi::BOOT_SERVICES_REVISION,
                header_size: 0,
                crc32: 0,
                reserved: 0,
            },
            raise_tpl: Self::raise_tpl_unimplemented,
            restore_tpl: Self::restore_tpl_unimplemented,
            allocate_pages: Self::allocate_pages_unimplemented,
            free_pages: Self::free_pages_unimplemented,
            get_memory_map: Self::get_memory_map_unimplemented,
            allocate_pool: Self::allocate_pool_unimplemented,
            free_pool: Self::free_pool_unimplemented,
            create_event: Self::create_event_unimplemented,
            set_timer: Self::set_timer_unimplemented,
            wait_for_event: Self::wait_for_event_unimplemented,
            signal_event: Self::signal_event_unimplemented,
            close_event: Self::close_event_unimplemented,
            check_event: Self::check_event_unimplemented,
            install_protocol_interface: Self::install_protocol_interface_unimplemented,
            reinstall_protocol_interface: Self::reinstall_protocol_interface_unimplemented,
            uninstall_protocol_interface: Self::uninstall_protocol_interface_unimplemented,
            handle_protocol: Self::handle_protocol_unimplemented,
            reserved: core::ptr::null_mut(),
            register_protocol_notify: Self::register_protocol_notify_unimplemented,
            locate_handle: Self::locate_handle_unimplemented,
            locate_device_path: Self::locate_device_path_unimplemented,
            install_configuration_table: Self::install_configuration_table_unimplemented,
            load_image: Self::load_image_unimplemented,
            start_image: Self::start_image_unimplemented,
            exit: Self::exit_unimplemented,
            unload_image: Self::unload_image_unimplemented,
            exit_boot_services: Self::exit_boot_services_unimplemented,
            get_next_monotonic_count: Self::get_next_monotonic_count_unimplemented,
            stall: Self::stall_unimplemented,
            set_watchdog_timer: Self::set_watchdog_timer_unimplemented,
            connect_controller: Self::connect_controller_unimplemented,
            disconnect_controller: Self::disconnect_controller_unimplemented,
            open_protocol: Self::open_protocol_unimplemented,
            close_protocol: Self::close_protocol_unimplemented,
            open_protocol_information: Self::open_protocol_information_unimplemented,
            protocols_per_handle: Self::protocols_per_handle_unimplemented,
            locate_handle_buffer: Self::locate_handle_buffer_unimplemented,
            locate_protocol: Self::locate_protocol_unimplemented,
            install_multiple_protocol_interfaces: Self::install_multiple_protocol_interfaces_unimplemented,
            uninstall_multiple_protocol_interfaces: Self::uninstall_multiple_protocol_interfaces_unimplemented,
            calculate_crc32: Self::calculate_crc32_unimplemented,
            copy_mem: Self::copy_mem_unimplemented,
            set_mem: Self::set_mem_unimplemented,
            create_event_ex: Self::create_event_ex_unimplemented,
        };

        bs.hdr.header_size = size_of::<efi::BootServices>() as u32;
        let mut table = EfiBootServicesTable { boot_services: Box::new(bs) };
        table.checksum();
        table
    }

    pub fn checksum(&mut self) {
        self.boot_services.hdr.crc32 = 0;
        let bs_ptr = self.boot_services.as_ref() as *const efi::BootServices as *const u8;
        let bs_slice = unsafe { from_raw_parts(bs_ptr, size_of::<efi::BootServices>()) };
        self.boot_services.hdr.crc32 = crc32fast::hash(bs_slice);
    }
}

pub struct EfiSystemTable {
    system_table: Box<efi::SystemTable, &'static dyn Allocator>,
    boot_service: EfiBootServicesTable, // These fields ensure the efi::BootServices and efi::RuntimeServices structure pointers (in
    runtime_service: EfiRuntimeServicesTable, // the system_table) have the same lifetime as the EfiSystemTable.
}

impl EfiSystemTable {
    fn init() -> EfiSystemTable {
        let mut st = efi::SystemTable {
            hdr: efi::TableHeader {
                signature: efi::SYSTEM_TABLE_SIGNATURE,
                revision: efi::SYSTEM_TABLE_REVISION,
                header_size: 0,
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
        };
        let mut bs = EfiBootServicesTable::init();
        let mut rt = EfiRuntimeServicesTable::init();
        st.boot_services = bs.boot_services.as_mut();
        st.runtime_services = rt.runtime_services.as_mut();

        st.hdr.header_size = size_of::<efi::SystemTable>() as u32;

        EfiSystemTable {
            system_table: Box::new_in(st, &EFI_RUNTIME_SERVICES_DATA_ALLOCATOR),
            boot_service: bs,
            runtime_service: rt,
        }
    }

    pub fn as_ptr(&self) -> *const efi::SystemTable {
        self.system_table.as_ref() as *const efi::SystemTable
    }

    #[allow(dead_code)]
    pub fn system_table(&self) -> &efi::SystemTable {
        self.system_table.as_ref()
    }

    #[allow(dead_code)]
    pub fn system_table_mut(&mut self) -> &mut efi::SystemTable {
        self.system_table.as_mut()
    }

    #[allow(dead_code)]
    pub fn boot_services(&self) -> &efi::BootServices {
        unsafe { self.system_table.boot_services.as_ref().expect("BootServices uninitialized") }
    }

    #[allow(dead_code)]
    pub fn boot_services_mut(&mut self) -> &mut efi::BootServices {
        unsafe { self.system_table.boot_services.as_mut().expect("BootServices uninitialized") }
    }

    #[allow(dead_code)]
    pub fn runtime_services(&self) -> &efi::RuntimeServices {
        unsafe { self.system_table.runtime_services.as_ref().expect("RuntimeServices uninitialized") }
    }

    pub fn runtime_services_mut(&mut self) -> &mut efi::RuntimeServices {
        unsafe { self.system_table.runtime_services.as_mut().expect("RuntimeServices uninitialized") }
    }

    pub fn checksum(&mut self) {
        self.system_table.hdr.crc32 = 0;
        let st_ptr = self.system_table.as_ref() as *const efi::SystemTable as *const u8;
        let st_slice = unsafe { from_raw_parts(st_ptr, size_of::<efi::SystemTable>()) };
        self.system_table.hdr.crc32 = crc32fast::hash(st_slice);
    }

    pub fn checksum_runtime_services(&mut self) {
        self.runtime_service.checksum();
    }

    pub fn checksum_boot_services(&mut self) {
        self.boot_service.checksum();
    }

    pub fn checksum_all(&mut self) {
        self.checksum_boot_services();
        self.checksum_runtime_services();
        self.checksum();
    }

    pub fn clear_boot_time_services(&mut self) {
        self.system_table.boot_services = core::ptr::null_mut();
        self.system_table.con_in = core::ptr::null_mut();
        self.system_table.console_in_handle = core::ptr::null_mut();
        self.system_table.con_out = core::ptr::null_mut();
        self.system_table.console_out_handle = core::ptr::null_mut();
        self.system_table.std_err = core::ptr::null_mut();
        self.system_table.standard_error_handle = core::ptr::null_mut();
        self.checksum();
    }
}

impl AsMut<efi::SystemTable> for EfiSystemTable {
    fn as_mut(&mut self) -> &mut efi::SystemTable {
        self.system_table.as_mut()
    }
}

impl AsRef<efi::SystemTable> for EfiSystemTable {
    fn as_ref(&self) -> &efi::SystemTable {
        self.system_table.as_ref()
    }
}

//access to global system table is only through mutex guard, so safe to mark sync/send.
unsafe impl Sync for EfiSystemTable {}
unsafe impl Send for EfiSystemTable {}

pub fn init_system_table() {
    let mut table = EfiSystemTable::init();
    table.checksum();
    _ = SYSTEM_TABLE.lock().insert(table);
}

/// A component to register a callback that recalculates the CRC32 checksum of the system table
/// when certain protocols are installed.
#[derive(IntoComponent, Default)]
pub(crate) struct SystemTableChecksumInstaller;

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
            let mut table = EfiSystemTable::init();
            table.checksum();

            let system_table_crc32 = table.as_ref().hdr.crc32;
            let boot_services_crc32 = table.boot_services_mut().hdr.crc32;
            let runtime_services_crc32 = table.runtime_services_mut().hdr.crc32;

            // Update a boot_services function
            extern "efiapi" fn raise_tpl(_: efi::Tpl) -> efi::Tpl {
                efi::TPL_APPLICATION
            }
            table.boot_services_mut().raise_tpl = raise_tpl;

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
            table.runtime_services_mut().get_variable = get_variable;

            // Update a system_table field
            table.as_mut().hdr.revision = 0x100;

            // Checksums should be different
            table.checksum_all();
            assert_ne!(system_table_crc32, table.system_table_mut().hdr.crc32);
            assert_ne!(boot_services_crc32, table.boot_services_mut().hdr.crc32);
            assert_ne!(runtime_services_crc32, table.runtime_services_mut().hdr.crc32);

            // Check that clearing boot time services changes the checksum
            table.system_table_mut().hdr.revision = efi::RUNTIME_SERVICES_REVISION;
            table.clear_boot_time_services();
            assert_eq!(table.system_table_mut().boot_services, core::ptr::null_mut());
        })
    }
}
