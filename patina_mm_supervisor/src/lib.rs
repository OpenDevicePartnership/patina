//! MM Supervisor Core
//!
//! A pure Rust implementation of the MM Supervisor Core for standalone MM mode environments.
//!
//! This crate provides the core functionality for running a supervisor in MM (Management Mode)
//! that orchestrates incoming requests on the BSP while APs wait in a holding pen.
//!
//! ## Architecture
//!
//! The entry point is executed on all cores:
//! - **BSP**: Performs one-time initialization and enters the request serving loop
//! - **APs**: Enter a holding pen and poll mailboxes for commands from BSP
//!
//! ## Memory Model
//!
//! This is a core component that manages its own memory. It does **not** use heap allocation.
//! All structures use fixed-size arrays with compile-time constants provided via const generics.
//!
//! ## Examples
//!
//! ```rust,no_run
//! # #[cfg(target_arch = "x86_64")]
//! # mod example {
//! use patina_mm_supervisor::*;
//!
//! struct MyPlatform;
//!
//! impl CpuInfo for MyPlatform {
//!     fn ap_poll_timeout_us() -> u64 { 1000 }
//! }
//!
//! impl PlatformInfo for MyPlatform {
//!     type CpuInfo = Self;
//! }
//!
//! // The const generic argument is the maximum CPU count used to size internal arrays.
//! static SUPERVISOR: MmSupervisorCore<MyPlatform, 8> = MmSupervisorCore::new();
//! # }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![cfg(target_arch = "x86_64")]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod cpu;
mod init;
mod mailbox;
mod mem;
mod mm_policy;
mod perf_timer;
mod privilege_mgmt;
mod runtime;
mod save_state;
mod state;
mod supervisor_handlers;

use cpu::{CpuManager, get_current_cpu_id, is_bsp};
use mailbox::MailboxManager;
// Re-exported for use by descendant modules via `crate::` paths.
use mem::{AllocationType, SharedPagingAllocator};

use privilege_mgmt::{invoke_demoted_routine, syscall_setup::SyscallInterface};

use spin::Mutex;

use patina::management_mode::supervisor::UserCommandType;

use core::{
    ffi::c_void,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};

use patina::{
    base::{UEFI_PAGE_SIZE, align_range},
    management_mode::MmCommBufferStatus,
};
use patina_paging::{MemoryAttributes, PageTable};

use state::{init_state, security_state};

// Public API: traits that platform binaries must implement.
pub use cpu::CpuInfo;

// Publicly re-export the handler types since platform-specific handlers will need to reference these for
// their function signatures and return types.
pub use init::PolicyInitError;
pub use supervisor_handlers::SupervisorMmiHandler;

// The entry-point shim references `rust_main`, which is provided by the platform binary, and is
// only meaningful on the firmware (UEFI) target. Exclude it from host builds (tests, doctests)
// so their harnesses can link.
#[cfg(target_os = "uefi")]
core::arch::global_asm!(include_str!("entry_point.asm"));

/// GUID for `gMmCommonRegionHobGuid`.
///
/// `{ 0xd4ffc718, 0xfb82, 0x4274, { 0x9a, 0xfc, 0xaa, 0x8b, 0x1e, 0xef, 0x52, 0x93 } }`
pub const MM_COMMON_REGION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("d4ffc718-fb82-4274-9afc-aa8b1eef5293");

// GUID for gMmSupervisorPassDownHobGuid
// { 0x3f2d2d1a, 0x7c6a, 0x4e2e, { 0x91, 0x2e, 0x5c, 0x4f, 0x5b, 0x8c, 0x2a, 0x9d } }
/// GUID for the MM Supervisor PassDown HOB.
pub const MM_SUPV_PASS_DOWN_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("3f2d2d1a-7c6a-4e2e-912e-5c4f5b8c2a9d");

// GUID for gMpInformationHobGuid (StandaloneMmPkg/Include/Guid/MpInformation.h)
// { 0xba33f15d, 0x4000, 0x45c1, { 0x8e, 0x88, 0xf9, 0x16, 0x92, 0xd4, 0x57, 0xe3 } }
/// GUID for the MP Information HOB, which carries the processor count and the
/// `EFI_PROCESSOR_INFORMATION` array (APIC IDs) used by the save-state read path.
pub const MP_INFORMATION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("ba33f15d-4000-45c1-8e88-f91692d457e3");

