//! DXE Core
//!
//! A pure rust implementation of the UEFI DXE Core. Please review the getting started documentation at
//! <https://OpenDevicePartnership.github.io/patina/> for more information.
//!
//! ## Examples
//!
//! ```rust
//! # use core::ffi::c_void;
//! # use patina_dxe_core::*;
//! # use patina_ffs_extractors::NullSectionExtractor;
//! # #[derive(patina::component::IntoComponent, Default)]
//! # struct ExampleComponent;
//! # impl ExampleComponent {
//! #     fn entry_point(self) -> patina::error::Result<()> { Ok(()) }
//! # }
//! struct ExamplePlatform;
//!
//! impl ComponentInfo for ExamplePlatform {
//!   fn configs(mut add: Add<Config>) {
//!     add.config(32u32);
//!     add.config(true);
//!   }
//!
//!   fn components(mut add: Add<Component>) {
//!     add.component(ExampleComponent::default());
//!   }
//! }
//!
//! impl MemoryInfo for ExamplePlatform {
//!   fn prioritize_32_bit_memory() -> bool {
//!     true
//!   }
//! }
//!
//! impl CpuInfo for ExamplePlatform {
//!   #[cfg(target_arch = "aarch64")]
//!   fn gic_bases() -> GicBases {
//!     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
//!     /// SAFETY: Access to these registers is exclusive to this struct instance.
//!     unsafe { GicBases::new(0x0, 0x0) }
//!   }
//! }
//!
//! impl PlatformInfo for ExamplePlatform {
//!   type MemoryInfo = Self;
//!   type CpuInfo = Self;
//!   type ComponentInfo = Self;
//!   type Extractor = NullSectionExtractor;
//! }
//!
//! static CORE: Core<ExamplePlatform> = Core::new(NullSectionExtractor);
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
mod component_dispatcher;
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
pub use component_dispatcher::MockComponentInfo;
pub use component_dispatcher::{Add, Component, ComponentInfo, Config, Service};
use spin::Once;

#[cfg(test)]
#[macro_use]
#[coverage(off)]
pub mod test_support;

use core::{
    ffi::c_void,
    num::NonZeroUsize,
    ptr::{self, NonNull},
    str::FromStr,
};

use alloc::boxed::Box;
use gcd::SpinLockedGcd;
use memory_manager::CoreMemoryManager;
use mu_rust_helpers::{function, guid::CALLER_ID};
use patina::{
    boot_services::StandardBootServices,
    component::IntoComponent,
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

use crate::{
    component_dispatcher::ComponentDispatcher, config_tables::memory_attributes_table, perf_timer::PerfTimer,
    tpl_mutex::TplMutex,
};

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

/// A trait to be implemented by the platform to provide configuration values and types related to memory management
/// to be used directly by the Patina DXE Core.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct ExamplePlatform;
///
/// impl MemoryInfo for ExamplePlatform {
///   fn prioritize_32_bit_memory() -> bool {
///     true
///   }
/// }
#[cfg_attr(test, mockall::automock)]
pub trait MemoryInfo {
    /// Informs the core that it should prioritize allocating 32-bit memory when not otherwise specified.
    ///
    /// This should only be used as a workaround in environments where address width bugs exist in uncontrollable
    /// dependent software. For example, when booting to an OS that erroneously stores UEFI allocated addresses
    /// greater than 32-bits in a 32-bit variable.
    #[inline(always)]
    fn prioritize_32_bit_memory() -> bool {
        false
    }
}

/// A trait to be implemented by the platform to provide configuration values and types related to the CPU.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct ExamplePlatform;
///
/// impl CpuInfo for ExamplePlatform {
///
///   #[cfg(target_arch = "aarch64")]
///   fn gic_bases() -> GicBases {
///     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
///     /// SAFETY: Access to these registers is exclusive to this struct instance.
///     unsafe { GicBases::new(0x1E000000, 0x1E010000) }
///   }
/// }
/// ```
#[cfg_attr(test, mockall::automock)]
pub trait CpuInfo {
    /// Informs the core of the GIC base addresses for AARCH64 systems.
    #[cfg(target_arch = "aarch64")]
    fn gic_bases() -> GicBases;

    /// Returns the performance timer frequency for the platform.
    ///
    /// By default, this returns `None`, indicating that the core should attempt to determine the frequency
    /// automatically using cpu architecture-specific methods.
    #[inline(always)]
    fn perf_timer_frequency() -> Option<u64> {
        None
    }
}

/// A trait to be implemented by the platform to provide configuration values and types to be used directly by the
/// Patina DXE Core.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct ExamplePlatform;
///
/// // An example of using all default implementations.
/// impl PlatformInfo for ExamplePlatform {
///   type MemoryInfo = Self;
///   type CpuInfo = Self;
///   type ComponentInfo = Self;
///   type Extractor = patina_ffs_extractors::NullSectionExtractor;
/// }
///
/// impl ComponentInfo for ExamplePlatform {}
/// impl MemoryInfo for ExamplePlatform {}
/// impl CpuInfo for ExamplePlatform {}
/// ```
#[cfg_attr(test, mockall::automock(
    type Extractor = patina_ffs_extractors::NullSectionExtractor;
    type ComponentInfo = MockComponentInfo;
    type MemoryInfo = MockMemoryInfo;
    type CpuInfo = MockCpuInfo;
))]
pub trait PlatformInfo {
    /// The platform's memory information and configuration.
    type MemoryInfo: MemoryInfo;

