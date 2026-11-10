//! CPU Management Module
//!
//! Tracks the CPUs registered with the MM Supervisor Core: each CPU's APIC ID, which one is
//! the BSP, the state of each AP, and the per-CPU semaphores of the SMI exit barrier.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use spin::Once;

use crate::error::MmSupervisorResult;

use super::CoreInitError;

/// Signals a counting rendezvous semaphore.
fn sem_signal(sem: &AtomicU32) {
    sem.fetch_add(1, Ordering::AcqRel);
}

/// Takes one count if the semaphore is positive, without blocking. Returns whether it did.
fn sem_try_take(sem: &AtomicU32) -> bool {
    let mut value = sem.load(Ordering::Acquire);
    while value != 0 {
        match sem.compare_exchange_weak(value, value - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(current) => value = current,
        }
    }
    false
}

/// The state of an Application Processor (AP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApState {
    /// The AP is registered but not in MM.
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
            1 => ApState::InHoldingPen,
            2 => ApState::Busy,
            3 => ApState::Halted,
            0 | 4..=u8::MAX => ApState::NotPresent,
        }
    }
}

/// Information about a registered CPU, one slot per CPU index.
#[repr(C)]
pub(crate) struct CpuSlot {
    /// The CPU's APIC ID, or `u32::MAX` while the slot is unused.
    cpu_id: AtomicU32,
    /// 1 if this CPU is the BSP, 0 otherwise.
    is_bsp: AtomicU8,
    /// The [`ApState`]. A BSP slot stays `Busy`.
    state: AtomicU8,
    /// Padding for alignment.
    _padding: [u8; 2],
    /// Rendezvous semaphore for the SMI exit barrier.
    run: AtomicU32,
}

impl CpuSlot {
    /// Creates an unused CPU slot.
    const fn new() -> Self {
        Self {
            cpu_id: AtomicU32::new(u32::MAX),
            is_bsp: AtomicU8::new(0),
            state: AtomicU8::new(ApState::NotPresent as u8),
            _padding: [0; 2],
            run: AtomicU32::new(0),
        }
    }

    /// Returns whether this slot is in use.
    fn is_used(&self) -> bool {
        self.cpu_id.load(Ordering::Acquire) != u32::MAX
    }

    /// Returns the CPU's APIC ID, or `None` if the slot is unused.
    fn get_cpu_id(&self) -> Option<u32> {
        let id = self.cpu_id.load(Ordering::Acquire);
        if id == u32::MAX { None } else { Some(id) }
    }
}

impl Default for CpuSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks the registered CPUs and their states, one slot per core.
///
/// [`CpuManager::init`] publishes the slots once during BSP initialization. Before that every
/// lookup finds nothing.
pub struct CpuManager {
    /// One slot per dense CPU index.
    slots: Once<&'static [CpuSlot]>,
    /// The APIC ID of the BSP.
    bsp_id: AtomicU32,
}

impl CpuManager {
    /// Creates a CPU manager with no slot storage.
    pub const fn new() -> Self {
        Self { slots: Once::new(), bsp_id: AtomicU32::new(u32::MAX) }
    }

    /// Publishes `slots`, one per CPU index. Each must be unused, as `CpuSlot::default` creates
    /// it.
    ///
    /// ## Errors
    ///
    /// Returns [`CoreInitError::CpuSlotsAlreadyPublished`] if slots were already published.
    pub(crate) fn init(&self, slots: &'static [CpuSlot]) -> MmSupervisorResult<()> {
        let mut published = false;
        self.slots.call_once(|| {
            published = true;
            slots
        });
        if !published {
            log::error!("CPU slot storage was already published");
            return Err(CoreInitError::CpuSlotsAlreadyPublished.into());
        }
        Ok(())
    }

