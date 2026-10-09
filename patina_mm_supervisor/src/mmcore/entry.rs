//! `MmSupervisorCore` Entry Phase
//!
//! Construction of the supervisor instance and the MM entry point every core arrives through.
//!
//! [`entry_point`](MmSupervisorCore::entry_point) is the only place an error from any later phase
//! becomes a fail-stop: the MM entry point has no caller that could handle one.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use patina::management_mode::supervisor::UserCommandType;
use patina::pi::hob::PhaseHandoffInformationTable;
use spin::Mutex;

use super::CoreInitError;
use super::{cpu::CpuManager, mailbox::MailboxManager, smrr::smrr_enable};
use crate::{
    MmSupervisorCore, PlatformInfo,
    error::{MmSupervisorError, MmSupervisorResult},
    intrinsics::{current_apic_id, is_bsp},
    privilege_mgmt::invoke_demoted_routine,
    privilege_mgmt::syscall_setup::SyscallInterface,
    state::init_state,
};

/// Checks if a specific core has completed initialization.
///
/// Reads the 1-byte slot at `mm_initialized_buffer + cpu_index`.
/// A non-zero value indicates the core has completed initialization.
///
/// Returns [`CoreInitError::InitializedBufferUnavailable`] when the initialized buffer has not been
/// published yet, and [`CoreInitError::CpuIndexOutOfRange`] when `cpu_index` is outside it. Callers
/// that need a definitive answer should handle those cases explicitly and pick their own fallback,
/// typically failing closed by treating the core as uninitialized.
fn is_core_initialized(cpu_index: usize) -> MmSupervisorResult<bool> {
    let buffer = init_state().mm_initialized_buffer().ok_or(CoreInitError::InitializedBufferUnavailable)?;

    let slot =
        buffer.get(cpu_index).ok_or(CoreInitError::CpuIndexOutOfRange { index: cpu_index, len: buffer.len() })?;

    Ok(slot.load(Ordering::Acquire) != 0)
}

/// Marks a specific core as initialized.
///
/// Writes a non-zero value to the 1-byte slot at `mm_initialized_buffer + cpu_index`.
///
/// Returns [`CoreInitError::InitializedBufferUnavailable`] when the initialized buffer has not been
/// published yet, and [`CoreInitError::CpuIndexOutOfRange`] when `cpu_index` is outside it. In
/// either case the core's slot is left unwritten, so the caller must not treat the core as
/// initialized.
fn mark_core_initialized(cpu_index: usize) -> MmSupervisorResult<()> {
    let buffer = init_state().mm_initialized_buffer().ok_or(CoreInitError::InitializedBufferUnavailable)?;

    let slot =
        buffer.get(cpu_index).ok_or(CoreInitError::CpuIndexOutOfRange { index: cpu_index, len: buffer.len() })?;

    slot.store(1, Ordering::Release);
    Ok(())
}

