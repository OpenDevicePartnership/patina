//! CPU Management Module
//!
//! This module provides CPU identification and management for the MM Supervisor
//! Core. It handles BSP/AP detection, CPU registration, and state tracking.
//!
//! ## Memory Model
//!
//! This module does not perform heap allocation. All structures use fixed-size
//! arrays with compile-time constants provided via const generics.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use crate::semaphore::{sem_signal, sem_try_take, sem_wait};

/// The state of an Application Processor (AP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApState {
    /// The AP has not been registered yet.
    NotPresent = 0,
    /// The AP is in the holding pen, waiting for work.
    InHoldingPen = 1,
    /// The AP is currently executing a task.
    Busy = 2,
    /// The AP has been halted.
    Halted = 3,
}

impl From<u8> for ApState {
    fn from(value: u8) -> Self {
        match value {
            0 => ApState::NotPresent,
            1 => ApState::InHoldingPen,
            2 => ApState::Busy,
            3 => ApState::Halted,
            _ => ApState::NotPresent,
        }
    }
}

/// Information about a registered CPU stored in a fixed-size slot.
#[repr(C)]
struct CpuSlot {
    /// The CPU's APIC ID. u32::MAX means slot is unused.
    cpu_id: AtomicU32,
    /// Whether this CPU is the BSP (0 = AP, 1 = BSP).
    is_bsp: AtomicU8,
    /// Current state (for APs only).
    state: AtomicU8,
    /// Padding for alignment.
    _padding: [u8; 2],
    /// Rendezvous semaphore for the SMI exit barrier.
    run: AtomicU32,
}

impl CpuSlot {
    /// Creates a new empty CPU slot.
    const fn new() -> Self {
        Self {
            cpu_id: AtomicU32::new(u32::MAX),
            is_bsp: AtomicU8::new(0),
            state: AtomicU8::new(ApState::NotPresent as u8),
            _padding: [0; 2],
            run: AtomicU32::new(0),
        }
    }

    /// Checks if this slot is in use.
    fn is_used(&self) -> bool {
        self.cpu_id.load(Ordering::Acquire) != u32::MAX
    }

    /// Gets the CPU ID if the slot is used.
    fn get_cpu_id(&self) -> Option<u32> {
        let id = self.cpu_id.load(Ordering::Acquire);
        if id == u32::MAX { None } else { Some(id) }
    }
}

/// Manager for CPU-related operations.
///
/// Tracks registered CPUs and their states using fixed-size arrays.
///
/// ## Const Generic Parameters
///
/// * `MAX_CPUS` - The maximum number of CPUs that can be registered.
pub struct CpuManager<const MAX_CPUS: usize> {
    /// CPU slots - fixed size array.
    slots: [CpuSlot; MAX_CPUS],
    /// Number of CPUs currently registered.
    registered_count: AtomicU32,
    /// The APIC ID of the BSP.
    bsp_id: AtomicU32,
}

impl<const MAX_CPUS: usize> CpuManager<MAX_CPUS> {
    /// Creates a new CPU manager.
    ///
    /// This is a const fn and performs no heap allocation.
    pub const fn new() -> Self {
        Self {
            slots: [const { CpuSlot::new() }; MAX_CPUS],
            registered_count: AtomicU32::new(0),
            bsp_id: AtomicU32::new(u32::MAX),
        }
    }

