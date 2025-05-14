//! DXE Core
//!
//! A pure rust implementation of the UEFI DXE Core. Please review the getting started documentation at
//! <https://OpenDevicePartnership.github.io/patina/> for more information.
//!
//! ## Examples
//!
//! ``` rust,no_run
//! use uefi_sdk::error::EfiError;
//! # fn example_component() -> uefi_sdk::error::Result<()> { Ok(()) }
//! # #[derive(Default, Clone, Copy)]
//! # struct SectionExtractExample;
//! # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
//! #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
//! # }
//! # let physical_hob_list = core::ptr::null();
//! dxe_core::Core::default()
//!   .with_section_extractor(SectionExtractExample::default())
//!   .init_memory(physical_hob_list)
//!   .with_component(example_component)
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
#![feature(slice_ptr_get)]
#![feature(get_many_mut)]

extern crate alloc;

mod allocator;
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
mod memory_manager;
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
use memory_manager::CoreMemoryManager;
use mu_pi::{
    fw_fs,
    hob::{get_c_hob_list_size, HobList},
    protocols::{bds, status_code},
    status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_CORE, EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT},
};
use protocols::PROTOCOL_DB;
use r_efi::efi;
use uefi_cpu::{cpu::EfiCpu, interrupts::Interrupts};
use uefi_sdk::{
    boot_services::StandardBootServices,
    component::{Component, IntoComponent, Storage},
    error::{self, Result},
    runtime_services::StandardRuntimeServices,
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

#[derive(Debug, PartialEq)]
pub struct GicBases(pub u64, pub u64);

impl GicBases {
    pub fn new(gicd_base: u64, gicr_base: u64) -> Self {
        GicBases(gicd_base, gicr_base)
    }
}

impl Default for GicBases {
    fn default() -> Self {
        panic!("GicBases `Config` must be manually initialized and registered with the Core using `with_config`.");
    }
}

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
/// struct in the `NoAlloc` phase. Calling the [init_memory](Core::init_memory) method will transition the struct
/// to the `Alloc` phase. Each phase provides a subset of methods that are available to the struct, allowing
/// for a more controlled configuration and execution process.
///
/// During the `NoAlloc` phase, the struct provides methods to configure the DXE core environment
/// prior to allocation capability such as CPU functionality and section extraction. During this time,
/// no allocations are available.
///
/// Once the [init_memory](Core::init_memory) method is called, the struct transitions to the `Alloc` phase,
/// which provides methods for adding configuration and components with the DXE core, and eventually starting the
/// dispatching process and eventual handoff to the BDS phase.
///
/// ## Examples
///
/// ``` rust,no_run
/// use uefi_sdk::error::EfiError;
/// # fn example_component() -> uefi_sdk::error::Result<()> { Ok(()) }
/// # #[derive(Default, Clone, Copy)]
/// # struct SectionExtractExample;
/// # impl mu_pi::fw_fs::SectionExtractor for SectionExtractExample {
/// #     fn extract(&self, _: &mu_pi::fw_fs::Section) -> Result<Box<[u8]>, r_efi::base::Status> { Ok(Box::new([0])) }
/// # }
/// # let physical_hob_list = core::ptr::null();
/// dxe_core::Core::default()
///   .with_section_extractor(SectionExtractExample::default())
///   .init_memory(physical_hob_list)
///   .with_component(example_component)
///   .start()
///   .unwrap();
/// ```
pub struct Core<SectionExtractor, MemoryState>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    physical_hob_list: *const c_void,
    hob_list: HobList<'static>,
    section_extractor: SectionExtractor,
    components: Vec<Box<dyn Component>>,
    storage: Storage,
    _memory_state: core::marker::PhantomData<MemoryState>,
}

impl<SectionExtractor> Default for Core<SectionExtractor, NoAlloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    fn default() -> Self {
        Core {
            physical_hob_list: core::ptr::null(),
            hob_list: HobList::default(),
            section_extractor: SectionExtractor::default(),
            components: Vec::new(),
            storage: Storage::new(),
            _memory_state: core::marker::PhantomData,
        }
    }
}

