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

use super::control::{ApAction, ApConsumer, ApStateMachine};
use super::{ApWorkItem, MpDispatcher, MpHandOffInfo, Processor, ProcessorState};

mod ap_bootstrap;
mod ap_setup;
mod apic;
mod park;
mod sync;

/// Sentinel value indicating an unused entry in [`ApContext::apic_id`].
const APIC_ID_INVALID: u32 = 0xFFFF_FFFF;

/// Maximum APIC ID in xAPIC mode.
const X_APIC_ID_MAX: u32 = 0xFF;

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
    contexts: &'static [ApContext],
    bsp_processor_id: u32,
    timer: &'static dyn ArchTimerFunctionality,
    perf_frequency: NonZeroU64,
    startup_vector: u8,
    shutting_down: AtomicBool,
}

impl MpSupport {
    fn validate_handoff(handoff: &MpHandOffInfo<'_>) -> bool {
        let bsp_apic_id = Self::get_current_apic_id();
        let x2apic = Self::is_x2apic_enabled();
        let mut bsp_found = false;

        for (index, processor) in handoff.processors.iter().enumerate() {
            if processor.processor_id == bsp_apic_id {
                bsp_found = true;
            }

            if !x2apic && processor.processor_id > X_APIC_ID_MAX {
                log::error!("MP handoff processor {index} has an APIC ID that is invalid in xAPIC mode");
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

    fn is_x2apic_enabled() -> bool {
        apic::is_x2apic_enabled()
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

    fn cpuid(leaf: u32, subleaf: u32) -> core::arch::x86_64::CpuidResult {
        core::arch::x86_64::__cpuid_count(leaf, subleaf)
    }

    fn is_monitor_supported() -> bool {
        (Self::cpuid(1, 0).ecx & bit!(3)) != 0
    }

    #[cfg_attr(coverage, coverage(off))]
    fn flush_tlb() {
        // SAFETY: Reading CR3 and writing the same value back preserves the active
        // page tables while invalidating this processor's non-global translations.
        unsafe {
            core::arch::asm!(
                "mov {cr3}, cr3",
                "mov cr3, {cr3}",
                cr3 = out(reg) _,
                options(nostack, preserves_flags),
            );
        }
    }

    fn ap_run_dispatch_loop(ctx: &'static ApContext) -> ! {
        apic::mask_local_interrupts();

        let Some(ap) = ctx.sm.start() else {
            Self::fail_ap(ap_setup::FAILURE_START_REJECTED, 0);
        };
        ap_setup::increment_started_count();

        let use_mwait = Self::is_monitor_supported();
        loop {
            match ap.execute_pending_work(Self::flush_tlb) {
                ApAction::Executed => continue,
                ApAction::Exit => {
                    // SAFETY: The park environment was installed before APs were started.
                    unsafe { park::ap_park() }
                }
                ApAction::Idle => {}
            }
            if use_mwait {
                monitor_wait(&ap);
            } else {
                core::hint::spin_loop();
            }
        }
    }

    fn get_current_apic_id() -> u32 {
        if Self::is_x2apic_enabled() { Self::cpuid(0xB, 0).edx } else { Self::cpuid(1, 0).ebx >> 24 }
    }

    fn fail_ap(reason: u32, detail: u64) -> ! {
        // SAFETY: The assembly routine follows the EFIAPI register convention and never returns.
        unsafe { ap_setup::ap_record_failure(reason, detail, Self::get_current_apic_id()) }
    }

    fn apply_mtrrs_or_fail() {
        if !sync::apply() {
            Self::fail_ap(ap_setup::FAILURE_MTRR_SETUP, 0);
        }
    }

    fn reset_ap(&self, ctx: &ApContext) -> bool {
        let was_started = ctx.sm.state() != ProcessorState::NotStarted;
        let apic_id = ctx.apic_id.load(Ordering::Relaxed);
        apic::send_init(apic_id);
        self.try_for(INIT_TO_SIPI_DELAY_US, || false);
        if was_started && !ap_setup::decrement_started_count() {
            ctx.sm.set_healthy(false);
            return false;
        }
        ctx.sm.reset();
        // SAFETY: INIT delivery has completed and the architectural settle time
        // has elapsed, so this AP is in wait-for-SIPI and cannot reload TR.
        unsafe { ctx.gdt.reset_tss_descriptor(core::ptr::addr_of!(ctx.tss) as u64) };
        apic::send_startup(apic_id, self.startup_vector);
        self.try_for(SIPI_DELAY_US, || false);
        apic::send_startup(apic_id, self.startup_vector);
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
        let startup_vector = ap_bootstrap::startup_vector(bootstrap_page)?;
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

        Ok(Self {
            contexts: &[],
            bsp_processor_id: Self::get_current_apic_id(),
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
            Self::validate_handoff(handoff)
        });

        if handoff.is_none() {
            log::warn!("No usable processor handoff. MP Services will report a single processor!");
        }

        // Without a usable handoff there is still a BSP to describe, so the protocol
        // is published for a uniprocessor system rather than withheld entirely.
        let processors = handoff.as_ref().map(|handoff| handoff.processors).unwrap_or_default();
        let startup_signal_value = handoff.as_ref().map_or(0, |handoff| handoff.startup_signal_value);
        let ap_count = processors.len().saturating_sub(1);

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

        log::info!("BSP APIC ID: {:#x}", self.bsp_processor_id);

        // Assign each non-BSP handoff entry to a context slot, recording its
        // APIC ID so the AP can find itself once it wakes.
        let ap_handoffs = || processors.iter().filter(|p| p.processor_id != self.bsp_processor_id);
        let assigned = contexts.iter().zip(ap_handoffs()).count();
        for (ctx, p) in contexts.iter_mut().zip(ap_handoffs()) {
            ctx.assign_processor(p);
        }

        if assigned < contexts.len() {
            log::warn!("Handoff described {assigned} AP(s) but {} context slot(s) exist", contexts.len());
        }

        self.contexts = contexts;
        ap_setup::setup(self.contexts);

        if sync::capture().is_err() {
            log::error!("Failed to prepare BSP MTRRs for AP startup");
            return Err(EfiError::DeviceError);
        }

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
        let apic_id = Self::get_current_apic_id();
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

        let Ok(supported) = sync::capture() else {
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

/// Sleeps in `MWAIT` until the BSP writes this processor's lifecycle state.
fn monitor_wait(ap: &ApConsumer<'_>) {
    // SAFETY: MONITOR arms address monitoring on the lifecycle state, a live
    // writable atomic in write-back memory.
    unsafe {
        core::arch::asm!(
            "monitor",
            in("rax") ap.monitor_address(),
            in("rcx") 0,
            in("rdx") 0,
            options(nostack, preserves_flags),
        );
    }
    // Re-check after arming; sleep only while the state remains idle so work or
    // an exit request cannot be missed.
    if ap.should_wait() {
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
    fn ap_context_preserves_handoff_health() {
        let mut context = ApContext::new();
        context.assign_processor(&crate::mp::ProcessorHandOff {
            processor_id: 7,
            healthy: false,
            startup_signal_address: 0,
            startup_procedure_address: 0,
        });

        assert_eq!(context.apic_id.load(Ordering::Relaxed), 7);
        assert!(!context.sm.is_healthy());
    }

    #[test]
    fn ap_contexts_reuse_their_ap_stacks_for_double_faults() {
        let mut contexts = [ApContext::new(), ApContext::new()];
        let stack_tops = [NonZeroUsize::new(0x20_000).unwrap(), NonZeroUsize::new(0x40_000).unwrap()];

        for (context, stack_top) in contexts.iter_mut().zip(stack_tops) {
            // SAFETY: This test only records aligned addresses and never starts an AP.
            unsafe { context.set_stack_top(stack_top) }.unwrap();
            context.initialize_descriptor_tables();
        }

        assert_eq!(contexts[0].tss.ist1(), stack_tops[0].get() as u64);
        assert_eq!(contexts[1].tss.ist1(), stack_tops[1].get() as u64);
        assert_ne!(contexts[0].tss.ist1(), contexts[1].tss.ist1());
        let gdtr0_base = contexts[0].gdtr.base;
        let gdtr1_base = contexts[1].gdtr.base;
        assert_ne!(gdtr0_base, gdtr1_base);
    }
}