/// MM Supervisor PassDown HOB Revision
pub const MM_SUPV_PASS_DOWN_HOB_REVISION: u32 = 2;

/// Timeout for waiting for APs to arrive in the holding pen (1 second).
const AP_ARRIVAL_TIMEOUT_US: u64 = 1_000_000;

/// Timeout for waiting for an AP to complete a dispatched procedure (10 seconds).
const AP_TIMEOUT_US: u64 = 10_000_000;

/// A trait to be implemented by the platform to provide configuration values and types to be used
/// by the MM Supervisor Core.
///
/// ## Examples
///
/// ```rust,no_run
/// # #[cfg(target_arch = "x86_64")]
/// # mod example {
/// use patina_mm_supervisor::*;
///
/// struct ExamplePlatform;
///
/// impl CpuInfo for ExamplePlatform {
///     fn ap_poll_timeout_us() -> u64 { 1000 }
/// }
///
/// impl PlatformInfo for ExamplePlatform {
///     type CpuInfo = Self;
/// }
/// # }
/// ```
pub trait PlatformInfo: 'static {
    /// The platform's CPU information and configuration.
    type CpuInfo: CpuInfo;

    /// Returns the platform-specific supervisor MMI handlers.
    ///
    /// The supervisor dispatch loop iterates the core's built-in handlers first and then
    /// the handlers returned here, so platforms can register additional handlers (for
    /// example platform-specific or test handlers) without modifying the core.
    ///
    /// The default implementation returns an empty slice.
    fn mmi_handlers() -> &'static [SupervisorMmiHandler] {
        &[]
    }
}

/// Result of a page table ownership query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageOwnership {
    /// The page is user-accessible (U/S = 1, SpecialPurpose clear).
    User,
    /// The page is supervisor-only (U/S = 0, SpecialPurpose set).
    Supervisor,
}

/// Queries the page table to determine the ownership (user vs supervisor) of an address.
///
/// The address and size are page-aligned before querying (rounded down / up respectively).
///
/// Checks the `Supervisor` attribute which maps to the U/S bit on X64:
///   - `Supervisor` set  => `PageOwnership::Supervisor` (U/S = 0)
///   - `Supervisor` clear => `PageOwnership::User` (U/S = 1)
///
/// Returns `None` if the page table is not initialized or the address is unmapped.
pub(crate) fn query_address_ownership(address: u64, size: u64) -> Option<PageOwnership> {
    let (aligned_addr, aligned_size) = align_range(address, size, UEFI_PAGE_SIZE as u64).ok()?;
    let page_table = security_state().lock_page_table();
    let pt = page_table.as_ref()?;
    let attrs = pt.query_memory_region(aligned_addr, aligned_size).ok()?;
    log::trace!(
        "Queried page ownership for address range 0x{:016x}-0x{:016x}: attributes={:?}",
        aligned_addr,
        aligned_addr + aligned_size,
        attrs
    );
    if attrs.contains(MemoryAttributes::Supervisor) {
        Some(PageOwnership::Supervisor)
    } else {
        Some(PageOwnership::User)
    }
}

/// Communication buffer configuration extracted from PassDown HOB.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommBufferConfig {
    /// MM Supervisor communication buffer (external interface).
    pub supv_comm_buffer: u64,
    /// MM Supervisor internal communication buffer.
    pub supv_comm_buffer_internal: u64,
    /// Size of supervisor communication buffer.
    pub supv_comm_buffer_size: u64,
    /// MM User communication buffer (external interface).
    pub user_comm_buffer: u64,
    /// MM User internal communication buffer.
    pub user_comm_buffer_internal: u64,
    /// Size of user communication buffer.
    pub user_comm_buffer_size: u64,
    /// `MmCommBufferStatus` mailbox for user-targeted requests.
    pub user_status_buffer: u64,
    /// `MmCommBufferStatus` mailbox for supervisor-targeted requests.
    pub supv_status_buffer: u64,
    /// MM Supervisor to User buffer.
    pub supv_to_user_buffer: u64,
    /// Size of Supervisor to User buffer.
    pub supv_to_user_buffer_size: u64,
}

/// Request target derived from the user and supervisor `MmCommBufferStatus`
/// mailboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTarget {
    /// No pending request (neither mailbox is valid).
    None,
    /// Request targets the Supervisor.
    Supervisor,
    /// Request targets the User module.
    User,
}