impl<SectionExtractor> Core<SectionExtractor, NoAlloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    /// Registers the section extractor with it's own configuration.
    pub fn with_section_extractor(mut self, section_extractor: SectionExtractor) -> Self {
        self.section_extractor = section_extractor;
        self
    }

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    pub fn init_memory(mut self, physical_hob_list: *const c_void) -> Core<SectionExtractor, Alloc> {
        log::info!("DXE Core Crate v{}", env!("CARGO_PKG_VERSION"));

        let mut cpu = EfiCpu::default();
        cpu.initialize().expect("Failed to initialize CPU!");
        let mut interrupt_manager = Interrupts::default();
        interrupt_manager.initialize().expect("Failed to initialize Interrupts!");

        // For early debugging, the "no_alloc" feature must be enabled in the debugger crate.
        // uefi_debugger::initialize(&mut interrupt_manager);

        if physical_hob_list.is_null() {
            panic!("HOB list pointer is null!");
        }

        gcd::init_gcd(physical_hob_list);

        log::trace!("Initial GCD:\n{}", GCD);

        // After this point Rust Heap usage is permitted (since GCD is initialized with a single known-free region).
        // Relocate the hobs from the input list pointer into a Vec.
        self.hob_list.discover_hobs(physical_hob_list);

        log::trace!("HOB list discovered is:");
        log::trace!("{:#x?}", self.hob_list);

        //make sure that well-known handles exist.
        PROTOCOL_DB.init_protocol_db();
        // Initialize full allocation support.
        allocator::init_memory_support(&self.hob_list);
        // we have to relocate HOBs after memory services are initialized as we are going to allocate memory and
        // the initial free memory may not be enough to contain the HOB list. We need to relocate the HOBs because
        // the initial HOB list is not in mapped memory as passed from pre-DXE.
        self.hob_list.relocate_hobs();

        // Initialize the debugger if it is enabled.
        uefi_debugger::initialize(&mut interrupt_manager);

        log::info!("GCD - After memory init:\n{}", GCD);

        self.storage.add_service(cpu);
        self.storage.add_service(interrupt_manager);
        self.storage.add_service(CoreMemoryManager);

        Core {
            physical_hob_list,
            hob_list: self.hob_list,
            section_extractor: self.section_extractor,
            components: self.components,
            storage: self.storage,
            _memory_state: core::marker::PhantomData,
        }
    }
}

