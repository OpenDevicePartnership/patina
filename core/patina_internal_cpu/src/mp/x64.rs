//! `x86_64` Multiprocessor (MP) startup support.
//!
//! Brings Application Processors (APs) online by migrating them out of the PEI
//! wait loop into the DXE dispatch loop.
//!
//! ## Context Array
//!
//! The [`ApContext`] array is allocated by the caller and borrowed by
//! [`MpSupport`]. Every entry represents an AP; the BSP has no context.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{
    num::{NonZeroU64, NonZeroUsize},
    sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

use patina::{
    bit,
    component::service::{
        memory::{AccessType, AllocationOptions, MemoryManager, PageAllocationStrategy},
        perf_timer::ArchTimerFunctionality,
    },
    error::EfiError,
    uefi::memory::EfiMemoryType,
    uefi_pages_to_size,
};

use super::control::{self, ApStateMachine};
use super::{ApWorkItem, MpDispatcher, MpHandOffInfo, Processor, ProcessorState};

mod ap_bootstrap;
mod ap_setup;
mod apic;
mod cpu_state;
mod park;

/// Sentinel value indicating an unused entry in [`ApContext::apic_id`].
const APIC_ID_INVALID: u32 = 0xFFFF_FFFF;

/// Window for APs to migrate into the DXE dispatch loop after being signaled.
const AP_STARTUP_TIMEOUT_US: u64 = 100_000;

/// Maximum time to wait for an INIT-aborted AP to re-enter its dispatch loop.
const AP_ABORT_TIMEOUT_US: u64 = 100_000;
const INIT_TO_SIPI_DELAY_US: u64 = 10_000;
const SIPI_DELAY_US: u64 = 200;

/// Maximum time to wait for started APs to enter the reserved park loop.
const AP_PARK_TIMEOUT_US: u64 = 100_000;

/// Required wait loop mode for APs to be considered ready.
const REQUIRED_WAIT_LOOP_MODE: u32 = 8;

/// Per-processor context structure.
#[repr(C)]
pub struct ApContext {
    /// Stack top for this AP. Set by the BSP before AP startup.
    stack_top: AtomicU64,
    /// APIC ID of this AP. The AP will set this as they reserve indices.
    apic_id: AtomicU32,
    /// Dispatch state machine plus the work slot it guards for this AP.
    sm: ApStateMachine,
    /// Task state used to reset the stack before terminal double-fault handling.
    tss: crate::gdt::TaskStateSegment,
    /// GDT containing this AP's unique TSS descriptor.
    gdt: crate::gdt::ApGdt,
    /// Descriptor-table operand used by the assembly entry path.
    gdtr: crate::gdt::DescriptorTablePointer,
}

impl ApContext {
    /// Required usable stack size for each AP context.
    pub const STACK_SIZE: usize = 0x8000;

    const fn new() -> Self {
        Self {
            stack_top: AtomicU64::new(0),
            apic_id: AtomicU32::new(APIC_ID_INVALID),
            sm: ApStateMachine::new(),
            tss: crate::gdt::TaskStateSegment::new(0),
            gdt: crate::gdt::ApGdt::new(),
            gdtr: crate::gdt::DescriptorTablePointer { limit: 0, base: 0 },
        }
    }

    fn initialize_descriptor_tables(&mut self) {
        let stack_top = self.stack_top.load(Ordering::Relaxed);
        self.tss = crate::gdt::TaskStateSegment::new(stack_top);
        self.gdt.initialize(core::ptr::addr_of!(self.tss) as u64);
        self.gdtr = self.gdt.descriptor();
    }

    fn assign_processor(&mut self, processor: &super::ProcessorHandOff) {
        self.apic_id.store(processor.processor_id, Ordering::Relaxed);
        self.sm.set_healthy(processor.healthy);
    }

    /// Records the top of the stack provisioned for this AP.
    ///
    /// # Safety
    ///
    /// `stack_top` must be 16-byte aligned and identify the top of writable,
    /// exclusively owned stack storage that remains valid for the lifetime of
    /// the MP subsystem.
    pub unsafe fn set_stack_top(&mut self, stack_top: NonZeroUsize) -> Result<(), EfiError> {
        if !stack_top.get().is_multiple_of(16) {
            return Err(EfiError::InvalidParameter);
        }
        self.stack_top.store(stack_top.get() as u64, Ordering::Relaxed);
        Ok(())
    }
}

impl Default for ApContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Multiprocessor support for `x86_64`.
pub struct MpSupport {
    apic: apic::Apic,
    contexts: &'static [ApContext],
    bsp_processor_id: u32,
    timer: &'static dyn ArchTimerFunctionality,
    perf_frequency: NonZeroU64,
    startup_vector: u8,
    shutting_down: AtomicBool,
}

impl MpSupport {
    fn validate_handoff(&self, handoff: &MpHandOffInfo<'_>) -> bool {
        Self::validate_handoff_for(handoff, self.apic.current_apic_id(), self.apic.max_apic_id())
    }