    /// Registers a CPU with the manager.
    ///
    /// `cpu_id` is the APIC ID read from CPUID (sparse, not 0-based), while `cpu_index` is
    /// the dense, 0-based UEFI processor index that selects this CPU's slot.
    ///
    /// Returns `Some(cpu_index)` on success. Re-registering the same CPU is idempotent and
    /// returns the same index. Returns `None` if `cpu_index` is out of range or the
    /// slot is already occupied by a different CPU.
    pub fn register_cpu(&self, cpu_id: u32, cpu_index: usize, is_bsp: bool) -> Option<usize> {
        if cpu_index >= MAX_CPUS {
            log::warn!(
                "cpu_index {} exceeds maximum CPU count ({}), cannot register CPU {}",
                cpu_index,
                MAX_CPUS,
                cpu_id
            );
            return None;
        }

        // Each physical CPU owns a distinct, stable `cpu_index`, so this slot is only
        // ever written by this CPU. That makes the load-check-then-store below safe
        // without a compare-exchange.
        let slot = self.slots.get(cpu_index)?;
        match slot.get_cpu_id() {
            Some(existing) if existing == cpu_id => {
                // Idempotent re-registration.
                log::trace!("CPU {} already registered at index {}", cpu_id, cpu_index);
                return Some(cpu_index);
            }
            Some(existing) => {
                log::error!(
                    "cpu_index {} already occupied by APIC {}, cannot register APIC {}",
                    cpu_index,
                    existing,
                    cpu_id
                );
                return None;
            }
            None => {}
        }

        slot.is_bsp.store(if is_bsp { 1 } else { 0 }, Ordering::Release);
        slot.state.store(if is_bsp { ApState::Busy as u8 } else { ApState::NotPresent as u8 }, Ordering::Release);
        // Publish the CPU ID last so readers that observe it also see the fields above.
        slot.cpu_id.store(cpu_id, Ordering::Release);

        self.registered_count.fetch_add(1, Ordering::SeqCst);

        if is_bsp {
            self.bsp_id.store(cpu_id, Ordering::SeqCst);
            log::info!("Registered BSP with APIC ID {} at index {}", cpu_id, cpu_index);
        } else {
            log::trace!("Registered AP with APIC ID {} at index {}", cpu_id, cpu_index);
        }

        Some(cpu_index)
    }

    /// Gets the number of registered CPUs.
    pub fn registered_count(&self) -> usize {
        self.registered_count.load(Ordering::SeqCst) as usize
    }

    /// Gets the maximum number of CPUs supported.
    pub const fn max_cpus(&self) -> usize {
        MAX_CPUS
    }

    /// Gets the APIC ID of the BSP.
    pub fn bsp_id(&self) -> Option<u32> {
        let id = self.bsp_id.load(Ordering::SeqCst);
        if id == u32::MAX { None } else { Some(id) }
    }

    /// Checks if the given CPU ID is the BSP.
    pub fn is_bsp(&self, cpu_id: u32) -> bool {
        self.bsp_id() == Some(cpu_id)
    }

    /// Finds the slot index for a given CPU ID (APIC ID).
    fn find_slot(&self, cpu_id: u32) -> Option<usize> {
        for (index, slot) in self.slots.iter().enumerate() {
            if slot.get_cpu_id() == Some(cpu_id) {
                return Some(index);
            }
        }
        None
    }

    /// Finds the slot index for a given CPU ID (public wrapper).
    pub fn find_cpu_index(&self, cpu_id: u32) -> Option<usize> {
        self.find_slot(cpu_id)
    }

    /// Gets the APIC ID of the CPU at the given slot index.
    ///
    /// Returns `None` if the index is out of range or the slot is unused.
    pub fn get_cpu_id_by_index(&self, index: usize) -> Option<u32> {
        self.slots.get(index)?.get_cpu_id()
    }

    /// Gets the AP state by slot index.
    ///
    /// Returns `None` if the index is out of range or the slot is unused.
    pub fn get_ap_state_by_index(&self, index: usize) -> Option<ApState> {
        let slot = self.slots.get(index)?;
        if slot.is_used() { Some(ApState::from(slot.state.load(Ordering::Acquire))) } else { None }
    }

    /// Gets the state of an AP.
    pub fn get_ap_state(&self, cpu_id: u32) -> Option<ApState> {
        let index = self.find_slot(cpu_id)?;
        let slot = self.slots.get(index)?;
        Some(ApState::from(slot.state.load(Ordering::Acquire)))
    }

    /// Sets the state of an AP.
    pub fn set_ap_state(&self, cpu_id: u32, state: ApState) -> bool {
        let index = match self.find_slot(cpu_id) {
            Some(idx) => idx,
            None => return false,
        };

        let Some(slot) = self.slots.get(index) else {
            return false;
        };

        // Don't allow changing BSP state
        if slot.is_bsp.load(Ordering::Acquire) != 0 {
            log::warn!("Attempted to change BSP state, ignoring");
            return false;
        }

        slot.state.store(state as u8, Ordering::Release);
        true
    }

    /// Iterates over all registered AP IDs.
    ///
    /// Calls the provided closure for each registered AP.
    pub fn for_each_ap<F: FnMut(u32)>(&self, mut f: F) {
        for slot in &self.slots {
            if let Some(cpu_id) = slot.get_cpu_id()
                && slot.is_bsp.load(Ordering::Acquire) == 0
            {
                f(cpu_id);
            }
        }
    }