impl RequestTarget {
    /// Selects the dispatch target based on the two parallel status mailboxes.
    ///
    /// The user mailbox is checked first — if its `is_comm_buffer_valid` flag
    /// is set, the request belongs to the user module. Otherwise the
    /// supervisor mailbox is consulted. When neither mailbox is valid the
    /// request is treated as an asynchronous MMI, which is still dispatched
    /// through the user path so the user-core's async handler chain can run.
    pub fn select(user_status: &MmCommBufferStatus, supv_status: &MmCommBufferStatus) -> Self {
        if user_status.is_comm_buffer_valid != 0 {
            RequestTarget::User
        } else if supv_status.is_comm_buffer_valid != 0 {
            RequestTarget::Supervisor
        } else {
            // Async MMI — user-core's async dispatcher runs unconditionally
            // once we demote, so route through the user path.
            RequestTarget::User
        }
    }
}

/// The MM Supervisor Core responsible for managing the standalone MM environment.
///
/// This struct is generic over the [`PlatformInfo`] trait, which provides platform-specific
/// configuration including compile-time constants for array sizes.
///
/// The supervisor manages:
/// - BSP initialization and request handling
/// - AP management through the holding pen and mailbox system
/// - Request dispatching and response handling
///
/// ## Memory Model
///
/// This struct does not perform heap allocation. All internal structures use fixed-size
/// arrays sized by the `MAX_CPUS` const generic parameter.
///
/// ## Usage
///
/// Create a static instance of the supervisor and call `entry_point` from all cores:
///
/// ```rust,no_run
/// # #[cfg(target_arch = "x86_64")]
/// # mod example {
/// use core::ffi::c_void;
/// use patina_mm_supervisor::*;
///
/// struct MyPlatform;
///
/// impl CpuInfo for MyPlatform {
///     fn ap_poll_timeout_us() -> u64 { 1000 }
/// }
///
/// impl PlatformInfo for MyPlatform {
///     type CpuInfo = Self;
/// }
///
/// // The const generic argument is the maximum CPU count used to size internal arrays.
/// static SUPERVISOR: MmSupervisorCore<MyPlatform, 8> = MmSupervisorCore::new();
///
/// // The MM IPL invokes this entry point on every core.
/// pub extern "efiapi" fn mm_entry(cpu_index: usize, hob_list: *const c_void) {
///     // SAFETY: invoked once per core by the MM environment with a valid HOB list.
///     unsafe { SUPERVISOR.entry_point(cpu_index, hob_list) };
/// }
/// # }
/// ```
pub struct MmSupervisorCore<P: PlatformInfo, const MAX_CPUS: usize> {
    /// Manager for CPU-related operations.
    cpu_manager: CpuManager<MAX_CPUS>,
    /// Manager for AP mailboxes.
    mailbox_manager: MailboxManager<MAX_CPUS, P::CpuInfo>,
    /// Syscall interface for privilege transitions.
    syscall_interface: SyscallInterface<MAX_CPUS>,
    /// Flag indicating if the core has been initialized.
    initialized: AtomicBool,
    /// TESTING: serializes per-core initialization so only one core runs it at a time.
    init_lock: Mutex<()>,
    /// Phantom data for the platform type.
    _phantom: core::marker::PhantomData<fn() -> P>,
}

pub(crate) fn is_buffer_inside_mmram(base: u64, size: u64) -> bool {
    // we will go over the page allocator to see if this region falls inside any of the MMRAM regions
    security_state().page_allocator().is_region_inside_mmram(base, size)
}

/// Read CR3 register.
pub(crate) fn read_cr3() -> u64 {
    let mut _value = 0u64;

    #[cfg(all(not(test), target_arch = "x86_64"))]
    {
        // SAFETY: inline asm is inherently unsafe because Rust can't reason about it.
        // In this case we are reading the CR3 register, which is a safe operation.
        unsafe {
            core::arch::asm!("mov {}, cr3", out(reg) _value, options(nostack, preserves_flags));
        }
    }

    _value
}

