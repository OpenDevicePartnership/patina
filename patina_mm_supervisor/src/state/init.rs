//! Boot and Per-Core Synchronization State
//!
//! Holds [`InitState`], the flags and type-erased entry-point handles the
//! supervisor uses while bringing cores online, and the one global instance of
//! it.
//!
//! The security-relevant half lives in [`super::security`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
};

use spin::Once;

use patina::MemoryRange;

/// Boot-time and per-core synchronization state for the MM Supervisor Core.
pub(crate) struct InitState {
    /// Physical address of the global `MmSupervisorCore` instance.
    supervisor: Once<NonZeroUsize>,
    /// Type-erased lookup for the processor ID at a dense CPU index.
    processor_id_lookup_fn: Once<fn(usize) -> Option<u64>>,
    /// Set once BSP one-time initialization has completed.
    bsp_init_complete: AtomicBool,
    /// Per-core initialized slots from the `PassDown` HOB.
    mm_initialized_buffer: Once<&'static [AtomicU8]>,
    /// Number of cores that have completed per-core initialization.
    per_core_init_count: AtomicU32,
    /// User module entry point discovered from the HOB list.
    user_entry_point: Once<u64>,
    /// Validated Init image base and length, copied before the producer's HOBs are reclaimed.
    init_module_region: Once<MemoryRange>,
    /// Set after the init image has been freed.
    init_module_freed: AtomicBool,
    /// MSEG base address discovered from the MSEG SMRAM HOB, if the platform
    /// publishes one. Programmed into `IA32_SMM_MONITOR_CTL` during per-core init.
    mseg_base: Once<u64>,
    /// Type-erased AP startup dispatch function (conformed for the platform).
    ap_startup_fn: Once<fn(u64, u64, u64) -> u64>,
    /// Set once `ExitBootServices` has been signaled.
    at_runtime: Once<()>,
    /// SMRR range derived during BSP initialization.
    smrr_range: Once<MemoryRange>,
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
            init_module_region: Once::new(),
            init_module_freed: AtomicBool::new(false),
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

    /// Stores the per-core initialized slots (one-time).
    pub(crate) fn set_mm_initialized_buffer(&self, buffer: &'static [AtomicU8]) {
        self.mm_initialized_buffer.call_once(|| buffer);
    }

    /// Returns the per-core initialized slots, if set.
    pub(crate) fn mm_initialized_buffer(&self) -> Option<&'static [AtomicU8]> {
        self.mm_initialized_buffer.get().copied()
    }

    /// Increments the per-core init count and returns the new value (`SeqCst`).
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

    /// Records the validated Init image allocation once.
    pub(crate) fn set_init_module_region(&self, range: MemoryRange) {
        self.init_module_region.call_once(|| range);
    }

    /// Returns the saved Init image range without accessing the HOB list.
    pub(crate) fn init_module_region(&self) -> Option<MemoryRange> {
        self.init_module_region.get().copied()
    }

    /// Returns whether the init image has been freed.
    pub(crate) fn is_init_module_freed(&self) -> bool {
        self.init_module_freed.load(Ordering::Acquire)
    }

    /// Marks successful completion of init image cleanup.
    pub(crate) fn mark_init_module_freed(&self) {
        self.init_module_freed.store(true, Ordering::Release);
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

    /// Marks the supervisor as having entered runtime (`ExitBootServices` signaled).
    pub(crate) fn mark_at_runtime(&self) -> bool {
        let first = !self.at_runtime.is_completed();
        self.at_runtime.call_once(|| ());
        first
    }

    /// Returns whether `ExitBootServices` has been signaled.
    pub(crate) fn is_at_runtime(&self) -> bool {
        self.at_runtime.is_completed()
    }

    /// Stores the SMRR range (one-time).
    pub(crate) fn set_smrr_range(&self, range: MemoryRange) {
        self.smrr_range.call_once(|| range);
    }

    /// Returns the SMRR range, if set.
    pub(crate) fn smrr_range(&self) -> Option<MemoryRange> {
        self.smrr_range.get().copied()
    }
}

/// Global boot/synchronization state instance.
static INIT_STATE: InitState = InitState::new();

/// Returns the global [`InitState`].
#[inline]
pub(crate) fn init_state() -> &'static InitState {
    &INIT_STATE
}
