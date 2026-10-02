//! Provides the architecture agnostic wrapper for the implementation
//! and AP state maintained for the component/protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{boxed::Box, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

use patina::{
    component::service::perf_timer::ArchTimerFunctionality,
    standard::efi::{self, protocols::mp_services},
};
use patina_internal_cpu::mp::{ApWorkItem, MpDispatcher, MpSupport, Processor, ProcessorState};

use super::{
    dispatch::{Dispatch, deadline_for},
    notification::{NotificationRegistry, PendingDispatch},
};

/// UEFI MP Services processor number assigned to the BSP.
const BSP_PROCESSOR_INDEX: usize = 0;

pub(super) const fn ap_to_processor_index(ap_index: usize) -> usize {
    ap_index + 1
}

fn processor_to_ap_index(processor_index: usize) -> Option<usize> {
    processor_index.checked_sub(1)
}

/// Reports the outcome of a non-blocking dispatch once the periodic poll resolves
/// it, taking the processors the procedure did not complete on.
pub(super) type DispatchCompletion = Box<dyn FnOnce(&[usize])>;

/// The processors a dispatch targets, resolved under the dispatch lock.
enum Targets {
    /// Every started, enabled AP that no pending dispatch already owns.
    AllEligible,
    /// One specific AP.
    One(usize),
}

/// The structure that tracks and manages MP services. Invoked by the protocol wrapper.
pub(super) struct MpServices<M: MpDispatcher = MpSupport> {
    mp: M,
    processors: Vec<mp_services::ProcessorInformation>,
    timer: &'static dyn ArchTimerFunctionality,
    notifications: NotificationRegistry,
    ready_to_boot: AtomicBool,
}

impl<M: MpDispatcher> MpServices<M> {
    pub(super) fn new(
        mp: M,
        processors: Vec<mp_services::ProcessorInformation>,
        timer: &'static dyn ArchTimerFunctionality,
    ) -> Self {
        Self {
            mp,
            processors,
            timer,
            notifications: NotificationRegistry::new(),
            ready_to_boot: AtomicBool::new(false),
        }
    }

    /// Returns the total and enabled logical processor counts, including the BSP.
    ///
    /// Only callable from the BSP.
    pub(super) fn processor_count(&self) -> Result<(usize, usize), MpError> {
        self.bsp_check()?;
        Ok((self.mp.ap_count() + 1, self.mp.enabled_ap_count() + 1))
    }

    /// Enables or disables the AP at `processor_index`, optionally recording a new
    /// health status.
    ///
    /// Only callable from the BSP.
    pub(super) fn enable_disable_ap(
        &self,
        processor_index: usize,
        enable: bool,
        healthy: Option<bool>,
    ) -> Result<(), MpError> {
        self.bsp_check()?;
        let ap_index = processor_to_ap_index(processor_index).ok_or(MpError::InvalidProcessor)?;
        if ap_index >= self.mp.ap_count() {
            return Err(MpError::NotFound);
        }
        if self.notifications.with_dispatch_lock(|_| self.mp.set_ap_enabled(ap_index, enable, healthy)) {
            Ok(())
        } else {
            Err(MpError::NotSupported)
        }
    }

    /// Returns the processor index for the calling processor.
    pub(super) fn who_am_i(&self) -> Option<usize> {
        self.mp.who_am_i().map(|processor| match processor {
            Processor::Bsp => BSP_PROCESSOR_INDEX,
            Processor::Ap(index) => ap_to_processor_index(index),
        })
    }

    /// Signals all APs to park in a defined state for OS handoff.
    pub(super) fn park(&self) {
        debug_assert!(self.bsp_check().is_ok());
        self.mp.park();
    }

    /// Prevents new non-blocking requests and waits for existing requests to
    /// complete or reach their deadlines while allocation and event signaling
    /// are still available.
    pub(super) fn mark_ready_to_boot(&self) {
        self.ready_to_boot.store(true, Ordering::Release);
        self.notifications.finish_pending(&self.mp, self.timer);
    }

