//! `MmSupervisorCore` Entry Phase
//!
//! Construction of the supervisor instance and the MM entry point every core
//! arrives through.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use patina::management_mode::supervisor::UserCommandType;
use patina::pi::hob::PhaseHandoffInformationTable;

use super::CoreInitError;
use super::{cpu::CpuManager, mailbox::MailboxManager, smrr::smrr_enable};
use crate::{
    MmSupervisorCore, PlatformInfo,
    error::{MmSupervisorError, MmSupervisorResult},
    intrinsics::{current_apic_id, is_bsp},
    privilege_mgmt::syscall_setup::SyscallInterface,
    privilege_mgmt::{DEMOTED_ROUTINE_ARG_COUNT, invoke_demoted_routine},
    state::init_state,
};

/// Checks if a specific core has completed initialization.
///
/// Reads the 1-byte slot at `mm_initialized_buffer + cpu_index`.
/// A non-zero value indicates the core has completed initialization.
fn is_core_initialized(cpu_index: usize) -> MmSupervisorResult<bool> {
    let buffer = init_state().mm_initialized_buffer().ok_or(CoreInitError::InitializedBufferUnavailable)?;

    let slot =
        buffer.get(cpu_index).ok_or(CoreInitError::CpuIndexOutOfRange { index: cpu_index, len: buffer.len() })?;

    Ok(slot.load(Ordering::Acquire) != 0)
}

/// Marks a specific core as initialized.
///
/// Writes a non-zero value to the 1-byte slot at `mm_initialized_buffer + cpu_index`.
fn mark_core_initialized(cpu_index: usize) -> MmSupervisorResult<()> {
    let buffer = init_state().mm_initialized_buffer().ok_or(CoreInitError::InitializedBufferUnavailable)?;

    let slot =
        buffer.get(cpu_index).ok_or(CoreInitError::CpuIndexOutOfRange { index: cpu_index, len: buffer.len() })?;

    slot.store(1, Ordering::Release);
    Ok(())
}

/// Returns whether `cpu_index` has already completed initialization, reading an unpublished buffer
/// as a first entry rather than as a failure.
///
/// The buffer is published part way through BSP initialization, so every core that arrives in the
/// initializing MMI finds nothing to read: the BSP before it publishes the buffer, and any AP that
/// reaches the check before the BSP gets there. None of them can have been initialized. The buffer
/// is never withdrawn once published, so a later entry cannot reach this case, and a bad index is
/// still reported.
fn core_already_initialized(cpu_index: usize) -> MmSupervisorResult<bool> {
    match is_core_initialized(cpu_index) {
        Err(MmSupervisorError::CoreInit(CoreInitError::InitializedBufferUnavailable)) => Ok(false),
        result => result,
    }
}

