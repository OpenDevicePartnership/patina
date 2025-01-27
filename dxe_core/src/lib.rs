//! DXE Core
//!
//! A pure rust implementation of the UEFI DXE Core. Please review the getting started documentation at
//! <https://OpenDevicePartnership.github.io/uefi-dxe-core/> for more information.
//!
//! ## Examples
//!
//! ``` rust,no_run
//! use uefi_cpu::cpu::EfiCpuInit;
//! use uefi_cpu::interrupts::InterruptManager;
//! use uefi_cpu::interrupts::InterruptBases;
//! use uefi_cpu::interrupts::ExceptionType;
//! use uefi_cpu::interrupts::HandlerType;
//! use uefi_sdk::error::EfiError;
//! # #[derive(Default, Clone, Copy)]
//! # struct Driver;
//! # impl uefi_component_interface::DxeComponent for Driver {
//! #     fn entry_point(&self, _: &dyn uefi_component_interface::DxeComponentInterface) -> uefi_sdk::error::Result<()> { Ok(()) }
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct CpuInitExample;
//! # impl uefi_cpu::cpu::EfiCpuInit for CpuInitExample {
//! #     fn initialize(&mut self) -> Result<(), EfiError> {Ok(())}
//! #     fn flush_data_cache(
//! #         &self,
//! #         _start: r_efi::efi::PhysicalAddress,
//! #         _length: u64,
//! #         _flush_type: mu_pi::protocols::cpu_arch::CpuFlushType,
//! #     ) -> Result<(), EfiError> {Ok(())}
//! #     fn init(&self, _init_type: mu_pi::protocols::cpu_arch::CpuInitType) -> Result<(), EfiError> {Ok(())}
//! #     fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), EfiError> {Ok((0, 0))}
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct SectionExtractExample;
//! # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
//! #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct InterruptManagerExample;
//! # impl uefi_cpu::interrupts::InterruptManager for InterruptManagerExample {
//! #     fn initialize(&mut self) -> uefi_sdk::error::Result<()> { Ok(()) }
//! #     fn register_exception_handler(
//! #        &self,
//! #        exception_type: ExceptionType,
//! #        handler: HandlerType,
//! #    ) -> Result<(), EfiError> { Ok(()) }
//! #     fn unregister_exception_handler(
//! #        &self,
//! #        exception_type: ExceptionType,
//! #    ) -> Result<(), EfiError> { Ok(()) }
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct InterruptBasesExample;
//! # impl uefi_cpu::interrupts::InterruptBases for InterruptBasesExample {
//! #     fn get_interrupt_base_d(&self) -> u64 { 0 }
//! #     fn get_interrupt_base_r(&self) -> u64 { 0 }
//! # }
//! # let physical_hob_list = core::ptr::null();
//! dxe_core::Core::default()
//!   .with_cpu_init(CpuInitExample::default())
//!   .with_interrupt_manager(InterruptManagerExample::default())
//!   .with_section_extractor(SectionExtractExample::default())
//!   .with_interrupt_bases(InterruptBasesExample::default())
//!   .initialize(physical_hob_list)
//!   .with_driver(Box::new(Driver::default()))
//!   .start()
//!   .unwrap();
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![feature(alloc_error_handler)]
#![feature(c_variadic)]
#![feature(allocator_api)]
#![feature(new_uninit)]
#![feature(const_mut_refs)]
#![feature(slice_ptr_get)]
#![feature(get_many_mut)]
#![feature(is_sorted)]

extern crate alloc;

mod allocator;
mod component_interface;
mod cpu_arch_protocol;
mod dispatcher;
mod driver_services;
mod dxe_services;
mod event_db;
mod events;
mod filesystems;
mod fv;
mod gcd;
#[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
mod hw_interrupt_protocol;
mod image;
mod memory_attributes_protocol;
mod memory_attributes_table;
mod misc_boot_services;
mod pecoff;
mod protocol_db;
mod protocols;
mod runtime;
mod systemtables;
mod tpl_lock;

#[cfg(test)]
#[macro_use]
pub mod test_support;