    /// Returns the published slots, or an empty slice before [`CpuManager::init`].
    fn slots(&self) -> &'static [CpuSlot] {
        self.slots.get().copied().unwrap_or(&[])
    }

    /// Registers the CPU with APIC ID `cpu_id` in the slot at the dense, 0-based `cpu_index`, and
    /// returns `cpu_index`. Registering the same CPU again is a no-op.
    ///
    /// ## Errors
    ///
    /// Returns [`CoreInitError::CpuSlotsUnavailable`] before the slots are published,
    /// [`CoreInitError::CpuIndexOutOfRange`] if `cpu_index` has no slot, or
    /// [`CoreInitError::CpuIndexAlreadyRegistered`] if another CPU holds the slot.
    pub fn register_cpu(&self, cpu_id: u32, cpu_index: usize, is_bsp: bool) -> MmSupervisorResult<usize> {
        // Only the CPU that owns `cpu_index` writes this slot, so the check and store below need
        // no compare-exchange.
        let slots = self.slots.get().ok_or(CoreInitError::CpuSlotsUnavailable)?;
        let slot =
            slots.get(cpu_index).ok_or(CoreInitError::CpuIndexOutOfRange { index: cpu_index, len: slots.len() })?;
        match slot.get_cpu_id() {
            Some(existing) if existing == cpu_id => {
                log::trace!("CPU {cpu_id} already registered at index {cpu_index}");
                return Ok(cpu_index);
            }
            Some(existing) => {
                return Err(
                    CoreInitError::CpuIndexAlreadyRegistered { index: cpu_index, existing, requested: cpu_id }.into()
                );
            }
            None => {}
        }

        slot.is_bsp.store(u8::from(is_bsp), Ordering::Release);
        slot.state.store(if is_bsp { ApState::Busy as u8 } else { ApState::NotPresent as u8 }, Ordering::Release);
        // Publish the CPU ID last so readers that observe it also see the fields above.
        slot.cpu_id.store(cpu_id, Ordering::Release);

        if is_bsp {
            self.bsp_id.store(cpu_id, Ordering::SeqCst);
            log::info!("Registered BSP with APIC ID {cpu_id} at index {cpu_index}");
        } else {
            log::trace!("Registered AP with APIC ID {cpu_id} at index {cpu_index}");
        }

        Ok(cpu_index)
    }

    /// Returns the APIC ID of the BSP, once it has registered.
    pub fn bsp_id(&self) -> Option<u32> {
        let id = self.bsp_id.load(Ordering::SeqCst);
        if id == u32::MAX { None } else { Some(id) }
    }

    /// Returns whether `cpu_id` is the BSP's APIC ID.
    pub fn is_bsp(&self, cpu_id: u32) -> bool {
        self.bsp_id() == Some(cpu_id)
    }

    /// Returns the slot index registered to APIC ID `cpu_id`.
    fn find_slot(&self, cpu_id: u32) -> Option<usize> {
        for (index, slot) in self.slots().iter().enumerate() {
            if slot.get_cpu_id() == Some(cpu_id) {
                return Some(index);
            }
        }
        None
    }

    /// Returns the CPU index registered to APIC ID `cpu_id`.
    pub fn find_cpu_index(&self, cpu_id: u32) -> Option<usize> {
        self.find_slot(cpu_id)
    }

    /// Returns the APIC ID registered at `index`, or `None` if the index is out of range or the
    /// slot is unused.
    pub fn get_cpu_id_by_index(&self, index: usize) -> Option<u32> {
        self.slots().get(index)?.get_cpu_id()
    }

    /// Returns the AP state at `index`, or `None` if the index is out of range or the slot is
    /// unused.
    #[cfg(test)]
    pub fn get_ap_state_by_index(&self, index: usize) -> Option<ApState> {
        let slot = self.slots().get(index)?;
        if slot.is_used() { Some(ApState::from(slot.state.load(Ordering::Acquire))) } else { None }
    }

    /// Returns the state of the AP with APIC ID `cpu_id`.
    #[cfg(test)]
    pub fn get_ap_state(&self, cpu_id: u32) -> Option<ApState> {
        let index = self.find_slot(cpu_id)?;
        let slot = self.slots().get(index)?;
        Some(ApState::from(slot.state.load(Ordering::Acquire)))
    }

    /// Sets the state of an AP.
    ///
    /// Returns `false` without changing anything if `cpu_id` is not registered or is the BSP.
    pub fn set_ap_state(&self, cpu_id: u32, state: ApState) -> bool {
        let index = match self.find_slot(cpu_id) {
            Some(idx) => idx,
            None => return false,
        };

        let Some(slot) = self.slots().get(index) else {
            return false;
        };

        if slot.is_bsp.load(Ordering::Acquire) != 0 {
            log::warn!("Attempted to change BSP state, ignoring");
            return false;
        }

        slot.state.store(state as u8, Ordering::Release);
        true
    }

    /// Calls `f` with the APIC ID of each registered AP.
    #[cfg(test)]
    pub fn for_each_ap<F: FnMut(u32)>(&self, mut f: F) {
        for slot in self.slots() {
            if let Some(cpu_id) = slot.get_cpu_id()
                && slot.is_bsp.load(Ordering::Acquire) == 0
            {
                f(cpu_id);
            }
        }
    }

    /// Returns the number of registered APs in `state`.
    pub fn count_aps_in_state(&self, state: ApState) -> usize {
        let mut count = 0;
        for slot in self.slots() {
            if slot.is_used()
                && slot.is_bsp.load(Ordering::Acquire) == 0
                && slot.state.load(Ordering::Acquire) == state as u8
            {
                count += 1;
            }
        }
        count
    }

    /// Releases every registered AP from the exit barrier by signaling its semaphore. Called by
    /// the BSP.
    pub fn release_all_aps(&self) {
        for slot in self.slots() {
            if slot.is_used() && slot.is_bsp.load(Ordering::Acquire) == 0 {
                sem_signal(&slot.run);
            }
        }
    }

    /// Takes a pending exit barrier release for the AP at `cpu_index`, without blocking. Returns
    /// whether the AP was released. Called by an AP in the holding pen.
    pub fn take_release_by_index(&self, cpu_index: usize) -> bool {
        let Some(slot) = self.slots().get(cpu_index) else {
            return false;
        };
        sem_try_take(&slot.run)
    }

    /// Tells the BSP that this AP has left the holding pen by signaling the BSP's semaphore.
    pub fn ack_exit_to_bsp(&self) {
        if let Some(bsp_id) = self.bsp_id()
            && let Some(index) = self.find_slot(bsp_id)
            && let Some(slot) = self.slots().get(index)
        {
            sem_signal(&slot.run);
        }
    }

    /// Collects up to `ap_count` holding pen exit acknowledgements, giving up after `timeout_us`
    /// microseconds, and returns how many arrived.
    ///
    /// The wait is bounded because the APs may be running code the supervisor does not control.
    /// The caller decides what a short count means.
    pub fn wait_for_ap_exit_acks(&self, ap_count: usize, timeout_us: u64) -> usize {
        if ap_count == 0 {
            return 0;
        }

        let bsp_index = match self.bsp_id().and_then(|bsp_id| self.find_slot(bsp_id)) {
            Some(index) => index,
            None => return 0,
        };
        let Some(slot) = self.slots().get(bsp_index) else {
            return 0;
        };

        let mut acknowledged = 0;
        super::perf_timer::spin_until(timeout_us, || {
            while acknowledged < ap_count && sem_try_take(&slot.run) {
                acknowledged += 1;
            }
            acknowledged == ap_count
        });
        acknowledged
    }
}

impl Default for CpuManager {
    fn default() -> Self {
        Self::new()
    }
}
