//! Multiprocessor (MP) support for Patina.
//!
//! Provides the architecture abstraction for multiprocessor (MP) support.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#[cfg(target_arch = "x86_64")] // Will use for aarch64 too.
mod control;

#[cfg(target_arch = "x86_64")]
mod x64;

mod work;

use patina::{component::service::perf_timer::ArchTimerFunctionality, error::EfiError};
pub use work::ApWorkItem;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        pub use x64::{ApContext, MpSupport};
    } else {
        mod stub;
        pub use stub::{ApContext, MpSupport};
    }
}

/// Dispatch eligibility of an AP, derived from its dispatch state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessorState {
    /// Not started yet. Cannot accept work.
    NotStarted,
    /// Idle or finished with previous work. Ready for a new dispatch.
    Ready,
    /// Still running a prior dispatch. Cannot accept work.
    Busy,
    /// Excluded from dispatch, either by request or after failing to complete a
    /// prior dispatch within its timeout.
    Disabled,
}

/// Identity of a processor known to the MP dispatcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Processor {
    /// The bootstrap processor.
    Bsp,
    /// An application processor identified by its zero-based AP index.
    Ap(usize),
}

/// Handoff record for a single logical processor.
#[derive(Clone, Copy)]
pub struct ProcessorHandOff {
    /// Architecture specific ID of the processor.
    pub processor_id: u32,
    /// Whether the processor passed its built-in self-test in PEI.
    pub healthy: bool,
    /// Address of the word the AP monitors for the wake-up signal value.
    pub startup_signal_address: u64,
    /// Address of the slot into which the BSP writes the AP entry-point address.
    pub startup_procedure_address: u64,
}

/// Multiprocessor handoff, gathered from the platform HOBs.
pub struct MpHandOffInfo<'a> {
    /// Pointer width (in bytes) of the phase that produced the handoff.
    pub wait_loop_execution_mode: u32,
    /// Value the BSP writes to each AP's startup signal address to wake it.
    pub startup_signal_value: u32,
    /// Per-processor handoff records (including the BSP entry)
    pub processors: &'a [ProcessorHandOff],
}

/// Architecture abstraction interface for multiprocessor management.
///
/// This trait explicitly only operates on the APs, and not the BSP. The BSP
/// is not included in dispatching, counts, indexes, etc.
pub trait MpDispatcher: Sized {
    /// The number of pages required for runtime parking of the APs.
    /// This must be provided during instantiation of the dispatcher.
    const PARK_PAGES: usize = 0;

    /// Required alignment of the runtime park allocation.
    const PARK_ALIGNMENT: usize = 1;

    /// Index of the executable page within the park allocation.
    const PARK_CODE_PAGE: usize = 0;

    /// Index of the writable data and emergency-stack page within the park allocation.
    const PARK_DATA_PAGE: usize = 0;

    /// Builds architecture-specific runtime parking state while the allocation is writable.
    fn prepare_park_pages(park_pages: &mut [u8]) -> Result<(), EfiError>;

    /// Creates and starts multiprocessor support, migrating each AP out of its
    /// handoff loop into the Rust dispatch loop.
    fn initialize(
        contexts: &'static mut [ApContext],
        timer: &'static dyn ArchTimerFunctionality,
        handoff: Option<MpHandOffInfo<'_>>,
        park_pages: &'static [u8],
    ) -> Result<Self, EfiError>;

    /// Number of application processors known to the dispatcher.
    fn ap_count(&self) -> usize;

    /// Number of application processors that have reported as started.
    fn started_ap_count(&self) -> usize;

    /// Number of application processors that are started and not disabled.
    fn enabled_ap_count(&self) -> usize;

    /// Enables or disables the AP at `index` for dispatch, optionally recording a
    /// new health status. Returns `false` for an out-of-range AP index.
    ///
    /// Disabling an AP that is still running a dispatch fences it off immediately;
    /// it becomes eligible again only if it is re-enabled *and* its work completes.
    fn set_ap_enabled(&self, index: usize, enabled: bool, healthy: Option<bool>) -> bool;

    /// Whether the AP at `index` is currently considered healthy.
    fn ap_healthy(&self, index: usize) -> bool;

    /// Identity of the calling processor.
    fn who_am_i(&self) -> Option<Processor>;

    /// Architectural processor ID of the BSP.
    fn bsp_processor_id(&self) -> u32;

    /// Architectural processor ID recorded for the AP at `index`.
    fn ap_processor_id(&self, index: usize) -> Option<u32>;

    /// Whether the AP at `index` has completed the dispatch identified by `work_id`
    fn ap_finished(&self, index: usize, work_id: u64) -> bool;

    /// Dispatch eligibility of the AP at `index`.
    fn ap_availability(&self, index: usize) -> ProcessorState;

    /// Publishes `work` to the AP at `index` so it runs on its next scheduling,
    /// returning the id identifying this dispatch (for use with
    /// [`MpDispatcher::ap_finished`]). Returns `None` for an out-of-range index or
    /// an AP that cannot currently accept work.
    fn signal_ap(&self, index: usize, work: ApWorkItem) -> Option<u64>;

    /// Terminates the dispatch identified by `work_id` on the AP at `index` and
    /// waits until the processor has returned to its dispatch loop.
    ///
    /// Returns `true` when recovery completes within the architecture-specific
    /// health deadline. A `false` result still guarantees termination, but the
    /// caller should consider the processor unhealthy and unavailable.
    fn abort_ap(&self, index: usize, work_id: u64) -> bool;

    /// Quiesce every AP for OS handoff at `ExitBootServices`, leaving them in the
    /// state the OS's own bring-up expects to find them in.
    ///
    /// This is terminal: the APs cannot be dispatched to again.
    fn park(&self);

    /// Publishes architecture-specific state from the BSP to the AP at `index` and
    /// dispatches it, returning the id identifying the dispatch. On x64 this
    /// replicates the BSP's MTRR settings.
    ///
    /// The caller owns scheduling: it selects the processor, holds the dispatch lock
    /// across this call, and waits on the returned id.
    fn sync_ap(&self, index: usize) -> Option<u64>;
}
