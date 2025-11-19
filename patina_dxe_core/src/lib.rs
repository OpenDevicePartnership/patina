//! DXE Core
//!
//! A pure rust implementation of the UEFI DXE Core. Please review the getting started documentation at
//! <https://OpenDevicePartnership.github.io/patina/> for more information.
//!
//! ## Examples
//!
//! ``` rust,no_run
//! # use patina::component::prelude::*;
//! # #[derive(IntoComponent, Default)]
//! # struct ExampleComponent;
//! # impl ExampleComponent {
//! #     fn entry_point(self) -> patina::error::Result<()> { Ok(()) }
//! # }
//! # let physical_hob_list = core::ptr::null();
//! patina_dxe_core::Core::default()
//!   .init_memory(physical_hob_list)
//!   .with_service(patina_ffs_extractors::CompositeSectionExtractor::default())
//!   .with_component(ExampleComponent::default())
//!   .start()
//!   .unwrap();
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![feature(alloc_error_handler)]
#![feature(c_variadic)]
#![feature(allocator_api)]
#![feature(coverage_attribute)]

extern crate alloc;

mod allocator;
mod config_tables;
mod cpu_arch_protocol;
mod decompress;
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
mod memory_manager;
mod misc_boot_services;
mod pecoff;
mod perf_timer;
mod protocol_db;
mod protocols;
mod runtime;
mod systemtables;
mod tpl_mutex;

#[cfg(test)]
#[macro_use]
#[coverage(off)]
pub mod test_support;

use core::{ffi::c_void, ptr, str::FromStr};

use alloc::{boxed::Box, vec::Vec};
use gcd::SpinLockedGcd;
use memory_manager::CoreMemoryManager;
use mu_rust_helpers::{function, guid::CALLER_ID};
use patina::{
    boot_services::StandardBootServices,
    component::{Component, IntoComponent, Storage, service::IntoService},
    error::{self, Result},
    performance::{
        logging::{perf_function_begin, perf_function_end},
        measurement::create_performance_measurement,
    },
    pi::{
        hob::{HobList, get_c_hob_list_size},
        protocols::{bds, status_code},
        status_code::{EFI_PROGRESS_CODE, EFI_SOFTWARE_DXE_CORE, EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT},
    },
    runtime_services::StandardRuntimeServices,
};
use patina_ffs::section::SectionExtractor;
use patina_internal_cpu::{cpu::EfiCpu, interrupts::Interrupts};
use protocols::PROTOCOL_DB;
use r_efi::efi;

use crate::{config_tables::memory_attributes_table, perf_timer::PerfTimer};