    fn parse_handoff<'a>(&self, handoff: Option<MpHandOffInfo<'a>>) -> Option<MpHandOffInfo<'a>> {
        let handoff = handoff.filter(|handoff| {
            if handoff.wait_loop_execution_mode != REQUIRED_WAIT_LOOP_MODE {
                log::error!(
                    "MP handoff wait-loop execution mode is {} bytes; x86_64 requires processors handed off in \
                     64-bit mode ({REQUIRED_WAIT_LOOP_MODE} bytes). Continuing with the BSP only.",
                    handoff.wait_loop_execution_mode
                );
                return false;
            }
            if handoff.processors.len() < 2 {
                log::warn!(
                    "MP handoff described {} processor(s); continuing with the BSP only.",
                    handoff.processors.len()
                );
                return false;
            }
            self.validate_handoff(handoff)
        });

        if handoff.is_none() {
            log::warn!("No usable processor handoff. MP Services will report a single processor!");
        }
        handoff
    }

    fn validate_handoff_for(handoff: &MpHandOffInfo<'_>, bsp_apic_id: u32, max_apic_id: u32) -> bool {
        let mut bsp_found = false;

        for (index, processor) in handoff.processors.iter().enumerate() {
            if processor.processor_id == bsp_apic_id {
                bsp_found = true;
            }

            if processor.processor_id > max_apic_id {
                log::error!("MP handoff processor {index} has an APIC ID that is invalid in the active APIC mode");
                return false;
            }

            if handoff.processors.iter().take(index).any(|prior| prior.processor_id == processor.processor_id) {
                log::error!("MP handoff contains duplicate APIC ID {:#x}", processor.processor_id);
                return false;
            }

            if processor.processor_id != bsp_apic_id
                && (processor.startup_signal_address == 0
                    || !processor.startup_signal_address.is_multiple_of(core::mem::align_of::<u32>() as u64)
                    || processor.startup_procedure_address == 0
                    || !processor.startup_procedure_address.is_multiple_of(core::mem::align_of::<u64>() as u64))
            {
                log::error!("MP handoff processor {index} has invalid startup slot addresses");
                return false;
            }
        }

        if !bsp_found {
            log::error!("MP handoff does not contain an entry for BSP APIC ID {bsp_apic_id:#x}");
            return false;
        }
        true
    }

    fn prepare_contexts<'a>(
        contexts: &'a mut [ApContext],
        processors: &[super::ProcessorHandOff],
        bsp_processor_id: u32,
    ) -> Result<&'a mut [ApContext], EfiError> {
        let ap_count = processors.len().saturating_sub(1);
        if processors.iter().filter(|processor| processor.processor_id != bsp_processor_id).count() != ap_count {
            log::error!("MP handoff must contain exactly one BSP");
            return Err(EfiError::InvalidParameter);
        }
        let context_count = contexts.len();
        let contexts = contexts.get_mut(..ap_count).ok_or_else(|| {
            log::error!("MP support requires {ap_count} AP contexts but only {context_count} were provided");
            EfiError::InvalidParameter
        })?;

        if contexts.iter().any(|ctx| ctx.stack_top.load(Ordering::Relaxed) == 0) {
            log::error!("Every AP context must have a provisioned stack");
            return Err(EfiError::InvalidParameter);
        }

        for context in contexts.iter_mut() {
            context.initialize_descriptor_tables();
        }

        let ap_handoffs = || processors.iter().filter(|p| p.processor_id != bsp_processor_id);
        let assigned = contexts.iter().zip(ap_handoffs()).count();
        for (ctx, processor) in contexts.iter_mut().zip(ap_handoffs()) {
            ctx.assign_processor(processor);
        }

        if assigned < contexts.len() {
            log::warn!("Handoff described {assigned} AP(s) but {} context slot(s) exist", contexts.len());
        }

        Ok(contexts)
    }

    /// Spins until `done` returns true or `timeout_us` microseconds elapse.
    #[inline(never)]
    fn try_for(&self, timeout_us: u64, mut done: impl FnMut() -> bool) -> bool {
        let ticks = timeout_us.saturating_mul(self.perf_frequency.get()).div_ceil(1_000_000);
        let start = self.timer.cpu_count();
        while self.timer.cpu_count().wrapping_sub(start) < ticks {
            ap_setup::check_for_failures();
            if done() {
                return true;
            }
            core::hint::spin_loop();
        }
        ap_setup::check_for_failures();
        done()
    }

    fn is_monitor_supported() -> bool {
        (cpu_state::cpuid(1, 0).ecx & bit!(3)) != 0
    }

    fn ap_run_dispatch_loop(ctx: &'static ApContext, apic: &apic::Apic) {
        apic.mask_local_interrupts();

        let Some(ap) = ctx.sm.start() else {
            Self::fail_ap(ap_setup::FAILURE_START_REJECTED, 0);
        };
        ap_setup::increment_started_count();

        let idle = if Self::is_monitor_supported() { monitor_wait } else { spin_wait };
        ap.run_dispatch_loop(cpu_state::flush_tlb, idle);
    }

    fn fail_ap(reason: u32, detail: u64) -> ! {
        // SAFETY: The assembly routine follows the EFIAPI register convention and never returns.
        unsafe { ap_setup::ap_record_failure(reason, detail, apic::Apic::current().current_apic_id()) }
    }

    fn apply_mtrrs_or_fail() {
        if !cpu_state::apply() {
            Self::fail_ap(ap_setup::FAILURE_MTRR_SETUP, 0);
        }
    }

    fn setup_aps_with(
        &mut self,
        contexts: &'static mut [ApContext],
        handoff: Option<MpHandOffInfo<'_>>,
    ) -> Result<(), EfiError> {
        // Without a usable handoff there is still a BSP to describe, so the protocol
        // is published for a uniprocessor system rather than withheld entirely.
        let processors = handoff.as_ref().map(|handoff| handoff.processors).unwrap_or_default();
        let startup_signal_value = handoff.as_ref().map_or(0, |handoff| handoff.startup_signal_value);
        log::info!("BSP APIC ID: {:#x}", self.bsp_processor_id);
        let ap_handoffs = || processors.iter().filter(|p| p.processor_id != self.bsp_processor_id);
        self.contexts = Self::prepare_contexts(contexts, processors, self.bsp_processor_id)?;
        ap_setup::setup(self.contexts);

        // Wake each AP by writing the entry point into its handoff procedure
        // slot and raising its startup signal.
        let ap_count = self.contexts.len();
        if ap_count == 0 {
            log::info!("MP Services initialized: uniprocessor system (BSP only)");
            return Ok(());
        }

        log::info!("Waking APs via handoff...");
        let start_timestamp = self.timer.cpu_count();
        for p in ap_handoffs() {
            // SAFETY: the addresses come from the handoff for APs still parked
            // in their wait loop. `wake_ap` publishes the entry before the signal.
            unsafe {
                ap_setup::wake_ap(p.startup_procedure_address, p.startup_signal_address, startup_signal_value);
            }
        }

        // Wait for all APs to migrate into the dispatch loop.
        self.try_for(AP_STARTUP_TIMEOUT_US, || self.started_ap_count() == ap_count);
        ap_setup::check_for_failures();
        let started_count = self.started_ap_count();
        let ap_start_time =
            (self.timer.cpu_count().saturating_sub(start_timestamp) * 1_000_000) / self.perf_frequency.get();

        log::info!("MP Services initialized: {started_count}/{ap_count} APs started in {ap_start_time} us");
        Ok(())
    }

    fn reset_ap(&self, ctx: &ApContext) -> bool {
        let was_started = ctx.sm.state() != ProcessorState::NotStarted;
        let apic_id = ctx.apic_id.load(Ordering::Relaxed);
        self.apic.send_init(apic_id);
        self.try_for(INIT_TO_SIPI_DELAY_US, || false);
        if was_started && !ap_setup::decrement_started_count() {
            ctx.sm.set_healthy(false);
            return false;
        }
        ctx.sm.reset();
        // SAFETY: INIT delivery has completed and the architectural settle time
        // has elapsed, so this AP is in wait-for-SIPI and cannot reload TR.
        unsafe { ctx.gdt.reset_tss_descriptor(core::ptr::addr_of!(ctx.tss) as u64) };
        self.apic.send_startup(apic_id, self.startup_vector);
        self.try_for(SIPI_DELAY_US, || false);
        self.apic.send_startup(apic_id, self.startup_vector);
        self.try_for(SIPI_DELAY_US, || false);
        let recovered = self.try_for(AP_ABORT_TIMEOUT_US, || ctx.sm.state() == ProcessorState::Ready);
        if !recovered {
            ctx.sm.set_healthy(false);
        }
        recovered
    }
}