use core::{ffi::c_void, ptr, str::FromStr};

use alloc::{boxed::Box, vec::Vec};
use gcd::SpinLockedGcd;
use mu_pi::{
    fw_fs,
    hob::{get_c_hob_list_size, HobList},
    protocols::{bds, status_code},
    status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_CORE, EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT},
};
use protocols::PROTOCOL_DB;
use r_efi::efi;
use uefi_component_interface::DxeComponent;
use uefi_sdk::error::{self, Result};

#[macro_export]
macro_rules! ensure {
    ($condition:expr, $err:expr) => {{
        if !($condition) {
            error!($err);
        }
    }};
}

#[macro_export]
macro_rules! error {
    ($err:expr) => {{
        return Err($err.into()).into();
    }};
}

pub(crate) static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(events::gcd_map_change));

#[doc(hidden)]
/// A zero-sized type to gate allocation functions in the [Core].
pub struct Alloc;

#[doc(hidden)]
/// A zero-sized type to gate non-allocation functions in the [Core].
pub struct NoAlloc;
/// The initialize phase DxeCore, responsible for setting up the environment with the given configuration.
///
/// This struct is the entry point for the DXE Core, which is a two phase system. The current phase is denoted by the
/// current struct representing the generic parameter "MemoryState". Creating a [Core] object will initialize the
/// struct in the `NoAlloc` phase. Calling the [initialize](Core::initialize) method will transition the struct
/// to the `Alloc` phase. Each phase provides a subset of methods that are available to the struct, allowing
/// for a more controlled configuration and execution process.
///
/// During the `NoAlloc` phase, the struct provides methods to configure the DXE core environment
/// prior to allocation capability such as CPU functionality and section extraction. During this time,
/// no allocations are available.
///
/// Once the [initialize](Core::initialize) method is called, the struct transitions to the `Alloc` phase,
/// which provides methods for adding configuration and components with the DXE core, and eventually starting the
/// dispatching process and eventual handoff to the BDS phase.
///
/// ## Examples
///
/// ``` rust,no_run
/// use uefi_cpu::cpu::EfiCpuInit;
/// use uefi_cpu::interrupts::InterruptManager;
/// use uefi_cpu::interrupts::ExceptionType;
/// use uefi_cpu::interrupts::HandlerType;
/// use uefi_sdk::error::EfiError;
/// # #[derive(Default, Clone, Copy)]
/// # struct Driver;
/// # impl uefi_component_interface::DxeComponent for Driver {
/// #     fn entry_point(&self, _: &dyn uefi_component_interface::DxeComponentInterface) -> uefi_sdk::error::Result<()> { Ok(()) }
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct CpuInitExample;
/// # impl EfiCpuInit for CpuInitExample {
/// #     fn initialize(&mut self) -> Result<(), EfiError> {Ok(())}
/// #     fn flush_data_cache(
/// #         &self,
/// #         _start: r_efi::efi::PhysicalAddress,
/// #         _length: u64,
/// #         _flush_type: mu_pi::protocols::cpu_arch::CpuFlushType,
/// #     ) -> Result<(), EfiError> {Ok(())}
/// #     fn init(&self, _init_type: mu_pi::protocols::cpu_arch::CpuInitType) -> Result<(), EfiError> {Ok(())}
/// #     fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), EfiError> {Ok((0, 0))}
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct SectionExtractExample;
/// # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
/// #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct InterruptManagerExample;
/// # impl uefi_cpu::interrupts::InterruptManager for InterruptManagerExample {
/// #     fn initialize(&mut self) -> uefi_sdk::error::Result<()> { Ok(()) }
/// #     fn register_exception_handler(
/// #        &self,
/// #        exception_type: ExceptionType,
/// #        handler: HandlerType,
/// #    ) -> Result<(), EfiError> { Ok(()) }
/// #     fn unregister_exception_handler(
/// #        &self,
/// #        exception_type: ExceptionType,
/// #    ) -> Result<(), EfiError> { Ok(()) }
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct InterruptBasesExample;
/// # impl uefi_cpu::interrupts::InterruptBases for InterruptBasesExample {
/// #     fn get_interrupt_base_d(&self) -> u64 { 0 }
/// #     fn get_interrupt_base_r(&self) -> u64 { 0 }
/// # }
/// # let physical_hob_list = core::ptr::null();
/// dxe_core::Core::default()
///   .with_cpu_init(CpuInitExample::default())
///   .with_interrupt_manager(InterruptManagerExample::default())
///   .with_section_extractor(SectionExtractExample::default())
///   .with_interrupt_bases(InterruptBasesExample::default())
///   .initialize(physical_hob_list)
///   .with_driver(Box::new(Driver::default()))
///   .start()
///   .unwrap();
/// ```
pub struct Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, MemoryState>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    cpu_init: CpuInit,
    section_extractor: SectionExtractor,
    interrupt_manager: InterruptManager,
    interrupt_bases: InterruptBases,
    drivers: Vec<Box<dyn DxeComponent>>,
    _memory_state: core::marker::PhantomData<MemoryState>,
}