    /// Replicates the BSP's dynamic state (MTRRs for x64) to every enabled AP.
    pub(super) fn synchronize(&self) {
        self.notifications.finish_pending(&self.mp, self.timer);
        let synchronized = self.notifications.with_dispatch_lock(|pending| {
            debug_assert!(pending.is_empty());
            self.mp.sync_aps()
        });
        assert!(synchronized, "failed to synchronize BSP MTRRs to every enabled AP");
    }

    /// Returns the [`mp_services::ProcessorInformation`] for `index`.
    pub(super) fn processor_info(&self, index: usize) -> Result<mp_services::ProcessorInformation, MpError> {
        self.bsp_check()?;
        let processor_id = if index == BSP_PROCESSOR_INDEX {
            self.mp.bsp_processor_id()
        } else {
            self.mp.ap_processor_id(processor_to_ap_index(index).ok_or(MpError::NotFound)?).ok_or(MpError::NotFound)?
        };
        let mut info = self
            .processors
            .iter()
            .find(|p| p.processor_id == u64::from(processor_id))
            .copied()
            .unwrap_or_else(|| topology_unknown(processor_id));
        info.status_flag = self.status_flag(index);
        Ok(info)
    }

    fn status_flag(&self, index: usize) -> u32 {
        let mut flags = 0;
        if index == BSP_PROCESSOR_INDEX {
            flags |= mp_services::PROCESSOR_AS_BSP_BIT;
            flags |= mp_services::PROCESSOR_ENABLED_BIT | mp_services::PROCESSOR_HEALTH_STATUS_BIT;
            return flags;
        }
        let Some(ap_index) = processor_to_ap_index(index) else {
            return flags;
        };
        if !matches!(self.mp.ap_availability(ap_index), ProcessorState::NotStarted | ProcessorState::Disabled) {
            flags |= mp_services::PROCESSOR_ENABLED_BIT;
        }
        if self.mp.ap_healthy(ap_index) {
            flags |= mp_services::PROCESSOR_HEALTH_STATUS_BIT;
        }
        flags
    }

    /// Signals `targets` immediately, then either blocks until they finish (no
    /// `completion`) or registers the dispatch for the periodic poll. A single shared
    /// deadline bounds the whole operation, so a hung AP cannot extend the timeout
    /// for the others.
    fn dispatch(
        &self,
        targets: Targets,
        single_thread: bool,
        work: ApWorkItem,
        timeout_us: usize,
        completion: Option<DispatchCompletion>,
    ) -> Result<(), MpError> {
        if completion.is_some() && self.ready_to_boot.load(Ordering::Acquire) {
            return Err(MpError::NotSupported);
        }
        let deadline = deadline_for(self.timer, timeout_us);

        // The registry lock is the dispatch lock: it raises to TPL_NOTIFY, which the
        // periodic poll also needs, so no processor can be taken between being selected
        // and being signaled.
        let started = self.notifications.with_dispatch_lock(|pending| {
            let queued = self.select(&targets, pending.as_slice())?;
            let mut dispatch = Dispatch::new(work, single_thread, queued, deadline);
            dispatch.start(&self.mp);

            // Non-blocking: hand the started dispatch to the periodic poll, which waits
            // for completion and reports it.
            match completion {
                Some(completion) => {
                    pending.push(PendingDispatch::new(dispatch, completion));
                    Ok(None)
                }
                None => Ok(Some(dispatch)),
            }
        })?;

        let Some(dispatch) = started else { return Ok(()) };
        let failed = dispatch.complete_blocking(&self.mp, self.timer);
        if failed.is_empty() { Ok(()) } else { Err(MpError::Timeout(failed)) }
    }

