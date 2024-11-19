//! DXE Core
//!
//! A pure rust implementation of the UEFI DXE Core. Please review the getting started documentation at
//! <https://pop-project.github.io/uefi-dxe-core/> for more information.
//!
//! ## Examples
//!
//! ``` rust,no_run
//! # #[derive(Default, Clone, Copy)]
//! # struct Driver;
//! # impl uefi_component_interface::DxeComponent for Driver {
//! #     fn entry_point(&self, _: &dyn uefi_component_interface::DxeComponentInterface) -> uefi_sdk::error::Result<()> { Ok(()) }
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct CpuInitExample;
//! # impl uefi_cpu_init::EfiCpuInit for CpuInitExample {
//! #     fn initialize(&mut self) -> Result<(), r_efi::efi::Status> {Ok(())}
//! #     fn flush_data_cache(
//! #         &self,
//! #         _start: r_efi::efi::PhysicalAddress,
//! #         _length: u64,
//! #         _flush_type: mu_pi::protocols::cpu_arch::CpuFlushType,
//! #     ) -> Result<(), r_efi::efi::Status> {Ok(())}
//! #     fn enable_interrupt(&self) -> Result<(), r_efi::efi::Status> {Ok(())}
//! #     fn disable_interrupt(&self) -> Result<(), r_efi::efi::Status> {Ok(())}
//! #     fn get_interrupt_state(&self) -> Result<bool, r_efi::efi::Status> {Ok(true)}
//! #     fn init(&self, _init_type: mu_pi::protocols::cpu_arch::CpuInitType) -> Result<(), r_efi::efi::Status> {Ok(())}
//! #     fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), r_efi::efi::Status> {Ok((0, 0))}
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct SectionExtractExample;
//! # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
//! #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
//! # }
//! # #[derive(Default, Clone, Copy)]
//! # struct InterruptManagerExample;
//! # impl uefi_interrupt::InterruptManager for InterruptManagerExample {
//! #     fn initialize(&mut self) -> uefi_sdk::error::Result<()> { Ok(()) }
//! # }
//! # let physical_hob_list = core::ptr::null();
//! dxe_core::Core::default()
//!   .with_cpu_init(CpuInitExample::default())
//!   .with_interrupt_manager(InterruptManagerExample::default())
//!   .with_section_extractor(SectionExtractExample::default())
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
mod image;
mod memory_attributes_table;
mod misc_boot_services;
mod protocols;
mod runtime;
mod systemtables;

#[cfg(test)]
#[macro_use]
pub mod test_support;

use core::{ffi::c_void, str::FromStr};

use alloc::{boxed::Box, vec::Vec};
use gcd::SpinLockedGcd;
use mu_pi::{fw_fs, hob::HobList, protocols::bds};
use r_efi::efi::{self};
use uefi_component_interface::DxeComponent;
use uefi_sdk::{
    error::{self, Result},
    if_aarch64, if_x64,
};

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

if_x64! {
    /// [`Core`] type alias for x86_64 architecture with cpu architecture specific trait implementations pre-selected.
    pub type X64Core<SectionExtractor> = Core<uefi_cpu_init::X64EfiCpuInit, SectionExtractor, uefi_interrupt::InterruptManagerX64>;
}

if_aarch64! {
    /// [`Core`] type alias for aarch64 architecture with cpu architecture specific trait implementations pre-selected.
    pub type Aarch64Core<SectionExtractor> = Core<uefi_cpu_init::NullEfiCpuInit, SectionExtractor, uefi_interrupt::InterruptManagerAarch64>;
}

/// The initialize phase DxeCore, responsible for setting up the environment with the given configuration.
///
/// This struct is the entry point for the DXE Core, which is a two phase system. This struct is responsible for
/// initializing the system and applying any configuration from the `with_*` function calls. During this phase, no
/// allocations are available. Allocations are only available once [initialize](Core::initialize) is called.
///
/// The return type from [initialize](Core::initialize) is a [CorePostInit] object, which signals the completion of
/// the first phase of the DXE Core and that allocations are available. See [CorePostInit] for more information.
///
/// ## Examples
///
/// ``` rust,no_run
/// # #[derive(Default, Clone, Copy)]
/// # struct Driver;
/// # impl uefi_component_interface::DxeComponent for Driver {
/// #     fn entry_point(&self, _: &dyn uefi_component_interface::DxeComponentInterface) -> uefi_sdk::error::Result<()> { Ok(()) }
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct CpuInitExample;
/// # impl uefi_cpu_init::EfiCpuInit for CpuInitExample {
/// #     fn initialize(&mut self) -> Result<(), r_efi::efi::Status> {Ok(())}
/// #     fn flush_data_cache(
/// #         &self,
/// #         _start: r_efi::efi::PhysicalAddress,
/// #         _length: u64,
/// #         _flush_type: mu_pi::protocols::cpu_arch::CpuFlushType,
/// #     ) -> Result<(), r_efi::efi::Status> {Ok(())}
/// #     fn enable_interrupt(&self) -> Result<(), r_efi::efi::Status> {Ok(())}
/// #     fn disable_interrupt(&self) -> Result<(), r_efi::efi::Status> {Ok(())}
/// #     fn get_interrupt_state(&self) -> Result<bool, r_efi::efi::Status> {Ok(true)}
/// #     fn init(&self, _init_type: mu_pi::protocols::cpu_arch::CpuInitType) -> Result<(), r_efi::efi::Status> {Ok(())}
/// #     fn get_timer_value(&self, _timer_index: u32) -> Result<(u64, u64), r_efi::efi::Status> {Ok((0, 0))}
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct SectionExtractExample;
/// # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
/// #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
/// # }
/// # #[derive(Default, Clone, Copy)]
/// # struct InterruptManagerExample;
/// # impl uefi_interrupt::InterruptManager for InterruptManagerExample {
/// #     fn initialize(&mut self) -> uefi_sdk::error::Result<()> { Ok(()) }
/// # }
/// # let physical_hob_list = core::ptr::null();
/// dxe_core::Core::default()
///   .with_cpu_init(CpuInitExample::default())
///   .with_interrupt_manager(InterruptManagerExample::default())
///   .with_section_extractor(SectionExtractExample::default())
///   .initialize(physical_hob_list)
///   .with_driver(Box::new(Driver::default()))
///   .start()
///   .unwrap();
/// ```
#[derive(Default)]
pub struct Core<CpuInit, SectionExtractor, InterruptManager>
where
    CpuInit: uefi_cpu_init::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_interrupt::InterruptManager + Default + Copy + 'static,
{
    cpu_init: CpuInit,
    section_extractor: SectionExtractor,
    interrupt_manager: InterruptManager,
}