impl<P: PlatformInfo> MmSupervisorCore<P> {
    /// Creates the supervisor. Performs no heap allocation, so it can initialize a `static`.
    pub const fn new() -> Self {
        Self {
            cpu_manager: CpuManager::new(),
            mailbox_manager: MailboxManager::new(),
            syscall_interface: SyscallInterface::new(),
            initialized: AtomicBool::new(false),
            number_of_cores: AtomicUsize::new(0),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Returns the number of cores the MP Information HOB reports, or zero before the BSP has
    /// read that HOB during initialization.
    ///
    /// The BSP waits for this many cores in every MMI before it serves a request.
    pub(crate) fn number_of_cores(&self) -> usize {
        self.number_of_cores.load(Ordering::Acquire)
    }

    /// Stores this instance's address for [`Self::instance`].
    ///
    /// On the first call it also registers the processor ID lookup and the AP startup
    /// trampoline for this instance's type. Returns `false` if an instance was already stored.
    #[must_use]
    pub(crate) fn set_instance(&'static self) -> bool {
        let physical_address = NonNull::from_ref(self).expose_provenance();
        let stored = init_state().set_supervisor(physical_address);
        if stored {
            init_state().set_processor_id_lookup_fn(Self::processor_id_by_index);
            init_state().set_ap_startup_fn(Self::start_ap_procedure_trampoline);
        }
        stored
    }

    /// Returns the instance stored by [`Self::set_instance`].
    ///
    /// ## Panics
    ///
    /// Panics if no instance has been stored.
    pub(crate) fn instance() -> &'static Self {
        // SAFETY: `set_instance` stores the address of a `&'static Self` and accepts only the
        // first one, so the address stays valid and never changes.
        unsafe {
            NonNull::<Self>::with_exposed_provenance(
                init_state().supervisor().expect("MM Supervisor Core is not initialized."),
            )
            .as_ref()
        }
    }

    /// Entry point for every core, BSP and APs.
    ///
    /// The BSP's first entry comes from its own SMI in the C `ExecuteFirstSmiInit`, before the APs
    /// are sent theirs. The BSP runs one-time initialization, starts the user core, and runs
    /// per-core initialization. Each AP then runs per-core initialization on its own first
    /// entry. On every later entry the BSP waits for every core the MP Information HOB reports to
    /// enter the MMI, serves the pending request while the APs wait in the holding pen, and each
    /// core returns once the APs have been released.
    ///
    /// ## Panics
    ///
    /// Panics if entry fails. The supervisor cannot continue from a failed entry.
    ///
    /// ## Safety
    ///
    /// Must be called from the MM entry path on each core, with the environment the MM IPL sets
    /// up. `hob_list` must point to a valid HOB list during the BSP's first entry. It is not read
    /// on later entries.
    pub unsafe fn entry_point(&'static self, cpu_index: usize, hob_list: *const c_void) {
        // SAFETY: the caller upholds `entry_point_internal`'s contract, which is the same.
        if let Err(err) = unsafe { self.entry_point_internal(cpu_index, hob_list) } {
            panic!("Failed to enter MM Supervisor Core internal entry point: {err}");
        }
    }

    /// Runs [`Self::entry_point`], returning any failure instead of panicking.
    ///
    /// ## Safety
    ///
    /// Same contract as [`Self::entry_point`].
    unsafe fn entry_point_internal(&'static self, cpu_index: usize, hob_list: *const c_void) -> MmSupervisorResult<()> {
        let cpu_id = current_apic_id();

        // The BSP flag in IA32_APIC_BASE identifies the BSP.
        let is_bsp = is_bsp();

        log::trace!("CPU {cpu_id} (index {cpu_index}) entering MM Supervisor Core (BSP: {is_bsp})");
        let core_initialized = core_already_initialized(cpu_index)?;

        if core_initialized {
            // Later entry: serve the request or wait in the holding pen, then return.
            log::trace!("CPU {cpu_id} (index {cpu_index}) re-entering MM Supervisor Core, skipping initialization.");
            smrr_enable();
            if is_bsp {
                self.free_init_module(init_state());
            }
            self.enter_runtime(cpu_id, cpu_index);

            return Ok(());
        }

        if is_bsp {
            assert!(self.set_instance(), "MM Supervisor Core instance was already set!");
            assert!(!hob_list.is_null(), "MM Supervisor Core requires a non-null HOB list pointer.");

            log::info!("MM Supervisor Core v{}", env!("CARGO_PKG_VERSION"));
            log::info!("BSP (CPU {cpu_id}, index {cpu_index}) starting one-time initialization...");

            // SAFETY: `hob_list` was checked non-null above, and the caller guarantees it points
            // to a valid HOB list during the BSP's first entry.
            let hob_hand_off_table = unsafe { hob_list.cast::<PhaseHandoffInformationTable>().as_ref_unchecked() };
            let user_hob_list = self.bsp_init(hob_hand_off_table)?;

            // `bsp_init` published the CPU slots, so the BSP can now take its own.
            self.cpu_manager.register_cpu(cpu_id, cpu_index, true)?;

            // Demote to the user core entry point found in the HOB list.
            let user_entry = init_state()
                .user_entry_point()
                .filter(|&entry| entry != 0)
                .ok_or(CoreInitError::UserEntryPointMissing)?;

            let cpl3_stack = self.syscall_interface.get_cpl3_stack(cpu_index)?;

            // SAFETY: this is the first transition from the supervisor (CPL0) to the user core
            // (CPL3). Initialization set up and validated the entry point and stack, and the user
            // core validates any further inputs.
            let ret = unsafe {
                invoke_demoted_routine(
                    cpu_index,
                    user_entry,
                    cpl3_stack,
                    DEMOTED_ROUTINE_ARG_COUNT,
                    UserCommandType::StartUserCore as u64,
                    user_hob_list,
                    0,
                )
            };
            log::info!("Returned from user entry point with value: 0x{ret:016x}");

            // Let the waiting APs proceed.
            self.initialized.store(true, Ordering::Release);
            init_state().mark_bsp_init_complete();

            log::info!("BSP initialization complete.");
        } else {
            log::info!("AP (CPU {cpu_id}, index {cpu_index}) waiting for BSP initialization...");

            while !init_state().is_bsp_init_complete() {
                core::hint::spin_loop();
            }

            self.cpu_manager.register_cpu(cpu_id, cpu_index, false)?;
        }

        self.per_core_init(cpu_id)?;

        mark_core_initialized(cpu_index)?;

        let init_count = init_state().inc_per_core_init_count();
        log::info!("CPU {cpu_id} (index {cpu_index}) completed per-core init ({init_count} cores initialized)");

        Ok(())
    }

    /// Returns the UEFI processor ID registered at a dense CPU index.
    fn processor_id_by_index(cpu_index: usize) -> Option<u64> {
        Self::instance().cpu_manager.get_cpu_id_by_index(cpu_index).map(u64::from)
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
        let supervisor: MmSupervisorCore<TestPlatform> = MmSupervisorCore::new();

        // The per-core storage is only published by BSP initialization.
        assert_eq!(supervisor.cpu_manager.get_cpu_id_by_index(0), None);
        assert_eq!(supervisor.mailbox_manager.check_mailbox(0), None);
        assert!(!supervisor.initialized.load(Ordering::Relaxed));
    }

    #[test]
    fn test_supervisor_is_const() {
        static _SUPERVISOR: MmSupervisorCore<TestPlatform> = MmSupervisorCore::new();
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