    /// Resolves the processors a dispatch will target.
    ///
    /// Must be called under the dispatch lock: a processor that a pending dispatch
    /// has queued but not yet signaled still looks idle, so it can only be excluded
    /// while that list cannot change.
    fn select(&self, targets: &Targets, pending: &[PendingDispatch]) -> Result<Vec<usize>, MpError> {
        let claimed = |index: usize| pending.iter().any(|dispatch| dispatch.claims(index));
        match *targets {
            Targets::One(index) => {
                if index >= self.mp.ap_count() {
                    return Err(MpError::NotFound);
                }
                if claimed(index) {
                    return Err(MpError::Busy);
                }
                match self.mp.ap_availability(index) {
                    ProcessorState::NotStarted | ProcessorState::Disabled => Err(MpError::InvalidProcessor),
                    ProcessorState::Busy => Err(MpError::Busy),
                    ProcessorState::Ready => Ok(alloc::vec![index]),
                }
            }
            Targets::AllEligible => {
                let mut aps = Vec::new();
                for i in 0..self.mp.ap_count() {
                    if claimed(i) {
                        return Err(MpError::Busy);
                    }
                    match self.mp.ap_availability(i) {
                        ProcessorState::Ready => aps.push(i),
                        ProcessorState::Busy => return Err(MpError::Busy),
                        ProcessorState::NotStarted | ProcessorState::Disabled => {}
                    }
                }
                if aps.is_empty() { Err(MpError::NotStarted) } else { Ok(aps) }
            }
        }
    }

    /// Runs `work` on all started APs. Only callable from the BSP.
    ///
    /// Without a `completion` the call blocks until every AP finishes, failing with
    /// [`MpError::Timeout`] and the processors that did not. With one, it returns
    /// immediately after starting the APs and the periodic poll invokes `completion`.
    pub(super) fn startup_all_aps(
        &self,
        work: ApWorkItem,
        single_thread: bool,
        timeout_us: usize,
        completion: Option<DispatchCompletion>,
    ) -> Result<(), MpError> {
        self.bsp_check()?;
        self.dispatch(Targets::AllEligible, single_thread, work, timeout_us, completion)
    }

    /// Runs `work` on a single AP. Only callable from the BSP.
    ///
    /// Without a `completion` the call blocks until the AP finishes, failing with
    /// [`MpError::Timeout`] if it does not. With one, it returns immediately and the
    /// periodic poll invokes `completion`.
    pub(super) fn startup_this_ap(
        &self,
        work: ApWorkItem,
        processor_index: usize,
        timeout_us: usize,
        completion: Option<DispatchCompletion>,
    ) -> Result<(), MpError> {
        self.bsp_check()?;
        let ap_index = processor_to_ap_index(processor_index).ok_or(MpError::InvalidProcessor)?;
        self.dispatch(Targets::One(ap_index), false, work, timeout_us, completion)
    }

    /// Resolves any pending non-blocking dispatches. Invoked by the component's
    /// periodic timer callback.
    pub(super) fn poll_notifications(&self) {
        self.notifications.poll(&self.mp, self.timer);
    }

    fn bsp_check(&self) -> Result<(), MpError> {
        if self.mp.who_am_i() == Some(Processor::Bsp) { Ok(()) } else { Err(MpError::NotBsp) }
    }
}

/// Placeholder information for a processor the PEI information HOB did not
/// describe. The topology fields are unknown, but the processor still exists and
/// its status flags are derived live.
fn topology_unknown(processor_id: u32) -> mp_services::ProcessorInformation {
    mp_services::ProcessorInformation {
        processor_id: u64::from(processor_id),
        status_flag: 0,
        location: mp_services::CpuPhysicalLocation { package: 0, core: 0, thread: 0 },
        extended_information: mp_services::ExtendedProcessorInformation {
            location2: mp_services::CpuPhysicalLocation2 { package: 0, module: 0, tile: 0, die: 0, core: 0, thread: 0 },
        },
    }
}

/// Errors from the MP Services operations, mapped to `efi::Status` by the wrappers.
pub(super) enum MpError {
    NotBsp,
    Busy,
    /// The dispatch did not complete on the listed processors before its deadline.
    Timeout(Vec<usize>),
    NotStarted,
    NotSupported,
    InvalidProcessor,
    NotFound,
}