impl<CpuInit, SectionExtractor, InterruptManager, InterruptBases> Default
    for Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, NoAlloc>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    fn default() -> Self {
        Core {
            cpu_init: CpuInit::default(),
            section_extractor: SectionExtractor::default(),
            interrupt_manager: InterruptManager::default(),
            interrupt_bases: InterruptBases::default(),
            drivers: Vec::new(),
            _memory_state: core::marker::PhantomData,
        }
    }
}

impl<CpuInit, SectionExtractor, InterruptManager, InterruptBases>
    Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, NoAlloc>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    /// Registers the CPU Init with it's own configuration.
    pub fn with_cpu_init(mut self, cpu_init: CpuInit) -> Self {
        self.cpu_init = cpu_init;
        self
    }

    /// Registers the Interrupt Manager with it's own configuration.
    pub fn with_interrupt_manager(mut self, interrupt_manager: InterruptManager) -> Self {
        self.interrupt_manager = interrupt_manager;
        self
    }

    /// Registers the section extractor with it's own configuration.
    pub fn with_section_extractor(mut self, section_extractor: SectionExtractor) -> Self {
        self.section_extractor = section_extractor;
        self
    }

    /// Returns the length of the HOB list.
    /// Clippy gets unhappy if we call get_c_hob_list_size directly, because it gets confused, thinking
    /// get_c_hob_list_size is not marked unsafe, but it is
    fn get_hob_list_len(hob_list: *const c_void) -> usize {
        unsafe { get_c_hob_list_size(hob_list) }
    }

    /// Registers the interrupt bases with it's own configuration.
    pub fn with_interrupt_bases(mut self, interrupt_bases: InterruptBases) -> Self {
        self.interrupt_bases = interrupt_bases;
        self
    }

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    pub fn initialize(
        mut self,
        physical_hob_list: *const c_void,
    ) -> Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, Alloc> {
        let _ = self.cpu_init.initialize();
        self.interrupt_manager.initialize().expect("Failed to initialize interrupt manager!");

        // For early debugging, the "no_alloc" feature must be enabled in the debugger crate.
        // uefi_debugger::initialize(&mut self.interrupt_manager);

        if physical_hob_list.is_null() {
            panic!("HOB list pointer is null!");
        }

        gcd::init_gcd(physical_hob_list);

        log::trace!("Initial GCD:\n{}", GCD);

        // After this point Rust Heap usage is permitted (since GCD is initialized with a single known-free region).
        // Relocate the hobs from the input list pointer into a Vec.
        let mut hob_list = HobList::default();
        hob_list.discover_hobs(physical_hob_list);

        log::trace!("HOB list discovered is:");
        log::trace!("{:#x?}", hob_list);

        //make sure that well-known handles exist.
        PROTOCOL_DB.init_protocol_db();
        // Initialize full allocation support.
        allocator::init_memory_support(&hob_list);
        // we have to relocate HOBs after memory services are initialized as we are going to allocate memory and
        // the initial free memory may not be enough to contain the HOB list. We need to relocate the HOBs because
        // the initial HOB list is not in mapped memory as passed from pre-DXE.
        hob_list.relocate_hobs();
        let hob_list_slice = unsafe {
            core::slice::from_raw_parts(physical_hob_list as *const u8, Self::get_hob_list_len(physical_hob_list))
        };
        let relocated_c_hob_list = hob_list_slice.to_vec().into_boxed_slice();

        // Initialize the debugger if it is enabled.
        uefi_debugger::initialize(&mut self.interrupt_manager);

        log::info!("GCD - After memory init:\n{}", GCD);

        // Instantiate system table.
        systemtables::init_system_table();
        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            let st = st.as_mut().expect("System Table not initialized!");

            allocator::install_memory_services(st.boot_services_mut());
            gcd::init_paging(&hob_list);
            events::init_events_support(st.boot_services_mut());
            protocols::init_protocol_support(st.boot_services_mut());
            misc_boot_services::init_misc_boot_services_support(st.boot_services_mut());
            runtime::init_runtime_support(st.runtime_services_mut());
            image::init_image_support(&hob_list, st);
            dispatcher::init_dispatcher(Box::from(self.section_extractor));
            fv::init_fv_support(&hob_list, Box::from(self.section_extractor));
            dxe_services::init_dxe_services(st);
            driver_services::init_driver_services(st.boot_services_mut());

            cpu_arch_protocol::install_cpu_arch_protocol(&mut self.cpu_init, &mut self.interrupt_manager);
            memory_attributes_protocol::install_memory_attributes_protocol();
            #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
            hw_interrupt_protocol::install_hw_interrupt_protocol(&mut self.interrupt_manager, &self.interrupt_bases);

            // re-checksum the system tables after above initialization.
            st.checksum_all();

            // Install HobList configuration table
            let hob_list_guid =
                uuid::Uuid::from_str("7739F24C-93D7-11D4-9A3A-0090273FC14D").expect("Invalid UUID format.");
            let hob_list_guid: efi::Guid = unsafe { *(hob_list_guid.to_bytes_le().as_ptr() as *const efi::Guid) };

            misc_boot_services::core_install_configuration_table(
                hob_list_guid,
                Some(unsafe { &mut *(Box::leak(relocated_c_hob_list).as_mut_ptr() as *mut c_void) }),
                st,
            )
            .expect("Unable to create configuration table due to invalid table entry.");

            // Install Memory Type Info configuration table.
            allocator::install_memory_type_info_table(st).expect("Unable to create Memory Type Info Table");
        }

        let boot_services_ptr;
        // let runtime_services_ptr;
        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            boot_services_ptr = st.as_mut().unwrap().boot_services_mut() as *mut efi::BootServices;
            // runtime_services_ptr = st.as_mut().unwrap().runtime_services_mut() as *mut efi::RuntimeServices;
        }
        tpl_lock::init_boot_services(boot_services_ptr);

        memory_attributes_table::init_memory_attributes_table_support();

        // This is currently commented out as it is breaking top of tree booting Q35 as qemu64 does not support
        // reading the time stamp counter in the way done in this code and results in a divide by zero exception.
        // Other cpu models crash in various other ways. It will be resolved, but is removed now to unblock other
        // development
        // _ = uefi_performance::init_performance_lib(&hob_list, unsafe { boot_services_ptr.as_ref().unwrap() }, unsafe {
        //     runtime_services_ptr.as_ref().unwrap()
        // });

        Core {
            cpu_init: self.cpu_init,
            section_extractor: self.section_extractor,
            interrupt_manager: self.interrupt_manager,
            interrupt_bases: self.interrupt_bases,
            drivers: self.drivers,
            _memory_state: core::marker::PhantomData,
        }
    }
}