impl MpDispatcher for MpSupport {
    fn initialize(
        memory_manager: &dyn MemoryManager,
        timer: &'static dyn ArchTimerFunctionality,
    ) -> Result<Self, EfiError> {
        let bootstrap_allocation = memory_manager
            .allocate_zero_pages(
                ap_bootstrap::PAGE_COUNT,
                AllocationOptions::new().with_strategy(PageAllocationStrategy::MaxAddress(ap_bootstrap::MAX_ADDRESS)),
            )
            .map_err(|e| {
                log::error!("Failed to allocate the AP bootstrap page below 1MB: {e:?}");
                EfiError::OutOfResources
            })?;

        let bootstrap_page = bootstrap_allocation.leak_as_slice::<u8>();
        let bootstrap_base = bootstrap_page.as_mut_ptr();
        ap_bootstrap::prepare(bootstrap_page)?;
        let startup_vector = ap_bootstrap::startup_vector(bootstrap_base as usize, bootstrap_page.len())?;
        // SAFETY: The complete range is the bootstrap allocation initialized above.
        unsafe {
            memory_manager.set_page_attributes(bootstrap_base as usize, 1, AccessType::ReadExecute, None).map_err(
                |e| {
                    log::error!("Failed to make the AP bootstrap executable: {e:?}");
                    EfiError::DeviceError
                },
            )?;
        }

        let park_allocation = memory_manager
            .allocate_zero_pages(
                park::PAGE_COUNT,
                AllocationOptions::new()
                    .with_alignment(park::ALIGNMENT)
                    .with_memory_type(EfiMemoryType::ReservedMemoryType),
            )
            .map_err(|e| {
                log::error!("Failed to allocate AP park pages: {e:?}");
                EfiError::OutOfResources
            })?;
        let park_pages = park_allocation.leak_as_slice::<u8>();
        let park_base = park_pages.as_mut_ptr();
        park::prepare(park_pages)?;
        // SAFETY: The complete range is the reserved park allocation initialized above.
        unsafe {
            memory_manager
                .set_page_attributes(park_base as usize, park::PAGE_COUNT, AccessType::ReadOnly, None)
                .map_err(|e| {
                    log::error!("Failed to make AP park state read-only: {e:?}");
                    EfiError::DeviceError
                })?;
            memory_manager
                .set_page_attributes(
                    park_base as usize + uefi_pages_to_size!(park::CODE_PAGE_INDEX),
                    1,
                    AccessType::ReadExecute,
                    None,
                )
                .map_err(|e| {
                    log::error!("Failed to make the AP park loop executable: {e:?}");
                    EfiError::DeviceError
                })?;
            memory_manager
                .set_page_attributes(
                    park_base as usize + uefi_pages_to_size!(park::DATA_PAGE_INDEX),
                    1,
                    AccessType::ReadWrite,
                    None,
                )
                .map_err(|e| {
                    log::error!("Failed to make the AP park stack writable: {e:?}");
                    EfiError::DeviceError
                })?;
        }
        park::install(park_pages)?;

        let perf_frequency = NonZeroU64::new(timer.perf_frequency()).ok_or_else(|| {
            log::error!("MP Services requires a calibrated timer.");
            EfiError::Unsupported
        })?;

        let apic = apic::Apic::current();
        let bsp_processor_id = apic.current_apic_id();
        Ok(Self {
            apic,
            contexts: &[],
            bsp_processor_id,
            timer,
            perf_frequency,
            startup_vector,
            shutting_down: AtomicBool::new(false),
        })
    }

