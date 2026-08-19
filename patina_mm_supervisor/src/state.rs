//! Consolidated global state for the MM Supervisor Core.
//!
//! This module collapses what used to be a dozen-plus free-standing `static`
//! variables into two cohesive, non-generic structures:
//!
//! - [`InitState`] — boot/synchronization flags and the type-erased entry-point
//!   handles used while bringing cores online.
//! - [`SecurityState`] — the security-relevant state (policy gate, page table,
//!   allocators, unblocked-memory tracker, communication-buffer configuration and
//!   the save-state hand-off) that the syscall dispatcher and request handlers
//!   validate against.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use spin::{Mutex, MutexGuard, Once, relax::Spin};

use patina::{
    guid::EVENT_EXIT_BOOT_SERVICES,
    management_mode::protocol::mm_supervisor_request::MM_SUPERVISOR_REQUEST_HANDLER_GUID,
};
use patina_paging::x64::X64PageTable;

use crate::{
    CommBufferConfig,
    mem::{PageAllocator, PagingPoolAllocator, SharedPagingAllocator},
    mm_policy::gate::PolicyGate,
    save_state::{SaveStateAccessHolder, SaveStateInfo},
    smrr::SmramRegion,
    supervisor_handlers::{
        EFI_DXE_MM_READY_TO_LOCK_PROTOCOL_GUID, SupervisorMmiHandler, UnblockedMemoryTracker,
        mm_exit_boot_services_handler, mm_ready_to_lock_handler, mm_supv_request_handler,
    },
};

/// Type alias for the global page table guarded by the [`SecurityState`].
type SupervisorPageTable = X64PageTable<SharedPagingAllocator>;

/// Boot-time and per-core synchronization state for the MM Supervisor Core.
pub(crate) struct InitState {
    /// Physical address of the global `MmSupervisorCore` instance.
    supervisor: Once<NonZeroUsize>,
    /// Type-erased lookup for the processor ID at a dense CPU index.
    processor_id_lookup_fn: Once<fn(usize) -> Option<u64>>,
    /// Set once BSP one-time initialization has completed.
    bsp_init_complete: AtomicBool,
    /// Pointer to the per-core initialized buffer from the PassDown HOB.
    mm_initialized_buffer: Once<u64>,
    /// Number of cores that have completed per-core initialization.
    per_core_init_count: AtomicU32,
    /// User module entry point discovered from the HOB list.
    user_entry_point: Once<u64>,
    /// MSEG base address discovered from the MSEG SMRAM HOB, if the platform
    /// publishes one. Programmed into `IA32_SMM_MONITOR_CTL` during per-core init.
    mseg_base: Once<u64>,
    /// Type-erased AP startup dispatch function (conformed for the platform).
    ap_startup_fn: Once<fn(u64, u64, u64) -> u64>,
    /// Set once ExitBootServices has been signaled.
    at_runtime: Once<()>,
    /// SMRR region (base and size) derived during BSP initialization.
    smrr_range: Once<SmramRegion>,
}

impl InitState {
    /// Creates an empty, uninitialized [`InitState`].
    pub(crate) const fn new() -> Self {
        Self {
            supervisor: Once::new(),
            processor_id_lookup_fn: Once::new(),
            bsp_init_complete: AtomicBool::new(false),
            mm_initialized_buffer: Once::new(),
            per_core_init_count: AtomicU32::new(0),
            user_entry_point: Once::new(),
            mseg_base: Once::new(),
            ap_startup_fn: Once::new(),
            at_runtime: Once::new(),
            smrr_range: Once::new(),
        }
    }

    /// Records the supervisor instance address.
    ///
    /// Returns `true` if `addr` is the value now stored (i.e. this call won the
    /// one-time initialization), matching the previous `call_once` comparison.
    pub(crate) fn set_supervisor(&self, addr: NonZeroUsize) -> bool {
        &addr == self.supervisor.call_once(|| addr)
    }

    /// Returns the stored supervisor instance address, if set.
    pub(crate) fn supervisor(&self) -> Option<NonZeroUsize> {
        self.supervisor.get().copied()
    }

    /// Stores the type-erased processor-ID lookup (one-time).
    pub(crate) fn set_processor_id_lookup_fn(&self, func: fn(usize) -> Option<u64>) {
        self.processor_id_lookup_fn.call_once(|| func);
    }

    /// Returns the type-erased processor-ID lookup, if initialized.
    pub(crate) fn processor_id_lookup_fn(&self) -> Option<fn(usize) -> Option<u64>> {
        self.processor_id_lookup_fn.get().copied()
    }

    /// Marks BSP one-time initialization as complete (Release ordering).
    pub(crate) fn mark_bsp_init_complete(&self) {
        self.bsp_init_complete.store(true, Ordering::Release);
    }

    /// Returns whether BSP one-time initialization has completed (Acquire ordering).
    pub(crate) fn is_bsp_init_complete(&self) -> bool {
        self.bsp_init_complete.load(Ordering::Acquire)
    }