    /// The platform's CPU information and configuration.
    type CpuInfo: CpuInfo;

    /// The platform's component information and configuration.
    type ComponentInfo: ComponentInfo;

    /// The platform's section extractor type, used when extracting sections from firmware volumes.
    type Extractor: SectionExtractor;
}

/// A configuration struct containing the GIC bases (gic_d, gic_r) for AARCH64 systems.
///
/// ## Invariants
///
/// - `self.0` (GIC Distributor Base) points to the GIC Distributor register space.
/// - `self.1` (GIC Redistributor Base) points to the GIC Redistributor register space.
/// - Access to these registers are exclusive to this GicBases instance.
///
/// ## Example
///
/// ```rust
/// use patina_dxe_core::*;
///
/// struct PlatformConfig;
/// # impl ComponentInfo for PlatformConfig {}
/// # impl MemoryInfo for PlatformConfig {}
/// # impl CpuInfo for PlatformConfig {}
///
/// impl PlatformInfo for PlatformConfig {
///   # type MemoryInfo = Self;
///   # type Extractor = patina_ffs_extractors::NullSectionExtractor;
///   # type ComponentInfo = Self;
///   # type CpuInfo = Self;
///
///   # #[cfg(target_arch = "aarch64")]
///   fn gic_bases() -> GicBases {
///     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
///     /// SAFETY: Access to these registers is exclusive to this struct instance.
///     unsafe { GicBases::new(0x1E000000, 0x1E010000) }
///   }
/// }
/// ```
#[derive(Debug, PartialEq)]
pub struct GicBases {
    /// The GIC Distributor base address.
    pub(crate) gicd: u64,
    /// The GIC Redistributor base address.
    pub(crate) gicr: u64,
}

impl GicBases {
    /// Creates a new instance of the GicBases struct with the provided GIC Distributor and Redistributor base addresses.
    ///
    /// ## Safety
    ///
    /// `gicd_base` must point to the GIC Distributor register space.
    ///
    /// `gicr_base` must point to the GIC Redistributor register space.
    ///
    /// Access to these registers are exclusive to this GicBases instance.
    ///
    /// Caller must guarantee that access to these registers is exclusive to this GicBases instance.
    #[coverage(off)]
    pub unsafe fn new(gicd_base: u64, gicr_base: u64) -> Self {
        GicBases { gicd: gicd_base, gicr: gicr_base }
    }
}

/// Static reference to the DXE Core instance in the compiled binary.
///
/// This is set during the `entry_point` call of the DXE Core and is used to provide static access to the core for use
/// only in efiapi functions where no reference to the core is otherwise available.
static __SELF: Once<NonZeroUsize> = Once::new();