    fn setup_aps(
        &mut self,
        contexts: &'static mut [ApContext],
        handoff: Option<MpHandOffInfo<'_>>,
    ) -> Result<(), EfiError> {
        let handoff = self.parse_handoff(handoff);
        if handoff.is_some() && cpu_state::capture().is_err() {
            log::error!("Failed to prepare BSP MTRRs for AP startup");
            return Err(EfiError::DeviceError);
        }
        self.setup_aps_with(contexts, handoff)
    }

    fn ap_count(&self) -> usize {
        self.contexts.len()
    }

    fn started_ap_count(&self) -> usize {
        ap_setup::started_count() as usize
    }

    fn enabled_ap_count(&self) -> usize {
        self.contexts
            .iter()
            .filter(|ctx| !matches!(ctx.sm.state(), ProcessorState::NotStarted | ProcessorState::Disabled))
            .count()
    }

    fn set_ap_enabled(&self, index: usize, enabled: bool, healthy: Option<bool>) -> bool {
        let Some(ctx) = self.contexts.get(index) else { return false };

        ctx.sm.set_enabled(enabled);
        if enabled && !self.reset_ap(ctx) {
            ctx.sm.set_enabled(false);
            return false;
        }

        if let Some(healthy) = healthy {
            ctx.sm.set_healthy(healthy);
        }
        true
    }

    fn ap_healthy(&self, index: usize) -> bool {
        self.contexts.get(index).is_some_and(|ctx| ctx.sm.is_healthy())
    }

    fn who_am_i(&self) -> Option<Processor> {
        let apic_id = self.apic.current_apic_id();
        if apic_id == self.bsp_processor_id {
            Some(Processor::Bsp)
        } else {
            self.contexts.iter().position(|ctx| ctx.apic_id.load(Ordering::Relaxed) == apic_id).map(Processor::Ap)
        }
    }

    fn bsp_processor_id(&self) -> u32 {
        self.bsp_processor_id
    }

    fn ap_processor_id(&self, index: usize) -> Option<u32> {
        self.contexts.get(index).map(|ctx| ctx.apic_id.load(Ordering::Relaxed))
    }

    fn ap_finished(&self, index: usize, work_id: u64) -> bool {
        ap_setup::check_for_failures();
        self.contexts.get(index).is_some_and(|ctx| ctx.sm.is_finished(work_id))
    }

    fn ap_availability(&self, index: usize) -> ProcessorState {
        ap_setup::check_for_failures();
        self.contexts.get(index).map_or(ProcessorState::NotStarted, |ctx| ctx.sm.state())
    }

    fn signal_ap(&self, index: usize, work: ApWorkItem) -> Option<u64> {
        ap_setup::check_for_failures();
        if self.shutting_down.load(Ordering::Acquire) {
            return None;
        }
        self.contexts.get(index).and_then(|ctx| ctx.sm.dispatch(work).ok())
    }

    fn abort_ap(&self, index: usize, work_id: u64) -> bool {
        ap_setup::check_for_failures();
        let Some(ctx) = self.contexts.get(index) else {
            return false;
        };

        if ctx.sm.is_finished(work_id) {
            return true;
        }

        self.reset_ap(ctx)
    }

    fn park(&self) {
        ap_setup::check_for_failures();
        self.shutting_down.store(true, Ordering::Release);

        let expected = self.started_ap_count();
        for ctx in self.contexts {
            ctx.sm.signal_exit();
        }

        self.try_for(AP_PARK_TIMEOUT_US, || park::parked_count() as usize == expected);
        ap_setup::check_for_failures();
        let parked = park::parked_count() as usize;
        if parked == expected {
            log::info!("Parked application processors: {parked}/{expected} acknowledged");
        } else {
            log::error!("Timed out parking application processors: {parked}/{expected} acknowledged");
            debug_assert!(parked == expected);
        }
    }

    fn sync_aps(&self) -> bool {
        ap_setup::check_for_failures();
        if self.shutting_down.load(Ordering::Acquire) {
            return false;
        }

        if self.contexts.iter().any(|ctx| ctx.sm.state() == ProcessorState::Busy) {
            return false;
        }

        let Ok(supported) = cpu_state::capture() else {
            log::error!("Failed to capture BSP MTRRs for AP synchronization");
            return false;
        };
        if !supported {
            return true;
        }

        let start_ts = self.timer.cpu_count();
        let work = ApWorkItem::new(|()| Self::apply_mtrrs_or_fail(), &());
        for ctx in self.contexts.iter().filter(|ctx| ctx.sm.state() == ProcessorState::Ready) {
            if ctx.sm.dispatch(work).is_err() {
                return false;
            }
        }

        while self.contexts.iter().any(|ctx| ctx.sm.state() == ProcessorState::Busy) {
            ap_setup::check_for_failures();
            core::hint::spin_loop();
        }
        ap_setup::check_for_failures();
        let end_ts = self.timer.cpu_count();
        let elapsed_us = end_ts.wrapping_sub(start_ts) * 1_000_000 / self.perf_frequency.get();
        log::info!("AP MTRR synchronization completed in {elapsed_us} us.");
        true
    }
}

#[cfg_attr(coverage, coverage(off))]
fn park_ap() -> ! {
    // SAFETY: The park environment is installed before APs enter the dispatch loop.
    unsafe { park::ap_park() }
}

fn spin_wait(_state: &core::sync::atomic::AtomicU8) {
    core::hint::spin_loop();
}