    /// Stores the per-core initialized buffer base address (one-time).
    pub(crate) fn set_mm_initialized_buffer(&self, buffer_base: u64) {
        self.mm_initialized_buffer.call_once(|| buffer_base);
    }

    /// Returns the per-core initialized buffer base address, if set.
    pub(crate) fn mm_initialized_buffer(&self) -> Option<u64> {
        self.mm_initialized_buffer.get().copied()
    }

    /// Increments the per-core init count and returns the new value (SeqCst).
    pub(crate) fn inc_per_core_init_count(&self) -> u32 {
        self.per_core_init_count.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Returns the number of cores that have completed per-core init (Acquire).
    pub(crate) fn per_core_init_count(&self) -> u32 {
        self.per_core_init_count.load(Ordering::Acquire)
    }

    /// Stores the user module entry point (one-time).
    pub(crate) fn set_user_entry_point(&self, entry: u64) {
        self.user_entry_point.call_once(|| entry);
    }

    /// Returns the user module entry point, if set.
    pub(crate) fn user_entry_point(&self) -> Option<u64> {
        self.user_entry_point.get().copied()
    }

    /// Stores the MSEG base address discovered from the MSEG SMRAM HOB (one-time).
    pub(crate) fn set_mseg_base(&self, base: u64) {
        self.mseg_base.call_once(|| base);
    }

    /// Returns the MSEG base address, if the platform published an MSEG SMRAM HOB.
    pub(crate) fn mseg_base(&self) -> Option<u64> {
        self.mseg_base.get().copied()
    }

    /// Stores the type-erased AP startup function (one-time).
    pub(crate) fn set_ap_startup_fn(&self, func: fn(u64, u64, u64) -> u64) {
        self.ap_startup_fn.call_once(|| func);
    }

    /// Returns the type-erased AP startup function, if set.
    pub(crate) fn ap_startup_fn(&self) -> Option<fn(u64, u64, u64) -> u64> {
        self.ap_startup_fn.get().copied()
    }

    /// Marks the supervisor as having entered runtime (ExitBootServices signaled).
    pub(crate) fn mark_at_runtime(&self) -> bool {
        let first = !self.at_runtime.is_completed();
        self.at_runtime.call_once(|| ());
        first
    }

    /// Returns whether ExitBootServices has been signaled.
    pub(crate) fn is_at_runtime(&self) -> bool {
        self.at_runtime.is_completed()
    }

    /// Stores the SMRR region (one-time).
    pub(crate) fn set_smrr_range(&self, range: SmramRegion) {
        self.smrr_range.call_once(|| range);
    }

    /// Returns the SMRR region, if set.
    pub(crate) fn smrr_range(&self) -> Option<SmramRegion> {
        self.smrr_range.get().copied()
    }
}

/// Security-relevant global state for the MM Supervisor Core.
pub(crate) struct SecurityState {
    /// Firmware security policy gate.
    policy_gate: Once<PolicyGate>,
    /// Global page table used for managing page attributes.
    page_table: Mutex<Option<SupervisorPageTable>>,
    /// SMRAM page-granularity allocator for general use.
    page_allocator: PageAllocator,
    /// Dedicated bump allocator for page-table structures.
    paging_allocator: PagingPoolAllocator,
    /// Tracker for all unblocked memory regions.
    unblocked_memory_tracker: UnblockedMemoryTracker,
    /// Communication buffer configuration from the PassDown HOB.
    comm_buffer_config: Once<CommBufferConfig>,
    /// Per-CPU save-state metadata for the save-state read syscall.
    save_state_info: Once<SaveStateInfo>,
    /// In-flight two-phase save-state read hand-off.
    save_state_access: Mutex<Option<SaveStateAccessHolder>>,
}

impl SecurityState {
    /// Creates an empty, uninitialized [`SecurityState`].
    pub(crate) const fn new() -> Self {
        Self {
            policy_gate: Once::new(),
            page_table: Mutex::new(None),
            page_allocator: PageAllocator::new(),
            paging_allocator: PagingPoolAllocator::new(),
            unblocked_memory_tracker: UnblockedMemoryTracker::new(),
            comm_buffer_config: Once::new(),
            save_state_info: Once::new(),
            save_state_access: Mutex::new(None),
        }
    }

    /// Stores the firmware policy gate (one-time).
    pub(crate) fn set_policy_gate(&self, gate: PolicyGate) {
        self.policy_gate.call_once(|| gate);
    }

    /// Returns the firmware policy gate, if initialized.
    pub(crate) fn policy_gate(&self) -> Option<&PolicyGate> {
        self.policy_gate.get()
    }

    /// Locks the global page table for read or modification.
    pub(crate) fn lock_page_table(&self) -> MutexGuard<'_, Option<SupervisorPageTable>, Spin> {
        self.page_table.lock()
    }

    /// Returns the SMRAM page allocator.
    pub(crate) fn page_allocator(&self) -> &PageAllocator {
        &self.page_allocator
    }

