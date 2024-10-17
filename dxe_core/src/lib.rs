//! DXE Core
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

extern crate alloc;

mod allocator;
mod component_interface;
mod dispatcher;
mod driver_services;
mod dxe_services;
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
use mu_pi::{fw_fs, hob::HobList, protocols::bds};
use r_efi::efi::{self};
use uefi_component_interface::DxeComponent;
use uefi_core::{
    error::{self, Result},
    interface,
};
use uefi_gcd::gcd::SpinLockedGcd;

pub static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(events::gcd_map_change));

pub type ComponentEntryPoint = fn() -> Result<()>;

/// The DxeCore object responsible for dispatching all drivers, both local and those found in firmware volumes.
#[derive(Default)]
pub struct Core<CpuInitializer, SectionExtractor>
where
    CpuInitializer: interface::CpuInitializer + Default,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    cpu: CpuInitializer,
    se: SectionExtractor,
}

impl<CpuInitializer, SectionExtractor> Core<CpuInitializer, SectionExtractor>
where
    CpuInitializer: interface::CpuInitializer + Default,
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    /// Registers the CPU initializer with it's own configuration.
    pub fn with_cpu_initializer(mut self, cpu: CpuInitializer) -> Self {
        self.cpu = cpu;
        self
    }

    /// Registers the section extractor with it's own configuration.
    pub fn with_section_extractor(mut self, se: SectionExtractor) -> Self {
        self.se = se;
        self
    }

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    pub fn initialize(mut self, physical_hob_list: *const c_void) -> CorePostInit {
        self.cpu.initialize();
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
            dispatcher::init_dispatcher(Box::from(self.se));
            fv::init_fv_support(&hob_list, Box::from(self.se));
            dxe_services::init_dxe_services(st);
            driver_services::init_driver_services(st.boot_services());
            // re-checksum the system tables after above initialization.
            st.checksum_all();

            // Install HobList configuration table
            let hob_list_guid = uuid::Uuid::from_str("7739F24C-93D7-11D4-9A3A-0090273FC14D").unwrap();
            let hob_list_guid: efi::Guid = unsafe { *(hob_list_guid.to_bytes_le().as_ptr() as *const efi::Guid) };
            misc_boot_services::core_install_configuration_table(
                hob_list_guid,
                unsafe { (physical_hob_list as *mut c_void).as_mut() },
                st,
            )
            .unwrap();
        }

        let mut st = systemtables::SYSTEM_TABLE.lock();
        let bs = st.as_mut().unwrap().boot_services() as *mut efi::BootServices;
        drop(st);
        tpl_lock::init_boot_services(bs);

        memory_attributes_table::init_memory_attributes_table_support();

        return CorePostInit::new(/* Potentially transfer configuration data here. */);
    }
}

/// Struct representing the core after basic initialization has been completed.
///
/// This struct can only be created by the [Core::initialize] function, and is used to dispatch all drivers. It ensures
/// that the GCD has been initialized and that allocations are now available.
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
            image::core_start_local_image(Box::leak(driver)).unwrap();
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