/// Platform configured DXE Core responsible for the DXE phase of UEFI booting.
///
/// This struct is generic over the [PlatformInfo] trait, which is used to provide platform-specific configuration to
/// the core. The [PlatformInfo] trait is composed of multiple sub-traits that configure the different subsystems of
/// the Patina DXE Core. Review the [PlatformInfo] trait documentation and each type alias within the trait for more
/// information on the different configurations available to the platform.
///
/// To properly use this struct, the platform must implement the [PlatformInfo] on a type and then create a static
/// instance of the [Core] struct with the platform types as generic parameters (See example below). From there, simply
/// call the [entry_point](Core::entry_point) method within the main function to start the DXE Core.
///
/// ## Examples
///
/// ```rust
/// # use core::ffi::c_void;
/// # use patina_dxe_core::*;
/// # use patina_ffs_extractors::NullSectionExtractor;
/// # #[derive(patina::component::IntoComponent, Default)]
/// # struct ExampleComponent;
/// # impl ExampleComponent {
/// #     fn entry_point(self) -> patina::error::Result<()> { Ok(()) }
/// # }
/// struct ExamplePlatform;
///
/// impl ComponentInfo for ExamplePlatform {
///   fn configs(mut add: Add<Config>) {
///     add.config(32u32);
///     add.config(true);
///   }
///
///   fn components(mut add: Add<Component>) {
///     add.component(ExampleComponent::default());
///   }
/// }
///
/// impl MemoryInfo for ExamplePlatform {
///   fn prioritize_32_bit_memory() -> bool { true }
/// }
///
/// impl CpuInfo for ExamplePlatform {
///   #[cfg(target_arch = "aarch64")]
///   fn gic_bases() -> GicBases {
///     /// SAFETY: gicd and gicr bases correctly point to the register spaces.
///     /// SAFETY: Access to these registers is exclusive to this struct instance.
///     unsafe { GicBases::new(0x0, 0x0) }
///   }
/// }
///
/// impl PlatformInfo for ExamplePlatform {
///   type MemoryInfo = Self;
///   type CpuInfo = Self;
///   type ComponentInfo = Self;
///   type Extractor = NullSectionExtractor;
/// }
///
/// static CORE: Core<ExamplePlatform> = Core::new(NullSectionExtractor);
/// ```
pub struct Core<P: PlatformInfo> {
    /// A parsed and heap-allocated list of HOBs provided by [Self::entry_point].
    hob_list: Once<HobList<'static>>,
    /// The subsystem responsible for data management and dispatch of Patina components.
    component_dispatcher: TplMutex<ComponentDispatcher>,
    // NOTE: This field will be moved into the `DISPATCHER_CONTEXT` when it is moved into the Core.
    section_extractor: P::Extractor,
}

#[coverage(off)]
impl<P: PlatformInfo> Core<P> {
    /// Creates a new instance of the DXE Core in the NoAlloc phase.
    pub const fn new(section_extractor: P::Extractor) -> Self {
        Self { component_dispatcher: ComponentDispatcher::new_locked(), hob_list: Once::new(), section_extractor }
    }