/// Returns whether `cpu_index` has already completed initialization, reading an unpublished
/// buffer as a first entry rather than as a failure.
///
/// The buffer is published part way through BSP initialization, so every core that arrives in the
/// initializing MMI finds nothing to read: the BSP before it publishes the buffer, and any AP that
/// reaches the check before the BSP gets there. None of them can have been initialized, which is
/// exactly what a missing buffer means here. The buffer is never withdrawn once published, so a
/// later entry cannot reach this case, and a bad index is still reported.
fn core_already_initialized(cpu_index: usize) -> MmSupervisorResult<bool> {
    match is_core_initialized(cpu_index) {
        Err(MmSupervisorError::CoreInit(CoreInitError::InitializedBufferUnavailable)) => Ok(false),
        result => result,
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
    pub(crate) fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        let stored = init_state().set_supervisor(physical_address);
        if stored {
            init_state().set_processor_id_lookup_fn(Self::processor_id_by_index);
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
    /// - Any initialization stage reports an error
    ///
    /// The last case is what the internal entry point now returns instead of halting on its own.
    /// This wrapper is the single place that turns those errors into a fail-stop, because the MM
    /// entry point has no caller that could handle one.
    ///
    /// ## Safety
    ///
    /// This function is unsafe because it is called from the MM entry point and assumes the environment
    /// is properly set up. The function will perform basic sanity checks against the incoming parameters
    /// but does not validate the entire system state.
    /// `hob_list` must be valid during BSP initialization. It is not accessed on runtime entries.
    pub unsafe fn entry_point(&'static self, cpu_index: usize, hob_list: *const c_void) {
        // This wrapper ensures that any error from the internal entry point
        // results in a panic.
        // SAFETY: Same safety guarantees as `entry_point` apply.
        if let Err(err) = unsafe { self.entry_point_internal(cpu_index, hob_list) } {
            panic!("Failed to enter MM Supervisor Core internal entry point: {err:?}");
        }
    }

    /// Internal entry point for the MM Supervisor Core.
    ///
    /// Returns `Ok(())` on successful entry.
    ///
    /// # Errors
    ///
    /// Reports the first initialization stage that fails rather than halting, so
    /// [`entry_point`](Self::entry_point) owns the decision to fail-stop. A core whose index has
    /// no slot is reported through [`CoreInitError`]; the BSP additionally propagates everything
    /// `bsp_init` can report.
    ///
    /// # Safety
    ///
    /// This function is unsafe for the same reasons as `entry_point`. It assumes the environment
    /// is properly set up and performs basic sanity checks against the incoming parameters, but
    /// does not validate the entire system state.
    unsafe fn entry_point_internal(&'static self, cpu_index: usize, hob_list: *const c_void) -> MmSupervisorResult<()> {
        // Get the current CPU's APIC ID, EBX[31:24] contains the initial APIC ID
        let cpu_id = current_apic_id();

        // Determine if we're BSP by checking IA32_APIC_BASE MSR
        let is_bsp = is_bsp();

        log::trace!("CPU {cpu_id} (index {cpu_index}) entering MM Supervisor Core (BSP: {is_bsp})");
        let core_initialized = core_already_initialized(cpu_index)?;

        // Check if this core has already completed initialization (per-core check)
        if core_initialized {
            // Subsequent entry: go directly to request loop or holding pen (does not return)
            log::trace!("CPU {cpu_id} (index {cpu_index}) re-entering MM Supervisor Core, skipping initialization.");
            smrr_enable();
            if is_bsp {
                self.free_init_module(init_state());
            }
            self.enter_runtime(cpu_id, cpu_index);

            return Ok(());
        }

        let _init_guard = self.init_lock.lock();

        // First entry: initialization phase
        if is_bsp {
            // BSP path: Initialize the supervisor
            assert!(self.set_instance(), "MM Supervisor Core instance was already set!");
            assert!(!hob_list.is_null(), "MM Supervisor Core requires a non-null HOB list pointer.");

            log::info!("MM Supervisor Core v{}", env!("CARGO_PKG_VERSION"));
            log::info!("BSP (CPU {cpu_id}, index {cpu_index}) starting one-time initialization...");

            self.cpu_manager.register_cpu(cpu_id, cpu_index, true)?;

            // Perform BSP-only one-time initialization.
            // SAFETY: `hob_list` is provided by the MM IPL and is guaranteed to
            // be a valid HOB list (the caller asserts it is non-null before
            // dispatching).
            let hob_hand_off_table = unsafe { hob_list.cast::<PhaseHandoffInformationTable>().as_ref_unchecked() };
            let user_hob_list = self.bsp_init(hob_hand_off_table)?;

            // Dispatch to the user level entry point discovered from the HOB list (if found)
            let user_entry = init_state()
                .user_entry_point()
                .filter(|&entry| entry != 0)
                .ok_or(CoreInitError::UserEntryPointMissing)?;

            let cpl3_stack = self.syscall_interface.get_cpl3_stack(cpu_index)?;

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
                    user_hob_list,
                    0,
                )
            };
            log::info!("Returned from user entry point with value: 0x{ret:016x}");

            // Mark BSP init as complete so APs can proceed
            self.initialized.store(true, Ordering::Release);
            init_state().mark_bsp_init_complete();

            log::info!("BSP initialization complete.");
        } else {
            // AP path: Wait for BSP to complete one-time initialization
            log::info!("AP (CPU {cpu_id}, index {cpu_index}) waiting for BSP initialization...");

            // Spin until BSP completes initialization
            while !init_state().is_bsp_init_complete() {
                core::hint::spin_loop();
            }

            self.cpu_manager.register_cpu(cpu_id, cpu_index, false)?;
        }

        // All cores perform per-core initialization
        self.per_core_init(cpu_id, is_bsp)?;

        // Mark this core as initialized in the per-core buffer
        mark_core_initialized(cpu_index)?;

        // Track that this core has completed per-core init
        let init_count = init_state().inc_per_core_init_count();
        log::info!("CPU {cpu_id} (index {cpu_index}) completed per-core init ({init_count} cores initialized)");

        // BSP waits for all registered CPUs to complete per-core init before returning
        if is_bsp {
            let expected_cpus = self.cpu_manager.registered_count();
            while init_state().per_core_init_count() < expected_cpus as u32 {
                core::hint::spin_loop();
            }

            log::info!("All {expected_cpus} cores completed initialization, returning to caller.");
        }

        Ok(())

        // First entry returns to caller after init is complete
        // (Each core has already marked itself as initialized via mark_core_initialized)
    }

    /// Get the CPU manager.
    pub fn cpu_manager(&self) -> &CpuManager<MAX_CPUS> {
        &self.cpu_manager
    }

    /// Returns the UEFI processor ID registered at a dense CPU index.
    fn processor_id_by_index(cpu_index: usize) -> Option<u64> {
        Self::instance().cpu_manager.get_cpu_id_by_index(cpu_index).map(u64::from)
    }

    /// Get the mailbox manager.
    pub fn mailbox_manager(&self) -> &MailboxManager<MAX_CPUS> {
        &self.mailbox_manager
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use core::sync::atomic::{AtomicU8, Ordering};

    use serial_test::serial;

    use super::super::CoreInitError;
    use super::{core_already_initialized, is_core_initialized, mark_core_initialized};
    use crate::state::init_state;
    use crate::{MmSupervisorCore, PlatformInfo};

    struct TestPlatform;

    impl PlatformInfo for TestPlatform {}

    #[test]
    fn test_supervisor_creation_and_accessors() {
        let supervisor: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();

        assert_eq!(supervisor.cpu_manager().max_cpus(), 4);
        assert_eq!(supervisor.cpu_manager().registered_count(), 0);
        assert_eq!(supervisor.mailbox_manager().check_mailbox(0), None);
        assert_eq!(supervisor.mailbox_manager().check_mailbox(4), None);
        assert!(!supervisor.initialized.load(Ordering::Relaxed));
    }

    #[test]
    fn test_supervisor_is_const() {
        static _SUPERVISOR: MmSupervisorCore<TestPlatform, 4> = MmSupervisorCore::new();
    }

    #[test]
    #[serial]
    fn test_core_initialization_functions_handle_state_values_and_bounds() {
        static SLOTS: [AtomicU8; 2] = [AtomicU8::new(0), AtomicU8::new(0)];

        assert!(init_state().mm_initialized_buffer().is_none());
        assert_eq!(is_core_initialized(0), Err(CoreInitError::InitializedBufferUnavailable.into()));
        // The entry point reads the same missing buffer as a first entry, because nothing can have
        // been initialized before the buffer that records it exists.
        assert_eq!(core_already_initialized(0), Ok(false));
        // Before the buffer is published the mark is reported rather than silently dropped.
        assert_eq!(mark_core_initialized(0), Err(CoreInitError::InitializedBufferUnavailable.into()));

        init_state().set_mm_initialized_buffer(&SLOTS);
        assert_eq!(is_core_initialized(0), Ok(false));
        assert_eq!(is_core_initialized(1), Ok(false));

        SLOTS[1].store(0xFF, Ordering::Relaxed);
        assert_eq!(is_core_initialized(1), Ok(true));
        SLOTS[1].store(0, Ordering::Relaxed);

        assert_eq!(mark_core_initialized(1), Ok(()));
        assert_eq!(is_core_initialized(0), Ok(false));
        assert_eq!(is_core_initialized(1), Ok(true));

        // An out-of-range mark fails instead of corrupting an adjacent slot.
        assert_eq!(
            mark_core_initialized(SLOTS.len()),
            Err(CoreInitError::CpuIndexOutOfRange { index: SLOTS.len(), len: SLOTS.len() }.into())
        );
        assert_eq!(
            is_core_initialized(SLOTS.len()),
            Err(CoreInitError::CpuIndexOutOfRange { index: SLOTS.len(), len: SLOTS.len() }.into())
        );
        // A bad index is a real fault and stays a fault, unlike an absent buffer.
        assert_eq!(
            core_already_initialized(SLOTS.len()),
            Err(CoreInitError::CpuIndexOutOfRange { index: SLOTS.len(), len: SLOTS.len() }.into())
        );
        assert_eq!(is_core_initialized(0), Ok(false));
        assert_eq!(is_core_initialized(1), Ok(true));
        assert_eq!(core_already_initialized(1), Ok(true));
    }
}
