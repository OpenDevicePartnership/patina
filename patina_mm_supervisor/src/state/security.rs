//! Security-Relevant Global State
//!
//! Holds [`SecurityState`], the state the syscall dispatcher and the request
//! handlers validate against: the policy gate, the page table, the allocators,
//! the unblocked memory tracker, the communication buffer configuration and the
//! save-state hand-off. One global instance backs it.
//!
//! Kept apart from [`super::init`] because the two are read by different
//! callers for different reasons. This half is consulted on every syscall.
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

use crate::privilege_mgmt::save_state::{SaveStateAccessHolder, SaveStateInfo};

/// Type alias for the global page table guarded by the [`SecurityState`].
pub(crate) type SupervisorPageTable = X64PageTable<SharedPagingAllocator>;

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