/// Checks if a specific core has completed initialization.
///
/// Reads the 1-byte slot at `mm_initialized_buffer + cpu_index`.
/// A non-zero value indicates the core has completed initialization.
fn is_core_initialized(cpu_index: usize) -> bool {
    if let Some(buffer_base) = init_state().mm_initialized_buffer() {
        if buffer_base == 0 {
            return false;
        }
        let slot_ptr = (buffer_base as usize + cpu_index) as *const u8;
        // SAFETY: The buffer is provided by the MM IPL and is guaranteed to be valid.
        // Each core only reads its own slot or slots of other cores.
        let value = unsafe { core::ptr::read_volatile(slot_ptr) };
        value != 0
    } else {
        false
    }
}

/// Marks a specific core as initialized.
///
/// Writes a non-zero value to the 1-byte slot at `mm_initialized_buffer + cpu_index`.
fn mark_core_initialized(cpu_index: usize) {
    if let Some(buffer_base) = init_state().mm_initialized_buffer() {
        if buffer_base == 0 {
            log::error!("MM initialized buffer is null, cannot mark core {} as initialized", cpu_index);
            return;
        }
        let slot_ptr = (buffer_base as usize + cpu_index) as *mut u8;
        // SAFETY: The buffer is provided by the MM IPL and is guaranteed to be valid.
        // Each core writes only to its own slot.
        unsafe { core::ptr::write_volatile(slot_ptr, 1) };
        log::trace!("Core {} marked as initialized at 0x{:016x}", cpu_index, slot_ptr as u64);
    } else {
        log::error!("MM initialized buffer not set, cannot mark core {} as initialized", cpu_index);
    }
}