impl<CpuInit, SectionExtractor, InterruptManager> Core<CpuInit, SectionExtractor, InterruptManager>
where
    CpuInit: uefi_cpu_init::EfiCpuInit + Default + 'static,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
    InterruptManager: uefi_interrupt::InterruptManager + Default + Copy + 'static,
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

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    pub fn initialize(mut self, physical_hob_list: *const c_void) -> CorePostInit {
        let _ = self.cpu_init.initialize();
        self.interrupt_manager.initialize().expect("Failed to initialize interrupt manager!");
        uefi_debugger::initialize(&mut self.interrupt_manager);

        let (free_memory_start, free_memory_size) = gcd::init_gcd(physical_hob_list);

        log::trace!("Free memory start: {:#x}", free_memory_start);
        log::trace!("Free memory size: {:#x}", free_memory_size);

        // After this point Rust Heap usage is permitted (since GCD is initialized).
        // Relocate the hobs from the input list pointer into a Vec.
        let mut hob_list = HobList::default();
        hob_list.discover_hobs(physical_hob_list);

        log::trace!("HOB list discovered is:");
        log::trace!("{:#x?}", hob_list);

        gcd::add_hob_resource_descriptors_to_gcd(&hob_list, free_memory_start, free_memory_size);

        log::trace!("GCD - After adding resource descriptor HOBs.");
        log::trace!("{:#x?}", GCD);

        gcd::add_hob_allocations_to_gcd(&hob_list);

        log::info!("GCD - After adding memory allocation HOBs.");
        log::info!("{:#x?}", GCD);

        // Instantiate system table.
        systemtables::init_system_table();

        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            let st = st.as_mut().expect("System Table not initialized!");

            allocator::init_memory_support(st.boot_services(), &hob_list);
            events::init_events_support(st.boot_services());
            protocols::init_protocol_support(st.boot_services());
            misc_boot_services::init_misc_boot_services_support(st.boot_services());
            runtime::init_runtime_support(st.runtime_services());
            image::init_image_support(&hob_list, st);
            dispatcher::init_dispatcher(Box::from(self.section_extractor));
            fv::init_fv_support(&hob_list, Box::from(self.section_extractor));
            dxe_services::init_dxe_services(st);
            driver_services::init_driver_services(st.boot_services());

            // Commenting out below install procotcol call until we stub the CPU
            // arch protocol install from C CpuDxe.
            // cpu_arch_protocol::install_cpu_arch_protocol(&mut self.cpu_init, &mut self.interrupt_manager);

            // re-checksum the system tables after above initialization.
            st.checksum_all();

            // Install HobList configuration table
            let hob_list_guid =
                uuid::Uuid::from_str("7739F24C-93D7-11D4-9A3A-0090273FC14D").expect("Invalid UUID format.");
            let hob_list_guid: efi::Guid = unsafe { *(hob_list_guid.to_bytes_le().as_ptr() as *const efi::Guid) };
            misc_boot_services::core_install_configuration_table(
                hob_list_guid,
                unsafe { (physical_hob_list as *mut c_void).as_mut() },
                st,
            )
            .expect("Unable to create configuration table due to invalid table entry.");
        }

        let mut st = systemtables::SYSTEM_TABLE.lock();
        let bs = st.as_mut().unwrap().boot_services() as *mut efi::BootServices;
        drop(st);
        tpl_lock::init_boot_services(bs);

        memory_attributes_table::init_memory_attributes_table_support();

        CorePostInit::new(/* Potentially transfer configuration data here. */)
    }
}

/// The execute phase of the DxeCore, responsible for dispatching all drivers.
///
/// This phase is responsible for dispatching all drivers that have been registered with the core or discovered by the
/// core. This structure cannot be generated directly, but is returned from [Core::initialize]. This phase allows for
/// additional configuration that may require allocations, as allocations are now available. Once all configuration has
/// been completed via the provided `with_*` functions, [start](CorePostInit::start) should be called to begin driver
/// dispatch and handoff to bds.
pub struct CorePostInit {
    drivers: Vec<Box<dyn DxeComponent>>,
}

impl CorePostInit {
    fn new() -> Self {
        Self { drivers: Vec::new() }
    }

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
}