/// Sleeps in `MWAIT` until the BSP writes this processor's lifecycle state.
#[cfg_attr(coverage, coverage(off))]
fn monitor_wait(state: &core::sync::atomic::AtomicU8) {
    // SAFETY: MONITOR arms address monitoring on the lifecycle state, a live
    // writable atomic in write-back memory.
    unsafe {
        core::arch::asm!(
            "monitor",
            in("rax") state.as_ptr(),
            in("rcx") 0,
            in("rdx") 0,
            options(nostack, preserves_flags),
        );
    }
    // Re-check after arming; sleep only while the state remains idle so work or
    // an exit request cannot be missed.
    if control::should_wait(state) {
        // SAFETY: MWAIT idles until a store to the monitored line (or another break
        // event); no side effects beyond resuming execution.
        unsafe {
            core::arch::asm!(
                "mwait",
                in("rax") 0,
                in("rcx") 0,
                options(nostack, preserves_flags),
            );
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use mockall::{Sequence, predicate::eq};
    use patina::component::service::{
        memory::{MemoryError, MockMemoryManager},
        perf_timer::MockArchTimerFunctionality,
    };
    use serial_test::serial;
    use std::boxed::Box;

    #[repr(align(16))]
    struct TestStack([u8; 64]);

    impl TestStack {
        fn top(&mut self) -> NonZeroUsize {
            NonZeroUsize::new(self.0.as_mut_ptr() as usize + self.0.len()).unwrap()
        }
    }

    fn processor_handoff(processor_id: u32) -> super::super::ProcessorHandOff {
        super::super::ProcessorHandOff {
            processor_id,
            healthy: true,
            startup_signal_address: 0x1000,
            startup_procedure_address: 0x2000,
        }
    }

    fn handoff(processors: &[super::super::ProcessorHandOff]) -> MpHandOffInfo<'_> {
        MpHandOffInfo { wait_loop_execution_mode: REQUIRED_WAIT_LOOP_MODE, startup_signal_value: 1, processors }
    }

    fn test_support(apic: apic::MockApicBackend, contexts: &'static [ApContext], shutting_down: bool) -> MpSupport {
        MpSupport {
            apic: apic::Apic::mock(apic),
            contexts,
            bsp_processor_id: 0,
            timer: Box::leak(Box::new(MockArchTimerFunctionality::new())),
            perf_frequency: NonZeroU64::new(1_000_000).unwrap(),
            startup_vector: 0,
            shutting_down: AtomicBool::new(shutting_down),
        }
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_dispatch_loop_runs_work_then_returns_on_exit() {
        let contexts = Box::leak(Box::new([ApContext::new()]));
        let context = &contexts[0];
        let work_count: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
        let mut apic = apic::MockApicBackend::new();
        apic.expect_mask_local_interrupts().once().return_const(());
        let apic = apic::Apic::mock(apic);
        ap_setup::setup(&[]);

        let controller = std::thread::spawn(move || {
            while context.sm.state() != ProcessorState::Ready {
                std::thread::yield_now();
            }
            context
                .sm
                .dispatch(ApWorkItem::new(
                    |count| {
                        count.fetch_add(1, Ordering::Relaxed);
                    },
                    work_count,
                ))
                .unwrap();
            while work_count.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            context.sm.signal_exit();
        });
        MpSupport::ap_run_dispatch_loop(context, &apic);
        controller.join().unwrap();

        assert_eq!(work_count.load(Ordering::Relaxed), 1);
        assert_eq!(ap_setup::started_count(), 1);
        assert_eq!(context.sm.state(), ProcessorState::Disabled);
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_setup_without_handoff_initializes_bsp_only() {
        let contexts = Box::leak(Box::new([ApContext::new()]));
        let mut support = test_support(apic::MockApicBackend::new(), &[], false);

        assert_eq!(support.setup_aps(contexts, None), Ok(()));
        assert_eq!(support.ap_count(), 0);
        assert_eq!(support.started_ap_count(), 0);
        assert_eq!(support.enabled_ap_count(), 0);
    }

    #[test]
    fn test_mp_support_initialize_maps_bootstrap_allocation_failure() {
        let mut memory = MockMemoryManager::new();
        memory.expect_allocate_zero_pages().once().returning(|_, _| Err(MemoryError::NoAvailableMemory));
        let timer = Box::leak(Box::new(MockArchTimerFunctionality::new()));

        assert!(matches!(MpSupport::initialize(&memory, timer), Err(EfiError::OutOfResources)));
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_parse_handoff_rejects_unusable_handoffs() {
        let wrong_mode_processors = [processor_handoff(0x10), processor_handoff(0x20)];
        let uniprocessor = [processor_handoff(0x10)];
        let unusable = [
            MpHandOffInfo {
                wait_loop_execution_mode: REQUIRED_WAIT_LOOP_MODE / 2,
                startup_signal_value: 1,
                processors: &wrong_mode_processors,
            },
            handoff(&uniprocessor),
        ];

        for handoff in unusable {
            let support = test_support(apic::MockApicBackend::new(), &[], false);
            assert!(support.parse_handoff(Some(handoff)).is_none());
        }

        let processors = [processor_handoff(0x10), processor_handoff(0x100)];
        let mut apic = apic::MockApicBackend::new();
        apic.expect_current_apic_id().once().return_const(0x10_u32);
        apic.expect_max_apic_id().once().return_const(0xFE_u32);
        let support = test_support(apic, &[], false);

        assert!(support.parse_handoff(Some(handoff(&processors))).is_none());
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_setup_wakes_valid_handoff_and_reports_timeout() {
        let procedure = Box::leak(Box::new(0_u64));
        let signal = Box::leak(Box::new(0_u32));
        let processors = [
            super::super::ProcessorHandOff {
                startup_signal_address: 0,
                startup_procedure_address: 0,
                ..processor_handoff(0x10)
            },
            super::super::ProcessorHandOff {
                startup_signal_address: core::ptr::from_mut(signal) as u64,
                startup_procedure_address: core::ptr::from_mut(procedure) as u64,
                ..processor_handoff(0x20)
            },
        ];
        let contexts = Box::leak(Box::new([ApContext::new()]));
        let stack = Box::leak(Box::new(TestStack([0; 64])));
        // SAFETY: the leaked stack is aligned, writable, and remains live with the leaked context.
        unsafe { contexts[0].set_stack_top(stack.top()) }.unwrap();

        let timer_count = AtomicU64::new(0);
        let mut timer = MockArchTimerFunctionality::new();
        timer
            .expect_cpu_count()
            .times(4)
            .returning(move || timer_count.fetch_add(AP_STARTUP_TIMEOUT_US + 1, Ordering::Relaxed));
        let mut support =
            MpSupport { timer: Box::leak(Box::new(timer)), ..test_support(apic::MockApicBackend::new(), &[], false) };
        support.bsp_processor_id = 0x10;

        assert_eq!(support.setup_aps_with(contexts, Some(handoff(&processors))), Ok(()));
        assert_eq!(*procedure, ap_setup::ap_entry_addr() as u64);
        assert_eq!(*signal, 1);
        assert_eq!(support.ap_count(), 1);
        assert_eq!(support.started_ap_count(), 0);
    }

    #[test]
    fn test_mp_support_try_for_reports_completion_and_timeout() {
        let mut timer = MockArchTimerFunctionality::new();
        timer.expect_cpu_count().times(2).return_const(10_u64);
        let support =
            MpSupport { timer: Box::leak(Box::new(timer)), ..test_support(apic::MockApicBackend::new(), &[], false) };
        let mut checks = 0;
        assert!(support.try_for(5, || {
            checks += 1;
            true
        }));
        assert_eq!(checks, 1);

        let mut sequence = Sequence::new();
        let mut timer = MockArchTimerFunctionality::new();
        timer.expect_cpu_count().once().in_sequence(&mut sequence).return_const(10_u64);
        timer.expect_cpu_count().once().in_sequence(&mut sequence).return_const(15_u64);
        let support =
            MpSupport { timer: Box::leak(Box::new(timer)), ..test_support(apic::MockApicBackend::new(), &[], false) };
        assert!(!support.try_for(5, || false));
    }

    #[test]
    fn ap_context_accepts_aligned_stack_top() {
        let mut context = ApContext::new();
        let stack_top = NonZeroUsize::new(0x20_000).unwrap();

        // SAFETY: This test only validates and records the address; it never starts an AP.
        assert_eq!(unsafe { context.set_stack_top(stack_top) }, Ok(()));
        assert_eq!(context.stack_top.load(Ordering::Relaxed), stack_top.get() as u64);
    }

    #[test]
    fn ap_context_rejects_misaligned_stack_top() {
        let mut context = ApContext::new();
        let stack_top = NonZeroUsize::new(0x20_008).unwrap();

        // SAFETY: This test only exercises validation and never starts an AP.
        assert_eq!(unsafe { context.set_stack_top(stack_top) }, Err(EfiError::InvalidParameter));
        assert_eq!(context.stack_top.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn mp_support_setup_rejects_insufficient_or_not_provisioned_contexts() {
        let duplicate_bsp = [processor_handoff(0x10), processor_handoff(0x10)];
        let mut duplicate_context = [ApContext::new()];
        assert!(matches!(
            MpSupport::prepare_contexts(&mut duplicate_context, &duplicate_bsp, 0x10),
            Err(EfiError::InvalidParameter)
        ));

        let processors = [processor_handoff(0x10), processor_handoff(0x20), processor_handoff(0x30)];
        let mut insufficient = [ApContext::new()];
        assert!(matches!(
            MpSupport::prepare_contexts(&mut insufficient, &processors, 0x10),
            Err(EfiError::InvalidParameter)
        ));

        let mut missing_stack = [ApContext::new(), ApContext::new()];
        let mut stack = TestStack([0; 64]);
        // SAFETY: `stack` is aligned, writable, exclusively owned, and outlives the context preparation.
        unsafe { missing_stack[0].set_stack_top(stack.top()) }.unwrap();
        assert!(matches!(
            MpSupport::prepare_contexts(&mut missing_stack, &processors, 0x10),
            Err(EfiError::InvalidParameter)
        ));

        let mut surplus = [ApContext::new(), ApContext::new()];
        let configured = MpSupport::prepare_contexts(&mut surplus, &[], 0x10).unwrap();
        assert!(configured.is_empty());
        assert!(surplus.iter().all(|context| context.apic_id.load(Ordering::Relaxed) == APIC_ID_INVALID));
    }

    #[test]
    fn mp_support_setup_assigns_aps_and_initializes_per_ap_descriptors() {
        let processors = [
            processor_handoff(0x20),
            processor_handoff(0x10),
            super::super::ProcessorHandOff { healthy: false, ..processor_handoff(0x30) },
        ];
        let mut contexts = [ApContext::new(), ApContext::new()];
        let mut stacks = [TestStack([0; 64]), TestStack([0; 64])];
        let stack_tops = [stacks[0].top(), stacks[1].top()];
        for (context, stack_top) in contexts.iter_mut().zip(stack_tops) {
            // SAFETY: each stack is aligned, writable, exclusively owned, and outlives context preparation.
            unsafe { context.set_stack_top(stack_top) }.unwrap();
        }

        let configured = MpSupport::prepare_contexts(&mut contexts, &processors, 0x10).unwrap();

        assert_eq!(configured[0].apic_id.load(Ordering::Relaxed), 0x20);
        assert_eq!(configured[1].apic_id.load(Ordering::Relaxed), 0x30);
        assert!(configured[0].sm.is_healthy());
        assert!(!configured[1].sm.is_healthy());
        assert_eq!(configured[0].tss.ist1(), stack_tops[0].get() as u64);
        assert_eq!(configured[1].tss.ist1(), stack_tops[1].get() as u64);
        for context in configured {
            let expected = context.gdt.descriptor();
            let expected_base = expected.base;
            let expected_limit = expected.limit;
            let base = context.gdtr.base;
            let limit = context.gdtr.limit;
            assert_eq!(base, expected_base);
            assert_eq!(limit, expected_limit);
        }
    }

    #[test]
    fn mp_support_accepts_valid_handoff_for_current_apic_mode() {
        let processors = [
            super::super::ProcessorHandOff {
                startup_signal_address: 0,
                startup_procedure_address: 0,
                ..processor_handoff(0x10)
            },
            processor_handoff(0x20),
        ];

        assert!(MpSupport::validate_handoff_for(&handoff(&processors), 0x10, u32::MAX));
    }

    #[test]
    fn mp_support_rejects_ambiguous_processor_identity() {
        let duplicate = [processor_handoff(0x10), processor_handoff(0x10)];
        assert!(!MpSupport::validate_handoff_for(&handoff(&duplicate), 0x10, u32::MAX));

        let missing_bsp = [processor_handoff(0x20), processor_handoff(0x30)];
        assert!(!MpSupport::validate_handoff_for(&handoff(&missing_bsp), 0x10, u32::MAX));

        let xapic_overflow = [processor_handoff(0x10), processor_handoff(0x100)];
        assert!(!MpSupport::validate_handoff_for(&handoff(&xapic_overflow), 0x10, 0xFE));
        assert!(MpSupport::validate_handoff_for(&handoff(&xapic_overflow), 0x10, u32::MAX - 1));

        let xapic_broadcast = [processor_handoff(0x10), processor_handoff(0xFF)];
        assert!(!MpSupport::validate_handoff_for(&handoff(&xapic_broadcast), 0x10, 0xFE));

        let x2apic_broadcast = [processor_handoff(0x10), processor_handoff(u32::MAX)];
        assert!(!MpSupport::validate_handoff_for(&handoff(&x2apic_broadcast), 0x10, u32::MAX - 1));
    }

    #[test]
    fn mp_support_rejects_invalid_ap_wake_slots() {
        for processor in [
            super::super::ProcessorHandOff { startup_signal_address: 0, ..processor_handoff(0x20) },
            super::super::ProcessorHandOff { startup_signal_address: 0x1001, ..processor_handoff(0x20) },
            super::super::ProcessorHandOff { startup_procedure_address: 0, ..processor_handoff(0x20) },
            super::super::ProcessorHandOff { startup_procedure_address: 0x2001, ..processor_handoff(0x20) },
        ] {
            let processors = [processor_handoff(0x10), processor];
            assert!(!MpSupport::validate_handoff_for(&handoff(&processors), 0x10, u32::MAX));
        }
    }

    #[test]
    fn mp_support_rejects_new_work_during_shutdown() {
        let contexts = Box::leak(Box::new([ApContext::new()]));
        assert!(contexts[0].sm.start().is_some());
        let support = test_support(apic::MockApicBackend::new(), contexts, true);
        static ARGUMENT: () = ();
        let work = ApWorkItem::new(|()| {}, &ARGUMENT);

        assert_eq!(support.signal_ap(0, work), None);
        assert!(!support.sync_aps());
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_reports_and_updates_processor_state() {
        let contexts = Box::leak(Box::new([ApContext::new(), ApContext::new()]));
        contexts[0].assign_processor(&processor_handoff(0x20));
        contexts[1].assign_processor(&super::super::ProcessorHandOff { healthy: false, ..processor_handoff(0x30) });
        let _consumer = contexts[0].sm.start().unwrap();
        let support = test_support(apic::MockApicBackend::new(), contexts, false);
        ap_setup::setup(&[]);

        assert_eq!(support.ap_count(), 2);
        assert_eq!(support.bsp_processor_id(), 0);
        assert_eq!(support.enabled_ap_count(), 1);
        assert_eq!(support.ap_processor_id(0), Some(0x20));
        assert_eq!(support.ap_processor_id(1), Some(0x30));
        assert_eq!(support.ap_processor_id(2), None);
        assert!(support.ap_healthy(0));
        assert!(!support.ap_healthy(1));
        assert!(!support.ap_healthy(2));
        assert_eq!(support.ap_availability(0), ProcessorState::Ready);
        assert_eq!(support.ap_availability(1), ProcessorState::NotStarted);
        assert_eq!(support.ap_availability(2), ProcessorState::NotStarted);
        assert!(support.abort_ap(0, 0));

        static ARGUMENT: () = ();
        let work_id = support.signal_ap(0, ApWorkItem::new(|()| {}, &ARGUMENT)).unwrap();
        assert_eq!(work_id, 1);
        assert_eq!(support.ap_availability(0), ProcessorState::Busy);
        assert!(support.ap_finished(0, 0));
        assert!(!support.ap_finished(0, work_id));
        assert!(!support.ap_finished(2, work_id));
        assert!(!support.sync_aps());
        assert_eq!(support.signal_ap(1, ApWorkItem::new(|()| {}, &ARGUMENT)), None);
        assert_eq!(support.signal_ap(2, ApWorkItem::new(|()| {}, &ARGUMENT)), None);
        assert!(!support.abort_ap(2, work_id));

        assert!(support.set_ap_enabled(0, false, Some(false)));
        assert!(!support.ap_healthy(0));
        assert_eq!(support.ap_availability(0), ProcessorState::Disabled);
        assert_eq!(support.enabled_ap_count(), 0);
        assert!(support.set_ap_enabled(1, false, Some(true)));
        assert!(support.ap_healthy(1));
        assert_eq!(support.ap_availability(1), ProcessorState::NotStarted);
        assert!(!support.set_ap_enabled(2, false, None));
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_park_stops_dispatch_and_requests_exit() {
        let contexts = Box::leak(Box::new([ApContext::new()]));
        let _consumer = contexts[0].sm.start().unwrap();
        let mut timer = MockArchTimerFunctionality::new();
        timer.expect_cpu_count().times(2).return_const(0_u64);
        let support = MpSupport {
            timer: Box::leak(Box::new(timer)),
            ..test_support(apic::MockApicBackend::new(), contexts, false)
        };
        ap_setup::setup(&[]);

        support.park();

        assert!(support.shutting_down.load(Ordering::Acquire));
        assert_eq!(support.ap_availability(0), ProcessorState::Disabled);
        static ARGUMENT: () = ();
        assert_eq!(support.signal_ap(0, ApWorkItem::new(|()| {}, &ARGUMENT)), None);
    }

    #[test]
    fn test_mp_support_identifies_bsp_ap_and_unknown_processor() {
        let mut sequence = Sequence::new();
        let mut apic = apic::MockApicBackend::new();
        apic.expect_current_apic_id().once().in_sequence(&mut sequence).return_const(0x10_u32);
        apic.expect_current_apic_id().once().in_sequence(&mut sequence).return_const(0x20_u32);
        apic.expect_current_apic_id().once().in_sequence(&mut sequence).return_const(0x30_u32);

        let contexts = Box::leak(Box::new([ApContext::new()]));
        contexts[0].assign_processor(&processor_handoff(0x20));
        let mut support = test_support(apic, contexts, false);
        support.bsp_processor_id = 0x10;

        assert_eq!(support.who_am_i(), Some(Processor::Bsp));
        assert_eq!(support.who_am_i(), Some(Processor::Ap(0)));
        assert_eq!(support.who_am_i(), None);
    }

    #[test]
    fn test_mp_support_reset_sends_init_then_two_startup_ipis() {
        const INIT_COMMAND: u32 = (0b101 << 8) | bit!(14);
        const STARTUP_COMMAND: u32 = (0b110 << 8) | bit!(14) | 8;

        let mut sequence = Sequence::new();
        let mut apic = apic::MockApicBackend::new();
        apic.expect_send_icr().with(eq(0x20), eq(INIT_COMMAND)).once().in_sequence(&mut sequence).return_const(());
        apic.expect_send_icr().with(eq(0x20), eq(STARTUP_COMMAND)).once().in_sequence(&mut sequence).return_const(());
        apic.expect_send_icr().with(eq(0x20), eq(STARTUP_COMMAND)).once().in_sequence(&mut sequence).return_const(());

        let contexts = Box::leak(Box::new([ApContext::new()]));
        contexts[0].assign_processor(&processor_handoff(0x20));
        let count = AtomicU64::new(0);
        let mut timer = MockArchTimerFunctionality::new();
        timer.expect_cpu_count().returning(move || count.fetch_add(AP_ABORT_TIMEOUT_US + 1, Ordering::Relaxed));
        let support = MpSupport {
            apic: apic::Apic::mock(apic),
            contexts,
            bsp_processor_id: 0x10,
            timer: Box::leak(Box::new(timer)),
            perf_frequency: NonZeroU64::new(1_000_000).unwrap(),
            startup_vector: 8,
            shutting_down: AtomicBool::new(false),
        };

        assert!(!support.set_ap_enabled(0, true, None));
        assert_eq!(support.contexts[0].sm.state(), ProcessorState::NotStarted);
        assert!(!support.contexts[0].sm.is_healthy());
    }

    #[test]
    #[serial(ap_setup)]
    fn test_mp_support_reset_recovers_started_processor() {
        const INIT_COMMAND: u32 = (0b101 << 8) | bit!(14);
        const STARTUP_COMMAND: u32 = (0b110 << 8) | bit!(14) | 8;

        let mut sequence = Sequence::new();
        let mut apic = apic::MockApicBackend::new();
        apic.expect_send_icr().with(eq(0x20), eq(INIT_COMMAND)).once().in_sequence(&mut sequence).return_const(());
        apic.expect_send_icr().with(eq(0x20), eq(STARTUP_COMMAND)).once().in_sequence(&mut sequence).return_const(());
        apic.expect_send_icr().with(eq(0x20), eq(STARTUP_COMMAND)).once().in_sequence(&mut sequence).return_const(());

        let contexts = Box::leak(Box::new([ApContext::new()]));
        contexts[0].assign_processor(&processor_handoff(0x20));
        let _consumer = contexts[0].sm.start().unwrap();
        ap_setup::setup(&[]);
        ap_setup::increment_started_count();

        let calls = AtomicU32::new(0);
        let context = &contexts[0];
        let mut timer = MockArchTimerFunctionality::new();
        timer.expect_cpu_count().times(8).returning(move || {
            let call = calls.fetch_add(1, Ordering::Relaxed);
            if call == 6 {
                assert!(context.sm.start().is_some());
            }
            u64::from(call) * (AP_ABORT_TIMEOUT_US + 1)
        });
        let support = MpSupport {
            apic: apic::Apic::mock(apic),
            contexts,
            bsp_processor_id: 0x10,
            timer: Box::leak(Box::new(timer)),
            perf_frequency: NonZeroU64::new(1_000_000).unwrap(),
            startup_vector: 8,
            shutting_down: AtomicBool::new(false),
        };

        assert!(support.reset_ap(&support.contexts[0]));
        assert_eq!(support.contexts[0].sm.state(), ProcessorState::Ready);
        assert!(support.contexts[0].sm.is_healthy());
        assert_eq!(support.started_ap_count(), 0);
    }
}