impl<P: PlatformInfo, const MAX_CPUS: usize> MmSupervisorCore<P, MAX_CPUS> {
    /// Creates a new instance of the MM Supervisor Core.
    ///
    /// This is a const fn that performs no heap allocation.
    pub const fn new() -> Self {
        Self {
            cpu_manager: CpuManager::new(),
            mailbox_manager: MailboxManager::new(),
            syscall_interface: SyscallInterface::new(),
            initialized: AtomicBool::new(false),
            init_lock: Mutex::new(()),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Sets the static supervisor instance for global access.
    ///
    /// Returns true if the address was successfully stored, false if already set.
    /// Also registers the type-erased AP startup function pointer.
    #[must_use]
    fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        let stored = init_state().set_supervisor(physical_address);
        if stored {
            // Register the conformed AP startup function for this platform
            init_state().set_ap_startup_fn(Self::start_ap_procedure_trampoline);
        }
        stored
    }

    /// Gets the static MM Supervisor Core instance for global access.
    #[allow(unused)]
    pub(crate) fn instance<'a>() -> &'a Self {
        // SAFETY: The pointer is guaranteed to be valid as set_instance ensures single initialization.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(
                init_state().supervisor().expect("MM Supervisor Core is not initialized."),
            )
            .as_ref()
        }
    }

    /// The entry point for the MM Supervisor Core.
    ///
    /// This function is called on all cores (BSP and APs). The BSP performs initialization
    /// and enters the request serving loop, while APs enter the holding pen.
    ///
    /// On the first call (initialization phase) this function returns after init is complete.
    /// On subsequent calls neither path returns: the BSP enters the request loop and the APs
    /// enter the holding pen.
    ///
    /// ## Panics
    ///
    /// Panics if:
    /// - The supervisor instance was already set
    /// - The HOB list pointer is null
    ///
    /// ## Safety
    ///
    /// This function is unsafe because it is called from the MM entry point and assumes the environment
    /// is properly set up. The function will perform basic sanity checks against the incoming parameters
    /// but does not validate the entire system state.
    pub unsafe fn entry_point(&'static self, cpu_index: usize, hob_list: *const c_void) {
        // Get the current CPU's APIC ID
        let cpu_id = get_current_cpu_id();

        // Determine if we're BSP by checking IA32_APIC_BASE MSR
        let is_bsp = is_bsp();

        log::trace!("CPU {} (index {}) entering MM Supervisor Core (BSP: {})", cpu_id, cpu_index, is_bsp);
        // Check if this core has already completed initialization (per-core check)
        if is_core_initialized(cpu_index) {
            // Subsequent entry: go directly to request loop or holding pen (does not return)
            log::trace!(
                "CPU {} (index {}) re-entering MM Supervisor Core, skipping initialization.",
                cpu_id,
                cpu_index
            );
            self.enter_runtime(cpu_id);

            return;
        }

        let _init_guard = self.init_lock.lock();

        // First entry: initialization phase
        if is_bsp {
            // BSP path: Initialize the supervisor
            assert!(self.set_instance(), "MM Supervisor Core instance was already set!");
            assert!(!hob_list.is_null(), "MM Supervisor Core requires a non-null HOB list pointer.");

            log::trace!("MM Supervisor Core v{}", env!("CARGO_PKG_VERSION"));
            log::trace!("BSP (CPU {}, index {}) starting one-time initialization...", cpu_id, cpu_index);

            // Register BSP with CPU manager
            self.cpu_manager.register_cpu(cpu_id, cpu_index, true);

            // Perform BSP-only one-time initialization (this sets up MM_INITIALIZED_BUFFER)
            self.bsp_init(hob_list);

            // Dispatch to the user level entry point discovered from the HOB list (if found)
            let user_entry = match init_state().user_entry_point() {
                Some(entry) if entry != 0 => entry,
                _ => {
                    log::error!("User entry point not configured, cannot demote");
                    return;
                }
            };

            let cpl3_stack = match self.syscall_interface.get_cpl3_stack(cpu_index) {
                Ok(stack) => stack,
                Err(e) => {
                    log::error!("Failed to get CPL3 stack for CPU {}: {:?}", cpu_index, e);
                    return;
                }
            };

            // SAFETY: We are transitioning from the supervisor (CPL0) to the user module (CPL3) for the first time.
            // The entry point and stack have been validated and set up during initialization, and the user module is
            // will be responsible for validating any further inputs.
            let ret = unsafe {
                invoke_demoted_routine(
                    cpu_index,
                    user_entry,
                    cpl3_stack,
                    3,
                    UserCommandType::StartUserCore as u64,
                    hob_list as u64,
                    0,
                )
            };
            log::trace!("Returned from user entry point with value: 0x{:016x}", ret);

            // Mark BSP init as complete so APs can proceed
            self.initialized.store(true, Ordering::Release);
            init_state().mark_bsp_init_complete();

            log::trace!("BSP one-time initialization complete.");
        } else {
            // AP path: Wait for BSP to complete one-time initialization
            log::trace!("AP (CPU {}, index {}) waiting for BSP initialization...", cpu_id, cpu_index);

            // Spin until BSP completes initialization
            while !init_state().is_bsp_init_complete() {
                core::hint::spin_loop();
            }

            // Register this AP with the CPU manager
            self.cpu_manager.register_cpu(cpu_id, cpu_index, false);
        }

        // All cores perform per-core initialization
        self.per_core_init(cpu_id, is_bsp);

        // Mark this core as initialized in the per-core buffer
        mark_core_initialized(cpu_index);

        // Track that this core has completed per-core init
        let init_count = init_state().inc_per_core_init_count();
        log::trace!("CPU {} (index {}) completed per-core init ({} cores initialized)", cpu_id, cpu_index, init_count);

        // BSP waits for all registered CPUs to complete per-core init before returning
        if is_bsp {
            let expected_cpus = self.cpu_manager.registered_count();
            while init_state().per_core_init_count() < expected_cpus as u32 {
                core::hint::spin_loop();
            }

            log::trace!("All {} cores completed initialization, returning to caller.", expected_cpus);
        }

        // First entry returns to caller after init is complete
        // (Each core has already marked itself as initialized via mark_core_initialized)
    }

    /// Get the CPU manager.
    pub fn cpu_manager(&self) -> &CpuManager<MAX_CPUS> {
        &self.cpu_manager
    }

    /// Get the mailbox manager.
    pub fn mailbox_manager(&self) -> &MailboxManager<MAX_CPUS, P::CpuInfo> {
        &self.mailbox_manager
    }
}

impl<P: PlatformInfo, const MAX_CPUS: usize> Default for MmSupervisorCore<P, MAX_CPUS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlatform;

    impl CpuInfo for TestPlatform {}

    impl PlatformInfo for TestPlatform {
        type CpuInfo = Self;
    }

    #[test]
    fn test_cpu_info_defaults() {
        assert_eq!(<TestPlatform as CpuInfo>::ap_poll_timeout_us(), 1000);
    }

    #[test]
    fn test_supervisor_creation() {
        let _supervisor: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();
        // Just verify it compiles and creates without panic
    }

    #[test]
    fn test_supervisor_is_const() {
        // Verify we can create a static instance (no heap allocation)
        static _SUPERVISOR: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();
    }
}