    /// Counts APs in a specific state.
    pub fn count_aps_in_state(&self, state: ApState) -> usize {
        let mut count = 0;
        for slot in &self.slots {
            if slot.is_used()
                && slot.is_bsp.load(Ordering::Acquire) == 0
                && slot.state.load(Ordering::Acquire) == state as u8
            {
                count += 1;
            }
        }
        count
    }

    /// Releases every registered AP from the exit barrier by signaling each AP's
    /// rendezvous semaphore. (Called by the BSP)
    ///
    pub fn release_all_aps(&self) {
        for slot in &self.slots {
            if slot.is_used() && slot.is_bsp.load(Ordering::Acquire) == 0 {
                sem_signal(&slot.run);
            }
        }
    }

    /// Consumes a pending exit-barrier release for the AP at `cpu_index`, if the BSP has
    /// signaled one. Non-blocking; returns whether the AP was released.
    ///
    /// Called by an AP spinning in the holding pen.
    pub fn take_release_by_index(&self, cpu_index: usize) -> bool {
        let Some(slot) = self.slots.get(cpu_index) else {
            return false;
        };
        sem_try_take(&slot.run)
    }

    /// Acknowledges to the BSP that this AP has left the holding pen by signaling the
    /// BSP's rendezvous semaphore.
    ///
    pub fn ack_exit_to_bsp(&self) {
        if let Some(bsp_id) = self.bsp_id()
            && let Some(index) = self.find_slot(bsp_id)
            && let Some(slot) = self.slots.get(index)
        {
            sem_signal(&slot.run);
        }
    }

    /// Blocks (spins, no timer) until `ap_count` APs have acknowledged leaving the pen.
    ///
    pub fn wait_for_ap_exit_acks(&self, ap_count: usize) {
        let bsp_index = match self.bsp_id().and_then(|bsp_id| self.find_slot(bsp_id)) {
            Some(index) => index,
            None => return,
        };
        let Some(slot) = self.slots.get(bsp_index) else {
            return;
        };
        for _ in 0..ap_count {
            sem_wait(&slot.run);
        }
    }
}