    /// Returns the page-table-pool allocator.
    pub(crate) fn paging_allocator(&self) -> &PagingPoolAllocator {
        &self.paging_allocator
    }

    /// Returns the unblocked-memory tracker.
    pub(crate) fn unblocked_tracker(&self) -> &UnblockedMemoryTracker {
        &self.unblocked_memory_tracker
    }

    /// Stores the communication buffer configuration (one-time).
    pub(crate) fn set_comm_buffer_config(&self, config: CommBufferConfig) {
        self.comm_buffer_config.call_once(|| config);
    }

    /// Returns the communication buffer configuration, if set.
    pub(crate) fn comm_buffer_config(&self) -> Option<&CommBufferConfig> {
        self.comm_buffer_config.get()
    }

    /// Stores the per-CPU save-state metadata (one-time).
    pub(crate) fn set_save_state_info(&self, info: SaveStateInfo) {
        self.save_state_info.call_once(|| info);
    }

    /// Returns the per-CPU save-state metadata, if set.
    pub(crate) fn save_state_info(&self) -> Option<SaveStateInfo> {
        self.save_state_info.get().copied()
    }

    /// Locks the in-flight save-state hand-off slot.
    pub(crate) fn lock_save_state_access(&self) -> MutexGuard<'_, Option<SaveStateAccessHolder>, Spin> {
        self.save_state_access.lock()
    }
}

/// Global boot/synchronization state instance.
static INIT_STATE: InitState = InitState::new();

/// Global security-relevant state instance.
static SECURITY_STATE: SecurityState = SecurityState::new();

/// Returns the global [`InitState`].
#[inline]
pub(crate) fn init_state() -> &'static InitState {
    &INIT_STATE
}

/// Returns the global [`SecurityState`].
#[inline]
pub(crate) fn security_state() -> &'static SecurityState {
    &SECURITY_STATE
}

/// The core's built-in supervisor MMI handlers.
///
/// - **MmReadyToLock**: Triggered from the non-MM environment upon DxeMmReadyToLock event.
///   After this handler runs, certain features (e.g., unblock memory) are no longer available.
/// - **MmSupvRequest**: Handles general supervisor requests such as unblock memory, fetch
///   policy, version info, and communication buffer updates.
/// - **MmExitBootServices**: Triggered from the non-MM environment upon ExitBootServices. After
///   this handler runs, the supervisor communication channel is closed and supervisor-targeted
///   requests are rejected.
///
/// This is an immutable, compile-time dispatch table with no runtime state. It is kept here
/// alongside the other module-level globals purely for locality.
pub(crate) static DEFAULT_SUPERVISOR_MMI_HANDLERS: &[SupervisorMmiHandler] = &[
    SupervisorMmiHandler {
        name: "MmReadyToLock",
        handler_guid: EFI_DXE_MM_READY_TO_LOCK_PROTOCOL_GUID.into_inner(),
        handle: mm_ready_to_lock_handler,
    },
    SupervisorMmiHandler {
        name: "MmSupvRequest",
        handler_guid: MM_SUPERVISOR_REQUEST_HANDLER_GUID.into_inner(),
        handle: mm_supv_request_handler,
    },
    SupervisorMmiHandler {
        name: "MmExitBootServices",
        handler_guid: EVENT_EXIT_BOOT_SERVICES.into_inner(),
        handle: mm_exit_boot_services_handler,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn processor_id_lookup(cpu_index: usize) -> Option<u64> {
        Some(cpu_index as u64 + 0x10)
    }

    fn replacement_processor_id_lookup(_: usize) -> Option<u64> {
        None
    }

    #[test]
    fn test_init_state_defaults() {
        let state = InitState::new();

        assert!(state.supervisor().is_none());
        assert!(state.processor_id_lookup_fn().is_none());
        assert!(!state.is_bsp_init_complete());
        assert_eq!(state.per_core_init_count(), 0);
        assert!(state.user_entry_point().is_none());
        assert!(state.mseg_base().is_none());
        assert!(state.ap_startup_fn().is_none());
        assert!(!state.is_at_runtime());
        assert!(state.smrr_range().is_none());
    }

    #[test]
    fn test_init_state_processor_id_lookup_is_initialized_once() {
        let state = InitState::new();

        state.set_processor_id_lookup_fn(processor_id_lookup);
        assert_eq!(state.processor_id_lookup_fn().unwrap()(3), Some(0x13));

        state.set_processor_id_lookup_fn(replacement_processor_id_lookup);
        assert_eq!(state.processor_id_lookup_fn().unwrap()(3), Some(0x13));
    }

    #[test]
    fn test_init_state_synchronization_updates() {
        let state = InitState::new();

        state.mark_bsp_init_complete();
        assert!(state.is_bsp_init_complete());

        assert_eq!(state.inc_per_core_init_count(), 1);
        assert_eq!(state.inc_per_core_init_count(), 2);
        assert_eq!(state.per_core_init_count(), 2);

        assert!(state.mark_at_runtime());
        assert!(!state.mark_at_runtime());
        assert!(state.is_at_runtime());
    }
}
