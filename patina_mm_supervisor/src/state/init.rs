//! Boot and Per-Core Synchronization State
//!
//! Holds [`InitState`], the flags and type-erased entry-point handles the supervisor uses while
//! bringing cores online, and the one global instance of it.
//!
//! Every field is written once, or counts up, and is read by cores that did not write it. The
//! setters are therefore one-time or atomic rather than plain assignments, so a core arriving
//! late cannot overwrite what an earlier core established.
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

use crate::memory::mmram::MmramRegion;

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
    init_module_region: Once<(u64, u64)>,
    /// Set after the init image has been freed.
    init_module_freed: AtomicBool,
    /// MSEG base address discovered from the MSEG SMRAM HOB, if the platform
    /// publishes one. Programmed into `IA32_SMM_MONITOR_CTL` during per-core init.
    mseg_base: Once<u64>,
    /// Type-erased AP startup dispatch function (conformed for the platform).
    ap_startup_fn: Once<fn(u64, u64, u64) -> u64>,
    /// Set once `ExitBootServices` has been signaled.
    at_runtime: Once<()>,
    /// SMRR region (base and size) derived during BSP initialization.
    smrr_range: Once<MmramRegion>,
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
    pub(crate) fn set_init_module_region(&self, base: u64, size: u64) {
        self.init_module_region.call_once(|| (base, size));
    }

    /// Returns the saved Init image base and length without accessing the HOB list.
    pub(crate) fn init_module_region(&self) -> Option<(u64, u64)> {
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

    /// Stores the SMRR region (one-time).
    pub(crate) fn set_smrr_range(&self, range: MmramRegion) {
        self.smrr_range.call_once(|| range);
    }

    /// Returns the SMRR region, if set.
    pub(crate) fn smrr_range(&self) -> Option<MmramRegion> {
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
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
        assert!(state.mm_initialized_buffer().is_none());
        assert_eq!(state.per_core_init_count(), 0);
        assert!(state.user_entry_point().is_none());
        assert!(state.init_module_region().is_none());
        assert!(!state.is_init_module_freed());
        assert!(state.mseg_base().is_none());
        assert!(state.ap_startup_fn().is_none());
        assert!(!state.is_at_runtime());
        assert!(state.smrr_range().is_none());
    }

    #[test]
    fn test_init_module_freed_flag() {
        let state = InitState::new();
        assert!(!state.is_init_module_freed());
        state.mark_init_module_freed();
        assert!(state.is_init_module_freed());
    }

    #[test]
    fn test_init_module_region_is_recorded_once() {
        let state = InitState::new();
        state.set_init_module_region(0x1000, 0x3000);
        state.set_init_module_region(0x8000, 0x9000);
        assert_eq!(state.init_module_region(), Some((0x1000, 0x3000)));
        assert!(!state.is_init_module_freed());
    }

    #[test]
    fn test_init_state_processor_id_lookup_is_initialized_once() {
        let state = InitState::new();

        state.set_processor_id_lookup_fn(processor_id_lookup);
        assert_eq!(state.processor_id_lookup_fn().unwrap()(3), Some(0x13));

        state.set_processor_id_lookup_fn(replacement_processor_id_lookup);
        assert_eq!(state.processor_id_lookup_fn().unwrap()(3), Some(0x13));

        // The replacement is a working lookup in its own right, so the retained value above
        // is the result of the one-time semantics rather than of an inert replacement.
        let fresh = InitState::new();
        fresh.set_processor_id_lookup_fn(replacement_processor_id_lookup);
        assert_eq!(fresh.processor_id_lookup_fn().unwrap()(3), None);
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

    #[test]
    fn test_init_state_one_time_values_ignore_later_writes() {
        static INITIALIZED_BUFFER: [AtomicU8; 2] = [AtomicU8::new(0), AtomicU8::new(1)];
        static REPLACEMENT_BUFFER: [AtomicU8; 1] = [AtomicU8::new(2)];

        let state = InitState::new();
        let smrr = MmramRegion::new(0x6000_0000, 0x10_0000, false);

        state.set_mm_initialized_buffer(&INITIALIZED_BUFFER);
        state.set_mseg_base(0x7000_0000);
        state.set_smrr_range(smrr);

        state.set_mm_initialized_buffer(&REPLACEMENT_BUFFER);
        state.set_mseg_base(0x9000_0000);
        state.set_smrr_range(MmramRegion::new(0x9000_0000, 0x2000, true));

        let stored_buffer = state.mm_initialized_buffer().expect("initialized buffer should be stored");
        assert!(core::ptr::eq(stored_buffer, &INITIALIZED_BUFFER));
        assert_eq!(state.mseg_base(), Some(0x7000_0000));
        assert_eq!(state.smrr_range(), Some(smrr));
    }

    #[test]
    fn test_init_state_supervisor_address_is_recorded_once() {
        let state = InitState::new();
        let first = NonZeroUsize::new(0x1000).unwrap();
        let second = NonZeroUsize::new(0x2000).unwrap();

        assert!(state.set_supervisor(first));
        assert!(!state.set_supervisor(second));
        assert_eq!(state.supervisor(), Some(first));
    }

    #[test]
    fn test_init_state_entry_points_are_recorded_once() {
        let state = InitState::new();

        state.set_user_entry_point(0x4000);
        state.set_user_entry_point(0x5000);
        assert_eq!(state.user_entry_point(), Some(0x4000));

        state.set_ap_startup_fn(|a, b, c| a + b + c);
        state.set_ap_startup_fn(|_, _, _| 0);
        assert_eq!(state.ap_startup_fn().unwrap()(1, 2, 3), 6);
    }
}