impl<const MAX_CPUS: usize> Default for CpuManager<MAX_CPUS> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_manager_creation() {
        let manager: CpuManager<4> = CpuManager::new();
        assert_eq!(manager.registered_count(), 0);
        assert!(manager.bsp_id().is_none());
        assert_eq!(manager.max_cpus(), 4);
    }

    #[test]
    fn test_cpu_manager_is_const() {
        // Verify we can create a static instance
        static _MANAGER: CpuManager<8> = CpuManager::new();
    }

    #[test]
    fn test_cpu_registration() {
        let manager: CpuManager<4> = CpuManager::new();

        // Register BSP
        let bsp_idx = manager.register_cpu(0, 0, true);
        assert_eq!(bsp_idx, Some(0));
        assert_eq!(manager.bsp_id(), Some(0));
        assert!(manager.is_bsp(0));

        // Register APs
        let ap1_idx = manager.register_cpu(1, 1, false);
        assert_eq!(ap1_idx, Some(1));
        assert!(!manager.is_bsp(1));

        let ap2_idx = manager.register_cpu(2, 2, false);
        assert_eq!(ap2_idx, Some(2));

        assert_eq!(manager.registered_count(), 3);
    }

    #[test]
    fn test_duplicate_registration() {
        let manager: CpuManager<4> = CpuManager::new();

        // Registering the same CPU twice is idempotent: the second call returns the
        // same index and does not consume another slot.
        let idx = manager.register_cpu(1, 1, false);
        assert!(idx.is_some());
        assert_eq!(manager.register_cpu(1, 1, false), idx);
        assert_eq!(manager.registered_count(), 1);
    }

    #[test]
    fn test_register_decouples_apic_id_from_cpu_index() {
        // APIC IDs are sparse and unordered; cpu_index is dense and 0-based. The CPU
        // must land in slots[cpu_index] regardless of APIC ID or registration order.
        let manager: CpuManager<4> = CpuManager::new();

        // Register out of order with non-contiguous APIC IDs.
        assert_eq!(manager.register_cpu(0x20, 2, false), Some(2));
        assert_eq!(manager.register_cpu(0x00, 0, true), Some(0));
        assert_eq!(manager.register_cpu(0x10, 1, false), Some(1));

        // cpu_index -> APIC ID
        assert_eq!(manager.get_cpu_id_by_index(0), Some(0x00));
        assert_eq!(manager.get_cpu_id_by_index(1), Some(0x10));
        assert_eq!(manager.get_cpu_id_by_index(2), Some(0x20));

        // APIC ID -> cpu_index
        assert_eq!(manager.find_cpu_index(0x00), Some(0));
        assert_eq!(manager.find_cpu_index(0x10), Some(1));
        assert_eq!(manager.find_cpu_index(0x20), Some(2));

        // A different APIC ID cannot take an already-occupied cpu_index.
        assert_eq!(manager.register_cpu(0x30, 1, false), None);
    }

    #[test]
    fn test_ap_state_management() {
        let manager: CpuManager<4> = CpuManager::new();
        manager.register_cpu(0, 0, true);
        manager.register_cpu(1, 1, false);

        // APs register as NotPresent and only enter the holding pen on check-in.
        assert_eq!(manager.get_ap_state(1), Some(ApState::NotPresent));

        // Change state
        assert!(manager.set_ap_state(1, ApState::Busy));
        assert_eq!(manager.get_ap_state(1), Some(ApState::Busy));

        // Cannot change BSP state
        assert!(!manager.set_ap_state(0, ApState::Halted));
    }

    #[test]
    fn test_for_each_ap() {
        let manager: CpuManager<4> = CpuManager::new();
        manager.register_cpu(0, 0, true);
        manager.register_cpu(1, 1, false);
        manager.register_cpu(2, 2, false);

        let mut ap_ids = [0u32; 4];
        let mut count = 0;
        manager.for_each_ap(|id| {
            if count < 4 {
                ap_ids[count] = id;
                count += 1;
            }
        });

        assert_eq!(count, 2);
        assert!(ap_ids[..count].contains(&1));
        assert!(ap_ids[..count].contains(&2));
    }

    #[test]
    fn test_max_cpu_limit() {
        let manager: CpuManager<2> = CpuManager::new();
        assert!(manager.register_cpu(0, 0, true).is_some());
        assert!(manager.register_cpu(1, 1, false).is_some());
        // cpu_index 2 is out of range for CpuManager<2>.
        assert!(manager.register_cpu(2, 2, false).is_none()); // Should fail
    }

    #[test]
    fn test_count_aps_in_state() {
        let manager: CpuManager<4> = CpuManager::new();
        manager.register_cpu(0, 0, true);
        manager.register_cpu(1, 1, false);
        manager.register_cpu(2, 2, false);

        // APs register as NotPresent; they only count as InHoldingPen after checking in.
        assert_eq!(manager.count_aps_in_state(ApState::NotPresent), 2);
        assert_eq!(manager.count_aps_in_state(ApState::InHoldingPen), 0);

        // Simulate both APs checking in to the holding pen.
        manager.set_ap_state(1, ApState::InHoldingPen);
        manager.set_ap_state(2, ApState::InHoldingPen);
        assert_eq!(manager.count_aps_in_state(ApState::InHoldingPen), 2);
        assert_eq!(manager.count_aps_in_state(ApState::Busy), 0);

        manager.set_ap_state(1, ApState::Busy);
        assert_eq!(manager.count_aps_in_state(ApState::InHoldingPen), 1);
        assert_eq!(manager.count_aps_in_state(ApState::Busy), 1);
    }

    #[test]
    fn test_exit_barrier_rendezvous() {
        let manager: CpuManager<4> = CpuManager::new();
        manager.register_cpu(0, 0, true); // BSP at slot 0
        manager.register_cpu(10, 1, false); // AP at slot 1
        manager.register_cpu(20, 2, false); // AP at slot 2

        // Nothing is pending before the BSP releases.
        assert!(!manager.take_release_by_index(1));
        assert!(!manager.take_release_by_index(2));

        // BSP releases all APs; each AP consumes its release exactly once.
        manager.release_all_aps();
        assert!(manager.take_release_by_index(1));
        assert!(!manager.take_release_by_index(1));
        assert!(manager.take_release_by_index(2));
        assert!(!manager.take_release_by_index(2));

        // Each AP acknowledges exit; the BSP's barrier then completes without blocking
        // (the acks were already counted into the BSP's semaphore).
        manager.ack_exit_to_bsp();
        manager.ack_exit_to_bsp();
        manager.wait_for_ap_exit_acks(2);

        // The semaphores are balanced again, ready for the next round.
        assert!(!manager.take_release_by_index(1));
    }
}