#[cfg_attr(coverage, coverage(off))]
impl From<MpError> for efi::Status {
    fn from(e: MpError) -> Self {
        match e {
            MpError::Timeout(_) => efi::Status::TIMEOUT,
            MpError::NotStarted => efi::Status::NOT_STARTED,
            MpError::Busy => efi::Status::NOT_READY,
            MpError::NotFound => efi::Status::NOT_FOUND,
            MpError::NotSupported => efi::Status::UNSUPPORTED,
            MpError::NotBsp => efi::Status::DEVICE_ERROR,
            MpError::InvalidProcessor => efi::Status::INVALID_PARAMETER,
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::eq;
    use patina_internal_cpu::mp::MockMpDispatcher;

    struct TestTimer;

    impl ArchTimerFunctionality for TestTimer {
        fn cpu_count(&self) -> u64 {
            0
        }

        fn perf_frequency(&self) -> u64 {
            1_000_000
        }
    }

    static TIMER: TestTimer = TestTimer;

    fn test_work() -> ApWorkItem {
        static ARGUMENT: () = ();
        ApWorkItem::new(|()| {}, &ARGUMENT)
    }

    fn test_service(mp: MockMpDispatcher) -> MpServices<MockMpDispatcher> {
        MpServices::new(mp, Vec::new(), &TIMER)
    }

    fn with_global_state(test: impl Fn() + std::panic::RefUnwindSafe) {
        if let Err(payload) = crate::test_support::with_global_lock(test) {
            std::panic::resume_unwind(payload);
        }
    }

    #[test]
    fn test_mp_services_converts_processor_indexes() {
        assert_eq!(ap_to_processor_index(0), 1);
        assert_eq!(ap_to_processor_index(7), 8);
        assert_eq!(processor_to_ap_index(0), None);
        assert_eq!(processor_to_ap_index(1), Some(0));
        assert_eq!(processor_to_ap_index(8), Some(7));
    }

    #[test]
    fn test_mp_services_reports_processor_counts_for_bsp() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_ap_count().once().return_const(3usize);
        mp.expect_enabled_ap_count().once().return_const(2usize);
        let service = test_service(mp);

        assert!(matches!(service.processor_count(), Ok((4, 3))));
    }

    #[test]
    fn test_mp_services_rejects_bsp_only_operation_from_ap() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Ap(0)));
        let service = test_service(mp);

        assert!(matches!(service.processor_count(), Err(MpError::NotBsp)));
    }

    #[test]
    fn test_mp_services_maps_processor_identity() {
        let mut mp = MockMpDispatcher::new();
        let mut identities = [Some(Processor::Bsp), Some(Processor::Ap(2)), None].into_iter();
        mp.expect_who_am_i().times(3).returning(move || identities.next().expect("identity expectation exhausted"));
        let service = test_service(mp);

        assert_eq!(service.who_am_i(), Some(0));
        assert_eq!(service.who_am_i(), Some(3));
        assert_eq!(service.who_am_i(), None);
    }

    #[test]
    fn test_mp_services_enable_disable_ap_validates_and_forwards() {
        with_global_state(|| {
            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            mp.expect_ap_count().once().return_const(1usize);
            mp.expect_set_ap_enabled().with(eq(0), eq(false), eq(Some(false))).once().return_const(true);
            let service = test_service(mp);
            assert!(matches!(service.enable_disable_ap(1, false, Some(false)), Ok(())));

            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            let service = test_service(mp);
            assert!(matches!(service.enable_disable_ap(0, true, None), Err(MpError::InvalidProcessor)));

            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            mp.expect_ap_count().once().return_const(1usize);
            let service = test_service(mp);
            assert!(matches!(service.enable_disable_ap(2, true, None), Err(MpError::NotFound)));

            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            mp.expect_ap_count().once().return_const(1usize);
            mp.expect_set_ap_enabled().once().return_const(false);
            let service = test_service(mp);
            assert!(matches!(service.enable_disable_ap(1, true, None), Err(MpError::NotSupported)));
        });
    }

    #[test]
    fn test_mp_services_processor_info_uses_topology_and_live_status() {
        let mut known = topology_unknown(7);
        known.location = mp_services::CpuPhysicalLocation { package: 1, core: 2, thread: 3 };
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_ap_processor_id().with(eq(0)).once().return_const(Some(7));
        mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Ready);
        mp.expect_ap_healthy().with(eq(0)).once().return_const(true);
        let service = MpServices::new(mp, vec![known], &TIMER);

        let Ok(info) = service.processor_info(1) else { panic!("known AP should have processor information") };

        assert_eq!(info.processor_id, 7);
        assert_eq!(info.location.package, 1);
        assert_eq!(info.location.core, 2);
        assert_eq!(info.location.thread, 3);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT, 0);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_HEALTH_STATUS_BIT, 0);
    }

    #[test]
    fn test_mp_services_processor_info_handles_bsp_disabled_and_missing_ap() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_bsp_processor_id().once().return_const(0x2Au32);
        let service = test_service(mp);
        let Ok(info) = service.processor_info(0) else { panic!("BSP should have processor information") };
        assert_eq!(info.processor_id, 0x2A);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_AS_BSP_BIT, 0);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_ENABLED_BIT, 0);
        assert_ne!(info.status_flag & mp_services::PROCESSOR_HEALTH_STATUS_BIT, 0);

        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_ap_processor_id().with(eq(0)).once().return_const(Some(7));
        mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Disabled);
        mp.expect_ap_healthy().with(eq(0)).once().return_const(false);
        let service = test_service(mp);
        let Ok(info) = service.processor_info(1) else { panic!("disabled AP should still have processor information") };
        assert_eq!(info.status_flag, 0);

        let mut mp = MockMpDispatcher::new();
        mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
        mp.expect_ap_processor_id().with(eq(0)).once().return_const(None);
        let service = test_service(mp);
        assert!(matches!(service.processor_info(1), Err(MpError::NotFound)));
    }

    #[test]
    fn test_mp_services_selects_eligible_targets() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(2usize);
        mp.expect_ap_availability().with(eq(1)).once().return_const(ProcessorState::Ready);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::One(1), &[]), Ok(indices) if indices == [1]));

        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(3usize);
        mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Ready);
        mp.expect_ap_availability().with(eq(1)).once().return_const(ProcessorState::Disabled);
        mp.expect_ap_availability().with(eq(2)).once().return_const(ProcessorState::NotStarted);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::AllEligible, &[]), Ok(indices) if indices == [0]));
    }

    #[test]
    fn test_mp_services_rejects_unavailable_or_claimed_targets() {
        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(1usize);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::One(1), &[]), Err(MpError::NotFound)));

        let pending = PendingDispatch::new(Dispatch::new(test_work(), false, vec![0], None), Box::new(|_| {}));
        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(1usize);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::One(0), &[pending]), Err(MpError::Busy)));

        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(1usize);
        mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Disabled);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::One(0), &[]), Err(MpError::InvalidProcessor)));

        let mut mp = MockMpDispatcher::new();
        mp.expect_ap_count().once().return_const(1usize);
        mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Busy);
        let service = test_service(mp);
        assert!(matches!(service.select(&Targets::AllEligible, &[]), Err(MpError::Busy)));
    }

    #[test]
    fn test_mp_services_completes_blocking_dispatch() {
        with_global_state(|| {
            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            mp.expect_ap_count().times(2).return_const(1usize);
            mp.expect_ap_availability().with(eq(0)).once().return_const(ProcessorState::Ready);
            mp.expect_signal_ap().withf(|index, _| *index == 0).once().return_const(Some(9));
            mp.expect_ap_finished().with(eq(0), eq(9)).once().return_const(true);
            let service = test_service(mp);

            assert!(matches!(service.startup_this_ap(test_work(), 1, 0, None), Ok(())));
        });
    }

    #[test]
    fn test_mp_services_startup_this_ap_rejects_bsp_and_missing_processor() {
        with_global_state(|| {
            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            let service = test_service(mp);
            assert!(matches!(
                service.startup_this_ap(test_work(), BSP_PROCESSOR_INDEX, 0, None),
                Err(MpError::InvalidProcessor)
            ));

            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            mp.expect_ap_count().once().return_const(1usize);
            let service = test_service(mp);
            assert!(matches!(service.startup_this_ap(test_work(), 2, 0, None), Err(MpError::NotFound)));
        });
    }

    #[test]
    fn test_mp_services_rejects_non_blocking_dispatch_after_ready_to_boot() {
        with_global_state(|| {
            let mut mp = MockMpDispatcher::new();
            mp.expect_who_am_i().once().return_const(Some(Processor::Bsp));
            let service = test_service(mp);

            service.mark_ready_to_boot();
            assert!(matches!(
                service.startup_all_aps(test_work(), false, 0, Some(Box::new(|_| {}))),
                Err(MpError::NotSupported)
            ));
        });
    }
}