    /// Sets the static DXE Core instance for global access.
    ///
    /// Returns true if the address of self is the same as the stored address, false otherwise.
    #[must_use]
    fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        &physical_address == __SELF.call_once(|| physical_address)
    }

    /// Gets the static DXE Core instance for global access.
    ///
    /// This should only be used in efiapi functions where no reference to the core is otherwise available.
    #[allow(unused)]
    pub(crate) fn instance<'a>() -> &'a Self {
        // SAFETY: The pointer is guaranteed to be set to a valid reference of this `Self` implementation as the atomic
        // compare_exchange guarantees only one initialization has occurred. If the pointer was already set during the
        // `set_instance` call of another `CORE` it would have returned a failure and then panicked before reaching this point.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(*__SELF.get().expect("DXE Core is already initialized.")).as_ref()
        }
    }

    /// The entry point for the Patina DXE Core.
    pub fn entry_point(&'static self, physical_hob_list: *const c_void) -> ! {
        assert!(self.set_instance(), "DXE Core instance was already set!");

        self.init_memory(physical_hob_list);

        if let Err(err) = self.start_dispatcher(physical_hob_list) {
            log::error!("DXE Core failed to start: {err:?}");
        }

        call_bds();
    }

    /// Attempts to set the HOB list for the DXE Core.
    ///
    /// Returns an `EfiError::AlreadyStarted` if the HOB list has already been set.
    fn set_hob_list(&self, hob_list: HobList<'static>) -> Result<&HobList<'static>> {
        match self.hob_list.is_completed() {
            true => Err(error::EfiError::AlreadyStarted),
            false => Ok(self.hob_list.call_once(|| hob_list)),
        }
    }

    /// Returns a reference to the HOB list.
    ///
    /// Must not be called until `set_hob_list` has been called successfully.
    fn hob_list(&self) -> &HobList<'static> {
        self.hob_list.get().expect("HOB list should have been initialized already.")
    }

    /// Initializes the core with the given configuration, including GCD initialization, enabling allocations.
    fn init_memory(&self, physical_hob_list: *const c_void) {
        log::info!("DXE Core Crate v{}", env!("CARGO_PKG_VERSION"));

        GCD.prioritize_32_bit_memory(P::MemoryInfo::prioritize_32_bit_memory());

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
        let mut hob_list = HobList::new();
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
        assert!(self.set_hob_list(hob_list).is_ok());

        // Add custom monitor commands to the debugger before initializing so that
        // they are available in the initial breakpoint.
        patina_debugger::add_monitor_command("gcd", "Prints the GCD", |_, out| {
            let _ = write!(out, "GCD -\n{GCD}");
        });

        // Initialize the debugger if it is enabled.
        patina_debugger::initialize(&mut interrupt_manager);

        log::info!("GCD - After memory init:\n{GCD}");

        let mut component_dispatcher = self.component_dispatcher.lock();
        component_dispatcher.add_service(cpu);
        component_dispatcher.add_service(interrupt_manager);
        component_dispatcher.add_service(CoreMemoryManager);
        component_dispatcher.add_service(PerfTimer::with_frequency(P::CpuInfo::perf_timer_frequency().unwrap_or(0)));
    }

    /// Performs a combined dispatch of Patina components and UEFI drivers.
    ///
    /// This function will continue to loop and perform dispatching until no components have been dispatched in a full
    /// iteration. The dispatching process involves a loop of two distinct dispatch phases:
    ///
    /// 1. A single iteration of dispatching Patina components, retaining those that were not dispatched.
    /// 2. A single iteration of dispatching UEFI drivers via the dispatcher module.
    fn core_dispatcher(&self) -> Result<()> {
        perf_function_begin(function!(), &CALLER_ID, create_performance_measurement);
        loop {
            // Patina component dispatch
            let dispatched = self.component_dispatcher.lock().dispatch();

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

    /// Returns the length of the HOB list.
    /// Clippy gets unhappy if we call get_c_hob_list_size directly, because it gets confused, thinking
    /// get_c_hob_list_size is not marked unsafe, but it is
    fn get_hob_list_len(hob_list: *const c_void) -> usize {
        unsafe { get_c_hob_list_size(hob_list) }
    }

    fn initialize_system_table(&self, physical_hob_list: *const c_void) -> Result<()> {
        let hob_list_slice = unsafe {
            core::slice::from_raw_parts(physical_hob_list as *const u8, Self::get_hob_list_len(physical_hob_list))
        };
        let relocated_c_hob_list = hob_list_slice.to_vec().into_boxed_slice();

        // Instantiate system table.
        systemtables::init_system_table();

        let mut st = systemtables::SYSTEM_TABLE.lock();
        let st = st.as_mut().expect("System Table not initialized!");

        allocator::install_memory_services(st.boot_services_mut());
        gcd::init_paging(self.hob_list());
        events::init_events_support(st.boot_services_mut());
        protocols::init_protocol_support(st.boot_services_mut());
        misc_boot_services::init_misc_boot_services_support(st.boot_services_mut());
        config_tables::init_config_tables_support(st.boot_services_mut());
        runtime::init_runtime_support(st.runtime_services_mut());
        image::init_image_support(self.hob_list(), st);
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

        memory_attributes_table::init_memory_attributes_table_support();

        self.component_dispatcher.lock().set_boot_services(StandardBootServices::new(st.boot_services()));
        self.component_dispatcher.lock().set_runtime_services(StandardRuntimeServices::new(st.runtime_services()));

        Ok(())
    }

    /// Registers platform provided components, configurations, and services.
    #[inline(always)]
    fn apply_component_info(&self) {
        self.component_dispatcher.lock().apply_component_info::<P::ComponentInfo>();
    }

    /// Registers core provided components
    #[allow(clippy::default_constructed_unit_structs)]
    fn add_core_components(&self) {
        let mut dispatcher = self.component_dispatcher.lock();
        dispatcher.insert_component(0, decompress::DecompressProtocolInstaller::default().into_component());
        dispatcher.insert_component(0, systemtables::SystemTableChecksumInstaller::default().into_component());
        dispatcher.insert_component(0, cpu_arch_protocol::CpuArchProtocolInstaller::default().into_component());
        #[cfg(all(target_os = "uefi", target_arch = "aarch64"))]
        dispatcher.insert_component(
            0,
            hw_interrupt_protocol::HwInterruptProtocolInstaller::new(P::CpuInfo::gic_bases()).into_component(),
        );
    }

    /// Starts the core, dispatching all drivers.
    fn start_dispatcher(&'static self, physical_hob_list: *const c_void) -> Result<()> {
        log::info!("Registering platform components");
        self.apply_component_info();
        log::info!("Finished.");

        log::info!("Registering default components");
        self.add_core_components();
        log::info!("Finished.");

        log::info!("Initializing System Table");
        self.initialize_system_table(physical_hob_list)?;
        log::info!("Finished.");

        log::info!("Parsing HOB list for Guided HOBs.");
        self.component_dispatcher.lock().insert_hobs(self.hob_list());
        log::info!("Finished.");

        dispatcher::register_section_extractor(&self.section_extractor);
        fv::register_section_extractor(&self.section_extractor);

        log::info!("Parsing FVs from FV HOBs");
        fv::parse_hob_fvs(self.hob_list())?;
        log::info!("Finished.");

        log::info!("Dispatching Drivers");
        self.core_dispatcher()?;
        self.component_dispatcher.lock().lock_configs();
        self.core_dispatcher()?;
        log::info!("Finished Dispatching Drivers");

        self.component_dispatcher.lock().display_not_dispatched();

        core_display_missing_arch_protocols();

        dispatcher::display_discovered_not_dispatched();

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

fn call_bds() -> ! {
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

    match protocols::PROTOCOL_DB.locate_protocol(bds::PROTOCOL_GUID) {
        Ok(bds) => {
            let bds = bds as *mut bds::Protocol;
            // SAFETY: The BDS arch protocol is the valid C structure as defined by the UEFI specification. The entry
            // field of the protocol is a valid function pointer that conforms to the expected calling convention.
            unsafe {
                ((*bds).entry)(bds);
            }
        }
        Err(err) => log::error!("Unable to locate BDS arch protocol: {err:?}"),
    };

    unreachable!("BDS arch protocol should be found and should never return.");
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_cannot_set_instance_twice() {
        static CORE: Core<MockPlatformInfo> =
            Core::<MockPlatformInfo>::new(patina_ffs_extractors::NullSectionExtractor);
        static CORE2: Core<MockPlatformInfo> =
            Core::<MockPlatformInfo>::new(patina_ffs_extractors::NullSectionExtractor);
        assert!(CORE.set_instance());

        if NonNull::from_ref(&CORE) != NonNull::from_ref(Core::<MockPlatformInfo>::instance()) {
            panic!("CORE instance mismatch");
        }

        // We return true because its the same address
        assert!(CORE.set_instance());
        // This should fail because CORE2 is a different instance
        assert!(!CORE2.set_instance());

        if NonNull::from_ref(&CORE) != NonNull::from_ref(Core::<MockPlatformInfo>::instance()) {
            panic!("CORE instance mismatch after second set_instance");
        }
    }

    #[test]
    fn test_trait_defaults_do_not_change() {
        /// A simple test to acknowledge that the default implementations of the trait default implementations
        /// should not change without a conscious decision, which requires updating this test.
        struct TestPlatform;

        impl PlatformInfo for TestPlatform {
            type MemoryInfo = TestPlatform;
            type CpuInfo = TestPlatform;
            type ComponentInfo = TestPlatform;
            type Extractor = patina_ffs_extractors::NullSectionExtractor;
        }

        impl MemoryInfo for TestPlatform {}

        impl ComponentInfo for TestPlatform {}

        impl CpuInfo for TestPlatform {
            #[cfg(target_arch = "aarch64")]
            fn gic_bases() -> GicBases {
                GicBases { gicd: 0, gicr: 0 }
            }
        }

        assert!(!<TestPlatform as PlatformInfo>::MemoryInfo::prioritize_32_bit_memory());
        assert!(<<TestPlatform as PlatformInfo>::CpuInfo>::perf_timer_frequency().is_none());
    }
}
