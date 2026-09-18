//! Stub multiprocessor support for architectures without a full implementation.
//!
//! This provides a [`MpDispatcher`] so that crates depending on MP support compile
//! and link on architectures where the AP startup mechanism has not yet been
//! implemented (e.g. aarch64). Its constructor returns [`EfiError::Unsupported`],
//! so an instance is never created and the remaining methods are unreachable.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::num::NonZeroUsize;

use patina::{
    component::service::{memory::MemoryManager, perf_timer::ArchTimerFunctionality},
    error::EfiError,
};

use super::{ApWorkItem, MpDispatcher, MpHandOffInfo, Processor, ProcessorState};

/// An MP manager placeholder for architectures without AP startup support. It's construction
/// will always fail so its functions are not reachable. It is a empty enum so it can't be instantiated.
pub enum MpSupport {}

/// Placeholder processor context for architectures without MP support.
#[derive(Default)]
pub struct ApContext;

impl ApContext {
    /// No AP stack is required because this implementation cannot start APs.
    pub const STACK_SIZE: usize = 0;

    /// Records an AP stack for a future architecture implementation.
    ///
    /// # Safety
    ///
    /// The stack must remain valid for the lifetime of the MP subsystem.
    pub unsafe fn set_stack_top(&mut self, _stack_top: NonZeroUsize) -> Result<(), EfiError> {
        Ok(())
    }
}

impl MpDispatcher for MpSupport {
    fn initialize(
        _memory_manager: &dyn MemoryManager,
        _timer: &'static dyn ArchTimerFunctionality,
    ) -> Result<Self, EfiError> {
        Err(EfiError::Unsupported)
    }

    fn setup_aps(
        &mut self,
        _contexts: &'static mut [ApContext],
        _handoff: Option<MpHandOffInfo<'_>>,
    ) -> Result<(), EfiError> {
        unreachable!()
    }

    fn ap_count(&self) -> usize {
        unreachable!();
    }

    fn started_ap_count(&self) -> usize {
        unreachable!()
    }

    fn enabled_ap_count(&self) -> usize {
        unreachable!()
    }

    fn set_ap_enabled(&self, _index: usize, _enabled: bool, _healthy: Option<bool>) -> bool {
        unreachable!()
    }

    fn ap_healthy(&self, _index: usize) -> bool {
        unreachable!()
    }

    fn who_am_i(&self) -> Option<Processor> {
        unreachable!();
    }

    fn bsp_processor_id(&self) -> u32 {
        unreachable!();
    }

    fn ap_processor_id(&self, _index: usize) -> Option<u32> {
        unreachable!();
    }

    fn ap_finished(&self, _index: usize, _work_id: u64) -> bool {
        unreachable!();
    }

    fn ap_availability(&self, _index: usize) -> ProcessorState {
        unreachable!();
    }

    fn signal_ap(&self, _index: usize, _work: ApWorkItem) -> Option<u64> {
        unreachable!();
    }

    fn abort_ap(&self, _index: usize, _work_id: u64) -> bool {
        unreachable!();
    }

    fn park(&self) {
        unreachable!();
    }

    fn sync_aps(&self) -> bool {
        unreachable!()
    }
}
