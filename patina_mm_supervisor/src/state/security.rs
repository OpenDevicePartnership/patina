//! Security-Relevant Global State
//!
//! Holds [`SecurityState`], the state the syscall dispatcher and the request handlers validate
//! against: the policy gate, the page table, the allocators, the unblocked memory tracker, the
//! communication buffer configuration and the save-state hand-off. One global instance backs it.
//!
//! Kept apart from [`super::init`] because the two are read by different callers for
//! different reasons. This half is consulted on every syscall, so what it holds and who may
//! change it is worth reading in one place.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use spin::{Mutex, MutexGuard, Once, relax::Spin};

use patina_paging::x64::X64PageTable;

use crate::{
    comm_buffer::CommBufferConfig,
    handlers::UnblockedMemoryTracker,
    memory::{PageAllocator, PagingPoolAllocator, SharedPagingAllocator},
    policy::gate::PolicyGate,
};

use super::save_state::{SaveStateAccessHolder, SaveStateInfo};

/// Type alias for the global page table guarded by the [`SecurityState`].
type SupervisorPageTable = X64PageTable<SharedPagingAllocator>;

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
    /// Communication buffer configuration from the `PassDown` HOB.
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

/// Global security-relevant state instance.
static SECURITY_STATE: SecurityState = SecurityState::new();

#[inline]
pub(crate) fn security_state() -> &'static SecurityState {
    &SECURITY_STATE
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::comm_buffer::CommChannel;
    use patina_internal_cpu::save_state::MmSaveStateRegister;

    #[test]
    fn test_security_state_defaults() {
        let state = SecurityState::new();

        assert!(state.policy_gate().is_none());
        assert!(state.lock_page_table().is_none());
        assert!(state.comm_buffer_config().is_none());
        assert!(state.save_state_info().is_none());
        assert!(state.lock_save_state_access().is_none());

        assert!(!state.page_allocator().is_initialized());
        assert!(!state.paging_allocator().is_initialized());
        assert_eq!(state.unblocked_tracker().region_count(), 0);
        assert!(!state.unblocked_tracker().is_core_init_complete());
    }

    #[test]
    fn test_security_state_one_time_values_ignore_later_writes() {
        let state = SecurityState::new();

        state.set_comm_buffer_config(CommBufferConfig {
            supervisor: CommChannel { external: 0x1000, ..Default::default() },
            ..Default::default()
        });
        state.set_comm_buffer_config(CommBufferConfig {
            supervisor: CommChannel { external: 0x2000, ..Default::default() },
            ..Default::default()
        });
        assert_eq!(state.comm_buffer_config().unwrap().supervisor.external, 0x1000);

        state.set_save_state_info(SaveStateInfo { number_of_cpus: 4, sm_base: 0x3000 });
        state.set_save_state_info(SaveStateInfo { number_of_cpus: 8, sm_base: 0x9000 });
        let info = state.save_state_info().unwrap();
        assert_eq!((info.number_of_cpus, info.sm_base), (4, 0x3000));
    }

    #[test]
    fn test_security_state_save_state_handoff_slot_round_trips() {
        let state = SecurityState::new();

        *state.lock_save_state_access() = Some(SaveStateAccessHolder {
            caller: 1,
            user_protocol: 0x1234,
            register: MmSaveStateRegister::Rax,
            cpu_index: 2,
        });

        let holder = state.lock_save_state_access().take().expect("hand-off is staged");
        assert_eq!(holder.user_protocol, 0x1234);
        assert_eq!(holder.cpu_index, 2);
        assert!(state.lock_save_state_access().is_none());
    }
}