#[doc(hidden)]
#[macro_export]
macro_rules! ensure {
    ($condition:expr, $err:expr) => {{
        if !($condition) {
            error!($err);
        }
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! error {
    ($err:expr) => {{
        return Err($err.into()).into();
    }};
}

pub(crate) static GCD: SpinLockedGcd = SpinLockedGcd::new(Some(events::gcd_map_change));

/// A configuration struct containing the GIC bases (gic_d, gic_r) for AARCH64 systems.
///
/// ## Example
///
/// ```rust,no_run
/// use patina_dxe_core::{Core, GicBases};
/// # let physical_hob_list = core::ptr::null();
///
/// let gic_bases = GicBases::new(0x1E000000, 0x1E010000);
/// let core = Core::default()
///    .init_memory(physical_hob_list)
///    .with_config(gic_bases)
///    .start()
///    .unwrap();
/// ```
#[derive(Debug, PartialEq)]
pub struct GicBases(pub u64, pub u64);

impl GicBases {
    /// Creates a new instance of the GicBases struct with the provided GIC Distributor and Redistributor base addresses.
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
/// ## Soft Service Dependencies
///
/// The core may take a soft dependency on some services, which are described in the below table. These services must
/// be directly registered with the [Core::with_service] method. If not, there is no guarantee that the service will
/// be available before the core needs it.
///
/// | Service Trait                           | Description                                      |
/// |-----------------------------------------|--------------------------------------------------|
/// | [patina_ffs::section::SectionExtractor] | FW volume section extraction w/ decompression    |
///
/// ## Examples
///
/// ``` rust,no_run
/// # use patina::component::prelude::*;
/// # #[derive(IntoComponent, Default)]
/// # struct ExampleComponent;
/// # impl ExampleComponent {
/// #     fn entry_point(self) -> patina::error::Result<()> { Ok(()) }
/// # }
/// # let physical_hob_list = core::ptr::null();
/// patina_dxe_core::Core::default()
///   .init_memory(physical_hob_list)
///   .with_service(patina_ffs_extractors::CompositeSectionExtractor::default())
///   .with_component(ExampleComponent::default())
///   .start()
///   .unwrap();
/// ```
pub struct Core<MemoryState> {
    physical_hob_list: *const c_void,
    hob_list: HobList<'static>,
    components: Vec<Box<dyn Component>>,
    storage: Storage,
    platform_timer: PerfTimer,
    _memory_state: core::marker::PhantomData<MemoryState>,
}

impl Default for Core<NoAlloc> {
    fn default() -> Self {
        Core {
            physical_hob_list: core::ptr::null(),
            hob_list: HobList::default(),
            components: Vec::new(),
            storage: Storage::new(),
            platform_timer: PerfTimer::new(),
            _memory_state: core::marker::PhantomData,
        }
    }
}

impl Core<NoAlloc> {
    /// Initializes the timer frequency if a platform provides a custom implementation.
    pub fn init_timer_frequency(self, frequency: Option<u64>) -> Core<NoAlloc> {
        if let Some(freq_override) = frequency {
            self.platform_timer.set_frequency(freq_override);
        }
        self
    }

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    pub fn init_memory(mut self, physical_hob_list: *const c_void) -> Core<Alloc> {
        log::info!("DXE Core Crate v{}", env!("CARGO_PKG_VERSION"));

        let mut cpu = EfiCpu::default();
        cpu.initialize().expect("Failed to initialize CPU!");
        let mut interrupt_manager = Interrupts::default();
        interrupt_manager.initialize().expect("Failed to initialize Interrupts!");

        // For early debugging, the "no_alloc" feature must be enabled in the debugger crate.
        // patina_debugger::initialize(&mut interrupt_manager);

        if physical_hob_list.is_null() {
            panic!("HOB list pointer is null!");
        }

        gcd::init_gcd(physical_hob_list);

        log::trace!("Initial GCD:\n{GCD}");

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

        // Add custom monitor commands to the debugger before initializing so that
        // they are available in the initial breakpoint.
        patina_debugger::add_monitor_command("gcd", "Prints the GCD", |_, out| {
            let _ = write!(out, "GCD -\n{GCD}");
        });

        // Initialize the debugger if it is enabled.
        patina_debugger::initialize(&mut interrupt_manager);

        log::info!("GCD - After memory init:\n{GCD}");

        self.storage.add_service(cpu);
        self.storage.add_service(interrupt_manager);
        self.storage.add_service(CoreMemoryManager);
        self.storage.add_service(PerfTimer::with_frequency(self.platform_timer.get_stored_frequency()));

        Core {
            physical_hob_list,
            hob_list: self.hob_list,
            components: self.components,
            storage: self.storage,
            platform_timer: self.platform_timer,
            _memory_state: core::marker::PhantomData,
        }
    }

    /// Informs the core that it should prioritize allocating 32-bit memory when
    /// not otherwise specified.
    ///
    /// This should only be used as a workaround in environments where address width
    /// bugs exist in uncontrollable dependent software. For example, when booting
    /// to an OS that puts any addresses from UEFI into a uint32.
    ///
    /// Must be called prior to [`Core::init_memory`].
    ///
    /// ## Example
    ///
    /// ``` rust,no_run
    /// # use patina::component::prelude::*;
    /// # fn example_component() -> patina::error::Result<()> { Ok(()) }
    /// # let physical_hob_list = core::ptr::null();
    /// patina_dxe_core::Core::default()
    ///   .prioritize_32_bit_memory()
    ///   .init_memory(physical_hob_list)
    ///   .start()
    ///   .unwrap();
    /// ```
    pub fn prioritize_32_bit_memory(self) -> Self {
        // This doesn't actually alter the core's state, but uses the same model
        // for consistent abstraction.
        GCD.prioritize_32_bit_memory(true);
        self
    }
}

impl Core<Alloc> {
    /// Directly registers an instantiated service with the core, making it available immediately.
    #[inline(always)]
    pub fn with_service(mut self, service: impl IntoService + 'static) -> Self {
        self.storage.add_service(service);
        self
    }

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
            if let patina::pi::hob::Hob::GuidHob(guid, data) = hob {
                let parser_funcs = self.storage.get_hob_parsers(&patina::OwnedGuid::from(guid.name));
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
    fn dispatch_components(&mut self) -> bool {
        let len = self.components.len();
        self.components.retain_mut(|component| {
            // Ok(true): Dispatchable and dispatched returning success
            // Ok(false): Not dispatchable at this time.
            // Err(e): Dispatchable and dispatched returning failure
            let name = component.metadata().name();
            log::trace!("Dispatch Start: Id = [{name:?}]");
            !match component.run(&mut self.storage) {
                Ok(true) => {
                    log::info!("Dispatched: Id = [{name:?}] Status = [Success]");
                    true
                }
                Ok(false) => false,
                Err(err) => {
                    log::error!("Dispatched: Id = [{name:?}] Status = [Failed] Error = [{err:?}]");
                    debug_assert!(false);
                    true // Component dispatched, even if it did fail, so remove from self.components to avoid re-dispatch.
                }
            }
        });
        len != self.components.len()
    }

    /// Performs a combined dispatch of Patina components and UEFI drivers.
    ///
    /// This function will continue to loop and perform dispatching until no components have been dispatched in a full
    /// iteration. The dispatching process involves a loop of two distinct dispatch phases:
    ///
    /// 1. A single iteration of dispatching Patina components, retaining those that were not dispatched.
    /// 2. A single iteration of dispatching UEFI drivers via the dispatcher module.
    fn core_dispatcher(&mut self) -> Result<()> {
        perf_function_begin(function!(), &CALLER_ID, create_performance_measurement);
        loop {
            // Patina component dispatch
            let dispatched = self.dispatch_components();

            // UEFI driver dispatch
            let dispatched = dispatched
                || dispatcher::dispatch().inspect_err(|err| log::error!("UEFI Driver Dispatch error: {err:?}"))?;

            if !dispatched {
                break;
            }
        }
        perf_function_end(function!(), &CALLER_ID, create_performance_measurement);

        Ok(())
    }

    fn display_components_not_dispatched(&self) {
        if !self.components.is_empty() {
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
                log::warn!(
                    "{:<max_name_len$} {:<max_param_len$}",
                    metadata.name(),
                    metadata.failed_param().unwrap_or("")
                );
            }
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
            config_tables::init_config_tables_support(st.boot_services_mut());
            runtime::init_runtime_support(st.runtime_services_mut());
            image::init_image_support(&self.hob_list, st);
            dispatcher::init_dispatcher();
            dxe_services::init_dxe_services(st);
            driver_services::init_driver_services(st.boot_services_mut());

            memory_attributes_protocol::install_memory_attributes_protocol();

            // re-checksum the system tables after above initialization.
            st.checksum_all();

            // Install HobList configuration table
            let (a, b, c, &[d0, d1, d2, d3, d4, d5, d6, d7]) =
                uuid::Uuid::from_str("7739F24C-93D7-11D4-9A3A-0090273FC14D").expect("Invalid UUID format.").as_fields();
            let hob_list_guid: efi::Guid = efi::Guid::from_fields(a, b, c, d0, d1, &[d2, d3, d4, d5, d6, d7]);

            config_tables::core_install_configuration_table(
                hob_list_guid,
                Box::leak(relocated_c_hob_list).as_mut_ptr() as *mut c_void,
                st,
            )
            .expect("Unable to create configuration table due to invalid table entry.");

            // Install Memory Type Info configuration table.
            allocator::install_memory_type_info_table(st).expect("Unable to create Memory Type Info Table");
        }

        let boot_services_ptr;
        let runtime_services_ptr;
        {
            let mut st = systemtables::SYSTEM_TABLE.lock();
            let st = st.as_mut().expect("System Table is not initialized!");
            boot_services_ptr = st.boot_services_mut() as *mut efi::BootServices;
            runtime_services_ptr = st.runtime_services_mut() as *mut efi::RuntimeServices;
        }

        memory_attributes_table::init_memory_attributes_table_support();

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
    #[allow(clippy::default_constructed_unit_structs)]
    fn add_core_components(&mut self) {
        self.insert_component(0, decompress::DecompressProtocolInstaller::default().into_component());
        self.insert_component(0, systemtables::SystemTableChecksumInstaller::default().into_component());
        self.insert_component(0, cpu_arch_protocol::CpuArchProtocolInstaller::default().into_component());
        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        self.insert_component(0, hw_interrupt_protocol::HwInterruptProtocolInstaller::default().into_component());
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

        if let Some(extractor) = self.storage.get_service::<dyn SectionExtractor>() {
            log::debug!("Section Extractor service found, registering with FV and Dispatcher.");
            dispatcher::register_section_extractor(extractor.clone());
            fv::register_section_extractor(extractor);
        }

        log::info!("Parsing FVs from FV HOBs");
        fv::parse_hob_fvs(&self.hob_list)?;
        log::info!("Finished.");

        log::info!("Dispatching Drivers");
        self.core_dispatcher()?;
        self.storage.lock_configs();
        self.core_dispatcher()?;
        log::info!("Finished Dispatching Drivers");

        self.display_components_not_dispatched();

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
        let guid = efi::Guid::from_bytes(&uuid.to_bytes_le());
        if protocols::PROTOCOL_DB.locate_protocol(guid).is_err() {
            log::warn!("Missing architectural protocol: {uuid:?}, {name:?}");
        }
    }
}

fn call_bds() {
    // Enable status code capability in Firmware Performance DXE.
    match protocols::PROTOCOL_DB.locate_protocol(status_code::PROTOCOL_GUID) {
        Ok(status_code_ptr) => {
            let status_code_protocol = unsafe { (status_code_ptr as *mut status_code::Protocol).as_mut() }.unwrap();
            (status_code_protocol.report_status_code)(
                EFI_PROGRESS_CODE,
                EFI_SOFTWARE_DXE_CORE | EFI_SW_DXE_CORE_PC_HANDOFF_TO_NEXT,
                0,
                &patina::guids::DXE_CORE,
                ptr::null(),
            );
        }
        Err(err) => log::error!("Unable to locate status code runtime protocol: {err:?}"),
    };

    if let Ok(protocol) = protocols::PROTOCOL_DB.locate_protocol(bds::PROTOCOL_GUID) {
        let bds = protocol as *mut bds::Protocol;
        unsafe {
            // If bds entry returns: then the dispatcher must be invoked again,
            // if it never returns: then an operating system or a system utility have been invoked.
            ((*bds).entry)(bds);
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use crate::Core;

    #[test]
    fn test_core_with_frequency() {
        let frequency = 12345678;
        let core = Core::default().init_timer_frequency(Some(frequency));

        assert!(core.platform_timer.get_stored_frequency() == frequency);
    }
}