impl<SectionExtractor> Core<SectionExtractor, Alloc>
where
    SectionExtractor: fw_fs::SectionExtractor + Default + Copy + 'static,
{
    /// Registers a component with the core, that will be dispatched during the driver execution phase.
    #[inline(always)]
    pub fn with_component<I>(mut self, component: impl IntoComponent<I>) -> Self {
        self.insert_component(self.components.len(), component.into_component());
        self
    }

    /// Inserts a component at the given index. If no index is provided, the component is added to the end of the list.
    fn insert_component(&mut self, idx: usize, mut component: Box<dyn Component>) {
        component.initialize(&mut self.storage);
        self.components.insert(idx, component);
    }

    /// Adds a configuration value to the Core's storage. All configuration is locked by default. If a component is
    /// present that requires a mutable configuration, it will automatically be unlocked.
    pub fn with_config<C: Default + 'static>(mut self, config: C) -> Self {
        self.storage.add_config(config);
        self
    }

    /// Parses the HOB list producing a `Hob\<T\>` struct for each guided HOB found with a registered parser.
    fn parse_hobs(&mut self) {
        for hob in self.hob_list.iter() {
            if let mu_pi::hob::Hob::GuidHob(guid, data) = hob {
                let parser_funcs = self.storage.get_hob_parsers(&guid.name);
                if parser_funcs.is_empty() {
                    let (f0, f1, f2, f3, f4, &[f5, f6, f7, f8, f9, f10]) = guid.name.as_fields();
                    let name = alloc::format!(
                        "{f0:08x}-{f1:04x}-{f2:04x}-{f3:02x}{f4:02x}-{f5:02x}{f6:02x}{f7:02x}{f8:02x}{f9:02x}{f10:02x}"
                    );
                    log::warn!(
                        "No parser registered for HOB: GuidHob {{ {:?}, name: Guid {{ {} }} }}",
                        guid.header,
                        name
                    );
                } else {
                    for parser_func in parser_funcs {
                        parser_func(data, &mut self.storage);
                    }
                }
            }
        }
    }

    /// Attempts to dispatch all components.
    ///
    /// This method will exit once no components remain or no components were dispatched during a full iteration.
    fn dispatch_components(&mut self) {
        loop {
            let len = self.components.len();
            self.components.retain_mut(|component| {
                // Ok(true): Dispatchable and dispatched returning success
                // Ok(false): Not dispatchable at this time.
                // Err(e): Dispatchable and dispatched returning failure
                log::info!("DISPATCH_ATTEMPT BEGIN: Id = [{:?}]", component.metadata().name());
                !match component.run(&mut self.storage) {
                    Ok(true) => {
                        log::info!("DISPATCH_ATTEMPT END: Id = [{:?}] Status = [Success]", component.metadata().name());
                        true
                    }
                    Ok(false) => {
                        log::info!("DISPATCH_ATTEMPT END: Id = [{:?}] Status = [Skipped]", component.metadata().name());
                        false
                    }
                    Err(err) => {
                        log::error!(
                            "DISPATCH_ATTEMPT END: Id = [{:?}] Status = [Failed] Error = [{:?}]",
                            component.metadata().name(),
                            err
                        );
                        debug_assert!(false);
                        true // Component dispatched, even if it did fail, so remove from self.components to avoid re-dispatch.
                    }
                }
            });
            if self.components.len() == len {
                break;
            }
        }
    }

    fn display_components_not_dispatched(&self) {
        let name_len = "name".len();
        let param_len = "failed_param".len();

        let max_name_len = self.components.iter().map(|c| c.metadata().name().len()).max().unwrap_or(name_len);
        let max_param_len = self
            .components
            .iter()
            .map(|c| c.metadata().failed_param().map(|s| s.len()).unwrap_or(0))
            .max()
            .unwrap_or(param_len);

        log::warn!("Components not dispatched:");
        log::warn!("{:-<max_name_len$} {:-<max_param_len$}", "", "");
        log::warn!("{:<max_name_len$} {:<max_param_len$}", "name", "failed_param");

        for component in &self.components {
            let metadata = component.metadata();
            log::warn!("{:<max_name_len$} {:<max_param_len$}", metadata.name(), metadata.failed_param().unwrap_or(""));
        }
    }

    /// Returns the length of the HOB list.
    /// Clippy gets unhappy if we call get_c_hob_list_size directly, because it gets confused, thinking
    /// get_c_hob_list_size is not marked unsafe, but it is
    fn get_hob_list_len(hob_list: *const c_void) -> usize {
        unsafe { get_c_hob_list_size(hob_list) }
    }

    fn initialize_system_table(&mut self) -> Result<()> {
        let hob_list_slice = unsafe {
            core::slice::from_raw_parts(
                self.physical_hob_list as *const u8,
                Self::get_hob_list_len(self.physical_hob_list),
            )
        };
        let relocated_c_hob_list = hob_list_slice.to_vec().into_boxed_slice();

        // Instantiate system table.
        systemtables::init_system_table();
        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            let st = st.as_mut().expect("System Table not initialized!");

            allocator::install_memory_services(st.boot_services_mut());
            gcd::init_paging(&self.hob_list);
            events::init_events_support(st.boot_services_mut());
            protocols::init_protocol_support(st.boot_services_mut());
            misc_boot_services::init_misc_boot_services_support(st.boot_services_mut());
            runtime::init_runtime_support(st.runtime_services_mut());
            image::init_image_support(&self.hob_list, st);
            dispatcher::init_dispatcher(Box::from(self.section_extractor));
            fv::init_fv_support(&self.hob_list, Box::from(self.section_extractor));
            dxe_services::init_dxe_services(st);
            driver_services::init_driver_services(st.boot_services_mut());

            memory_attributes_protocol::install_memory_attributes_protocol();

            // re-checksum the system tables after above initialization.
            st.checksum_all();

            // Install HobList configuration table
            let (a, b, c, &[d0, d1, d2, d3, d4, d5, d6, d7]) =
                uuid::Uuid::from_str("7739F24C-93D7-11D4-9A3A-0090273FC14D").expect("Invalid UUID format.").as_fields();
            let hob_list_guid: efi::Guid = efi::Guid::from_fields(a, b, c, d0, d1, &[d2, d3, d4, d5, d6, d7]);

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
        let runtime_services_ptr;
        let system_table_ptr;
        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            let st = st.as_mut().expect("System Table is not initialized!");
            boot_services_ptr = st.boot_services_mut() as *mut efi::BootServices;
            runtime_services_ptr = st.runtime_services_mut() as *mut efi::RuntimeServices;
            system_table_ptr = st.system_table() as *const efi::SystemTable;
        }

        tpl_lock::init_boot_services(boot_services_ptr);

        memory_attributes_table::init_memory_attributes_table_support();

        // This is currently commented out as it is breaking top of tree booting Q35 as qemu64 does not support
        // reading the time stamp counter in the way done in this code and results in a divide by zero exception.
        // Other cpu models crash in various other ways. It will be resolved, but is removed now to unblock other
        // development
        _ = uefi_performance::init_performance_lib(
            &self.hob_list,
            // SAFETY: `system_table_ptr` is a valid pointer that has been initialized earlier.
            unsafe { system_table_ptr.as_ref() }.expect("System Table not initialized!"),
        );

        // Add Boot Services and Runtime Services to storage.
        // SAFETY: This is valid because these pointer live thoughout the boot.
        // Note: I had to use the ptr instead of locking the table which event though is static does not seems to return static refs. Need to investigate.
        unsafe {
            self.storage.set_boot_services(StandardBootServices::new(&*boot_services_ptr));
            self.storage.set_runtime_services(StandardRuntimeServices::new(&*runtime_services_ptr));
        }

        Ok(())
    }

    /// Registers core provided components
    fn add_core_components(&mut self) {
        self.insert_component(0, cpu_arch_protocol::install_cpu_arch_protocol.into_component());
        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        self.insert_component(0, hw_interrupt_protocol::install_hw_interrupt_protocol.into_component());
    }

    /// Starts the core, dispatching all drivers.
    pub fn start(mut self) -> Result<()> {
        log::info!("Registering default components");
        self.add_core_components();
        log::info!("Finished.");

        log::info!("Initializing System Table");
        self.initialize_system_table()?;
        log::info!("Finished.");

        log::info!("Parsing HOB list for Guided HOBs.");
        self.parse_hobs();
        log::info!("Finished.");

        log::info!("Dispatching Local Drivers");
        self.dispatch_components();
        self.storage.lock_configs();
        self.dispatch_components();
        log::info!("Finished Dispatching Local Drivers");
        self.display_components_not_dispatched();

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