impl<CpuInit, SectionExtractor, InterruptManager, InterruptBases>
    Core<CpuInit, SectionExtractor, InterruptManager, InterruptBases, Alloc>
where
    CpuInit: uefi_cpu::cpu::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_cpu::interrupts::InterruptManager + Default + Copy + 'static,
    InterruptBases: uefi_cpu::interrupts::InterruptBases + Default + Copy + 'static,
{
    /// Registers a driver to be dispatched by the core.
    pub fn with_driver(mut self, driver: Box<dyn DxeComponent>) -> Self {
        self.drivers.push(driver);
        self
    }

    /// Starts the core, dispatching all drivers.
    pub fn start(self) -> Result<()> {
        log::info!("Dispatching Local Drivers");
        for driver in self.drivers {
            // This leaks the driver, making it static for the lifetime of the program.
            // Since the number of drivers is fixed and this function can only be called once (due to
            // `self` instead of `&self`), we don't have to worry about leaking memory.
            if let Err(driver_err) = image::core_start_local_image(Box::leak(driver)) {
                debug_assert!(false, "Driver failed with status {:?}", driver_err);
                log::error!("Driver failed with status {:?}", driver_err);
            }
        }

        dispatcher::core_dispatcher().expect("initial dispatch failed.");

        core_display_missing_arch_protocols();

        dispatcher::display_discovered_not_dispatched();

        call_bds();

        log::info!("Finished");
        Ok(())
    }
}

const ARCH_PROTOCOLS: &[(uuid::Uuid, &str)] = &[
    (uuid::uuid!("a46423e3-4617-49f1-b9ff-d1bfa9115839"), "Security"),
    (uuid::uuid!("26baccb1-6f42-11d4-bce7-0080c73c8881"), "Cpu"),
    (uuid::uuid!("26baccb2-6f42-11d4-bce7-0080c73c8881"), "Metronome"),
    (uuid::uuid!("26baccb3-6f42-11d4-bce7-0080c73c8881"), "Timer"),
    (uuid::uuid!("665e3ff6-46cc-11d4-9a38-0090273fc14d"), "Bds"),
    (uuid::uuid!("665e3ff5-46cc-11d4-9a38-0090273fc14d"), "Watchdog"),
    (uuid::uuid!("b7dfb4e1-052f-449f-87be-9818fc91b733"), "Runtime"),
    (uuid::uuid!("1e5668e2-8481-11d4-bcf1-0080c73c8881"), "Variable"),
    (uuid::uuid!("6441f818-6362-4e44-b570-7dba31dd2453"), "Variable Write"),
    (uuid::uuid!("5053697e-2cbc-4819-90d9-0580deee5754"), "Capsule"),
    (uuid::uuid!("1da97072-bddc-4b30-99f1-72a0b56fff2a"), "Monotonic Counter"),
    (uuid::uuid!("27cfac88-46cc-11d4-9a38-0090273fc14d"), "Reset"),
    (uuid::uuid!("27cfac87-46cc-11d4-9a38-0090273fc14d"), "Real Time Clock"),
];

fn core_display_missing_arch_protocols() {
    for (uuid, name) in ARCH_PROTOCOLS {
        let guid: efi::Guid = unsafe { core::mem::transmute(uuid.to_bytes_le()) };
        if protocols::PROTOCOL_DB.locate_protocol(guid).is_err() {
            log::warn!("Missing architectural protocol: {:?}, {:?}", uuid, name);
        }
    }
}

fn call_bds() {
    if let Ok(protocol) = protocols::PROTOCOL_DB.locate_protocol(bds::PROTOCOL_GUID) {
        let bds = protocol as *mut bds::Protocol;
        unsafe {
            ((*bds).entry)(bds);
        }
    }

    match protocols::PROTOCOL_DB.locate_protocol(status_code::PROTOCOL_GUID) {
        Ok(status_code_ptr) => {
            let status_code_protocol = unsafe { (status_code_ptr as *mut status_code::Protocol).as_mut() }.unwrap();
            (status_code_protocol.report_status_code)(
                EFI_PROGRESS_CODE,
                EFI_SOFTWARE_DXE_CORE | EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT,
                0,
                &uefi_sdk::guid::DXE_CORE,
                ptr::null(),
            );
        }
        Err(err) => log::error!("Unable to locate status code runtime protocol: {:?}", err),
    };
}
