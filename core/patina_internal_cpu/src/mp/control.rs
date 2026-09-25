//! Common structure for the AP state machine producer and consumer.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use crate::mp::ProcessorState;

use super::ApWorkItem;

/// Lifecycle of a processor within the dispatch protocol.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum RunState {
    /// Has not joined the dispatch loop.
    NotStarted = 0,
    /// Work published, but not yet claimed by the AP.
    Signalled = 1,
    /// The AP is executing the work item.
    Running = 2,
    /// Started and waiting for work.
    Idle = 3,
    /// The AP has been signaled to exit dispatch.
    SignalExit = 4,
    /// The AP has exited the dispatch loop.
    Exited = 5,
}

impl RunState {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Signalled,
            2 => Self::Running,
            3 => Self::Idle,
            4 => Self::SignalExit,
            5 => Self::Exited,
            _ => Self::NotStarted,
        }
    }
}

fn processor_state(run_state: RunState, enabled: bool) -> ProcessorState {
    match run_state {
        RunState::NotStarted => ProcessorState::NotStarted,
        RunState::SignalExit | RunState::Exited => ProcessorState::Disabled,
        _ if !enabled => ProcessorState::Disabled,
        RunState::Idle => ProcessorState::Ready,
        RunState::Signalled | RunState::Running => ProcessorState::Busy,
    }
}

/// Per-AP dispatch state machine. A single producer (the BSP) hands work to a
/// single consumer (the owning AP).
///
/// The consumer side is reachable only through [`ApStateMachine::start`], which uses a
/// compare-exchange that can succeed only once, ensuring exclusive ownership of the
/// consumer side.
///
/// The BSP assigns each accepted dispatch a monotonically increasing id. Since a
/// new dispatch is accepted only while the AP is idle, observing a newer id also
/// proves every older dispatch completed. The AP only manages lifecycle state.
pub(crate) struct ApStateMachine {
    state: AtomicU8,
    enabled: AtomicBool,
    healthy: AtomicBool,
    work_id: AtomicU64,

    // Work cell safety rules:
    // - The BSP may only write to this cell when AP is in the idle state.
    // - The AP may only read/write from this cell when in the running state.
    // - The cell must not be touched in any other condition.
    work: UnsafeCell<Option<ApWorkItem>>,
}

// SAFETY: Only the processor that wins `start` ever claims work, and only the BSP
// ever publishes it, so there is no concurrent access to the `UnsafeCell` work slot.
// The lifecycle state controls the handoff.
unsafe impl Sync for ApStateMachine {}

impl ApStateMachine {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU8::new(RunState::NotStarted as u8),
            work_id: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            healthy: AtomicBool::new(true),
            work: UnsafeCell::new(None),
        }
    }

    fn load(&self) -> RunState {
        RunState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Moves the run state from `from` to `to`. Fails without writing if the
    /// processor is not in `from`.
    fn transition(&self, from: RunState, to: RunState, success: Ordering) -> bool {
        self.state.compare_exchange(from as u8, to as u8, success, Ordering::Acquire).is_ok()
    }

    pub(crate) fn state(&self) -> ProcessorState {
        processor_state(self.load(), self.enabled.load(Ordering::Acquire))
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub(crate) fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::Relaxed);
    }

    /// Claims the AP context for the calling processor, returning a consumer handle that can
    /// be used to run work. None is returned in the event the context is already claimed by
    /// another processor. This guarantees exclusive access to the AP context for the claiming processor.
    pub(crate) fn start(&self) -> Option<ApConsumer<'_>> {
        if self.transition(RunState::NotStarted, RunState::Idle, Ordering::Release) {
            return Some(ApConsumer { sm: self });
        }
        let _ = self.transition(RunState::SignalExit, RunState::Exited, Ordering::Release);
        None
    }

    /// Publishes `work` to this processor, returning the id identifying the dispatch.
    ///
    /// Callers must serialize dispatches against one another. The work slot is
    /// written before the state is published, which is only sound because nothing
    /// else can take an idle processor in the meantime.
    pub(crate) fn dispatch(&self, work: ApWorkItem) -> Result<u64, ProcessorState> {
        let current = self.load();
        let availability = processor_state(current, self.enabled.load(Ordering::Acquire));
        if availability != ProcessorState::Ready {
            return Err(availability);
        }

        // SAFETY: The processor is idle, so the AP is not looking at the slot, and the
        // caller's dispatch lock keeps any other producer out until the store below
        // publishes the work.
        unsafe { *self.work.get() = Some(work) };
        let work_id = self.work_id.fetch_add(1, Ordering::Relaxed) + 1;
        if self
            .state
            .compare_exchange(current as u8, RunState::Signalled as u8, Ordering::Release, Ordering::Acquire)
            .is_err()
        {
            // SAFETY: A failed Idle -> Signalled transition means the AP cannot
            // claim the unpublished slot. Producers are externally serialized.
            unsafe { *self.work.get() = None };
            return Err(self.state());
        }
        Ok(work_id)
    }

    /// Requests that the consumer stop dispatching and unwind its caller.
    pub(crate) fn signal_exit(&self) {
        loop {
            let current = self.load();
            match current {
                RunState::SignalExit | RunState::Exited => return,
                _ => {}
            }

            if self
                .state
                .compare_exchange(current as u8, RunState::SignalExit as u8, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if current == RunState::Signalled {
                    // SAFETY: Winning Signalled -> SignalExit proves the AP did
                    // not claim this slot by transitioning to Running.
                    unsafe { *self.work.get() = None };
                }
                return;
            }
        }
    }

    /// Returns the processor to its pre-start state so an architectural reset
    /// can re-enter through [`Self::start`].
    pub(crate) fn reset(&self) {
        // Work is intentionally left alone. It is not safe to write to it here,
        // and it will be overwritten if/when the AP returns to idle and new work
        // is published.
        self.state.store(RunState::NotStarted as u8, Ordering::Release);
    }

    // Checks whether the dispatch identified by `id` has finished or was canceled by exit.
    // A newer id can be published only after this dispatch reached Idle.
    pub(crate) fn is_finished(&self, id: u64) -> bool {
        let work_id = self.work_id.load(Ordering::Acquire);
        work_id > id || (work_id == id && matches!(self.load(), RunState::Idle | RunState::Exited))
    }

    /// Simulates an AP that entered its work item and never came back.
    #[cfg(test)]
    fn wedge(&self) -> bool {
        self.transition(RunState::Signalled, RunState::Running, Ordering::Acquire)
    }
}

/// The consumer half of the state machine, owned by the AP the context belongs to.
///
/// Obtainable only by [`ApStateMachine::start`], and only once, so the producer side
/// cannot run work and two processors cannot share a context.
///
/// Architecture code drives this consumer from its dispatch loop.
pub(crate) struct ApConsumer<'a> {
    sm: &'a ApStateMachine,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ApAction {
    Idle,
    Executed,
    Exit,
}

impl ApConsumer<'_> {
    fn acknowledge_exit(&self) -> ApAction {
        let _ = self.sm.transition(RunState::SignalExit, RunState::Exited, Ordering::Release);
        ApAction::Exit
    }

    /// Runs published work or acknowledges a request to unwind the dispatch loop.
    ///
    /// `prepare` runs after this processor claims a work item and immediately before
    /// invoking it. It is not called while idle or when acknowledging exit.
    fn execute_pending_work(&self, prepare: impl FnOnce()) -> ApAction {
        if matches!(self.sm.load(), RunState::SignalExit | RunState::Exited) {
            return self.acknowledge_exit();
        }

        // Try to transition to the running state. If this fails, there is no work to execute.
        if !self.sm.transition(RunState::Signalled, RunState::Running, Ordering::Acquire) {
            return if self.sm.load() == RunState::SignalExit { self.acknowledge_exit() } else { ApAction::Idle };
        }

        // SAFETY: The AP has been transitioning to running and has exclusive access to the work.
        let work = unsafe { (*self.sm.work.get()).take() };
        let ran = match work {
            Some(work) => {
                prepare();
                work.run();
                true
            }
            None => false,
        };

        if !self.sm.transition(RunState::Running, RunState::Idle, Ordering::Release)
            && self.sm.load() == RunState::SignalExit
        {
            return self.acknowledge_exit();
        }
        if ran { ApAction::Executed } else { ApAction::Idle }
    }

    /// Runs work until exit is requested, invoking `idle` while waiting for work.
    pub(crate) fn run_dispatch_loop(&self, prepare: impl Fn(), mut idle: impl FnMut(&AtomicU8)) {
        loop {
            match self.execute_pending_work(&prepare) {
                ApAction::Executed => continue,
                ApAction::Exit => return,
                ApAction::Idle => idle(&self.sm.state),
            }
        }
    }
}

/// Whether a processor should still wait after arming its monitor.
pub(crate) fn should_wait(state: &AtomicU8) -> bool {
    RunState::from_u8(state.load(Ordering::Acquire)) == RunState::Idle
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::sync::atomic::AtomicU32;

    // AP procedure: increments the `AtomicU32` pointed to by `arg`.
    unsafe extern "efiapi" fn increment(arg: *mut c_void) {
        // SAFETY: every caller below passes the address of a live `AtomicU32`.
        let counter = unsafe { &*(arg as *const AtomicU32) };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn work_for(counter: &AtomicU32) -> ApWorkItem {
        // SAFETY: `increment` matches the `AtomicU32` argument, which outlives every
        // in-test dispatch (the work runs synchronously before the counter drops).
        unsafe { ApWorkItem::new_efi(increment, core::ptr::from_ref(counter).cast_mut().cast()) }
    }

    /// Lets an AP procedure observe the machine from inside its own work item, which
    /// is the only point at which the `Running` state is visible.
    struct Probe {
        sm: *const ApStateMachine,
        id: AtomicU64,
        saw_busy: AtomicBool,
        saw_finished: AtomicBool,
    }

    unsafe extern "efiapi" fn observe(arg: *mut c_void) {
        // SAFETY: the test passes the address of a live `Probe`.
        let probe = unsafe { &*(arg as *const Probe) };
        // SAFETY: the probe refers to a state machine that outlives the dispatch.
        let sm = unsafe { &*probe.sm };
        probe.saw_busy.store(sm.state() == ProcessorState::Busy, Ordering::SeqCst);
        probe.saw_finished.store(sm.is_finished(probe.id.load(Ordering::SeqCst)), Ordering::SeqCst);
    }

    unsafe extern "efiapi" fn request_exit(arg: *mut c_void) {
        // SAFETY: The test passes the address of a live state machine.
        let sm = unsafe { &*(arg as *const ApStateMachine) };
        sm.signal_exit();
    }

    #[test]
    fn new_is_not_started() {
        let sm = ApStateMachine::new();
        assert_eq!(sm.state(), ProcessorState::NotStarted);
    }

    #[test]
    fn start_becomes_ready() {
        let sm = ApStateMachine::new();
        let ap = sm.start().expect("a fresh machine can be started");
        assert_eq!(sm.state(), ProcessorState::Ready);
        assert!(should_wait(&ap.sm.state));
    }

    #[test]
    fn start_succeeds_for_only_one_processor() {
        let sm = ApStateMachine::new();
        let _ap = sm.start().expect("a fresh machine can be started");
        // A second processor reaching the same context gets nothing to drive it with.
        assert!(sm.start().is_none());
    }

    #[test]
    fn start_acknowledges_an_existing_exit_request() {
        let sm = ApStateMachine::new();
        sm.signal_exit();

        assert!(sm.start().is_none());
        assert_eq!(sm.load(), RunState::Exited);
    }

    #[test]
    fn execute_pending_work_reports_whether_it_ran() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        // Nothing published yet, so there is nothing to run.
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Idle);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        let id = sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(sm.is_finished(id));
        assert_eq!(sm.state(), ProcessorState::Ready);
    }

    #[test]
    fn execute_pending_work_prepares_only_before_work() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        assert_eq!(ap.execute_pending_work(|| counter.store(10, Ordering::SeqCst)), ApAction::Idle);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(ap.execute_pending_work(|| counter.store(10, Ordering::SeqCst)), ApAction::Executed);
        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn execute_pending_work_runs_each_dispatch_exactly_once() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        const CYCLES: u32 = 4;
        for _ in 0..CYCLES {
            sm.dispatch(work_for(&counter)).unwrap();
            assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
            // A second call finds nothing left to do.
            assert_eq!(ap.execute_pending_work(|| {}), ApAction::Idle);
        }
        assert_eq!(counter.load(Ordering::SeqCst), CYCLES);
        assert_eq!(sm.state(), ProcessorState::Ready);
    }

    #[test]
    fn should_wait_tracks_the_publish_and_the_run() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        // A caller monitoring the lifecycle state must sleep only while idle.
        assert!(should_wait(&ap.sm.state));
        sm.dispatch(work_for(&counter)).unwrap();
        assert!(!should_wait(&ap.sm.state));
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert!(should_wait(&ap.sm.state));
    }

    #[test]
    fn dispatch_before_started_is_rejected() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        assert_eq!(sm.dispatch(work_for(&counter)), Err(ProcessorState::NotStarted));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dispatch_when_ready_marks_busy() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        assert_eq!(sm.dispatch(work_for(&counter)), Ok(1));
        assert!(!should_wait(&ap.sm.state));
        assert_eq!(sm.state(), ProcessorState::Busy);
    }

    #[test]
    fn dispatch_when_busy_is_rejected() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let _ap = sm.start().unwrap();
        sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(sm.dispatch(work_for(&counter)), Err(ProcessorState::Busy));
    }

    #[test]
    fn disabled_rejects_dispatch_and_outranks_busy() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let _ap = sm.start().unwrap();
        sm.set_enabled(false);
        assert_eq!(sm.state(), ProcessorState::Disabled);
        assert_eq!(sm.dispatch(work_for(&counter)), Err(ProcessorState::Disabled));

        sm.set_enabled(true);
        sm.dispatch(work_for(&counter)).unwrap();
        // A wedged AP is fenced off while still busy, and must not report as busy or
        // it would keep blocking dispatches to the other processors.
        sm.set_enabled(false);
        assert_eq!(sm.state(), ProcessorState::Disabled);
    }

    #[test]
    fn exit_reaches_disabled_idle_processor() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        sm.set_enabled(false);

        sm.signal_exit();
        assert!(!should_wait(&ap.sm.state));
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Exit);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert_eq!(sm.state(), ProcessorState::Disabled);
    }

    #[test]
    fn exit_cancels_work_that_was_not_claimed() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        let id = sm.dispatch(work_for(&counter)).unwrap();

        sm.signal_exit();
        assert!(!should_wait(&ap.sm.state));
        assert!(!sm.is_finished(id));
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Exit);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        assert!(sm.is_finished(id));
        assert_eq!(sm.load(), RunState::Exited);
    }

    #[test]
    fn exit_requested_by_running_work_unwinds_after_work_returns() {
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        // SAFETY: `request_exit` receives this live state machine and runs
        // synchronously before it is dropped.
        let work = unsafe { ApWorkItem::new_efi(request_exit, (&raw const sm).cast_mut().cast()) };
        let id = sm.dispatch(work).unwrap();

        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Exit);
        assert!(sm.is_finished(id));
        assert_eq!(sm.load(), RunState::Exited);
    }

    #[test]
    fn exit_is_idempotent() {
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        sm.signal_exit();
        sm.signal_exit();
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Exit);
        sm.signal_exit();
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Exit);
        assert_eq!(sm.load(), RunState::Exited);
    }

    #[test]
    fn re_enabling_a_busy_ap_leaves_it_busy() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        sm.dispatch(work_for(&counter)).unwrap();
        sm.set_enabled(false);
        sm.set_enabled(true);
        assert_eq!(sm.state(), ProcessorState::Busy);

        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert_eq!(sm.state(), ProcessorState::Ready);
    }

    #[test]
    fn health_defaults_to_healthy_and_is_settable() {
        let sm = ApStateMachine::new();
        assert!(sm.is_healthy());
        sm.set_healthy(false);
        assert!(!sm.is_healthy());
    }

    #[test]
    fn work_runs_while_the_processor_is_still_busy() {
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        let probe = Probe {
            sm: &raw const sm,
            id: AtomicU64::new(0),
            saw_busy: AtomicBool::new(false),
            saw_finished: AtomicBool::new(true),
        };

        // SAFETY: `observe` matches the `Probe` argument, which outlives the dispatch.
        let work = unsafe { ApWorkItem::new_efi(observe, (&raw const probe).cast_mut().cast()) };
        let id = sm.dispatch(work).unwrap();
        probe.id.store(id, Ordering::SeqCst);

        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert!(probe.saw_busy.load(Ordering::SeqCst), "work must run before the processor goes idle");
        assert!(!probe.saw_finished.load(Ordering::SeqCst), "the dispatch must not report finished while running");
        assert!(sm.is_finished(id));
        assert_eq!(sm.state(), ProcessorState::Ready);
    }

    #[test]
    fn is_finished_flips_only_once_the_work_has_run() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        let id = sm.dispatch(work_for(&counter)).unwrap();

        assert!(!sm.is_finished(id));
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert!(sm.is_finished(id));
    }

    #[test]
    fn work_ids_are_monotonic_and_latch() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        let id0 = sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);

        let id1 = sm.dispatch(work_for(&counter)).unwrap();
        assert!(id1 > id0, "each dispatch must yield a fresh, larger id");
        // The earlier dispatch stays finished; the new one is not yet.
        assert!(sm.is_finished(id0));
        assert!(!sm.is_finished(id1));

        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert!(sm.is_finished(id1));
    }

    #[test]
    fn monitor_address_is_stable_and_non_null() {
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();
        let addr = ap.sm.state.as_ptr();
        assert!(!addr.is_null());
        assert_eq!(addr, ap.sm.state.as_ptr());
    }

    #[test]
    fn dispatch_publishes_work_and_wakes_only_once() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        // An idle processor's state is untouched until there is real work for it, so an
        // AP monitoring it is not woken by bookkeeping.
        let idle = sm.load();
        assert_eq!(sm.state(), ProcessorState::Ready);
        assert!(should_wait(&ap.sm.state));
        assert_eq!(sm.load(), idle);

        let id = sm.dispatch(work_for(&counter)).expect("an idle processor accepts work");
        assert!(!should_wait(&ap.sm.state));
        assert_ne!(sm.load(), idle);

        assert_eq!(ap.execute_pending_work(|| {}), ApAction::Executed);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert!(sm.is_finished(id));
    }

    #[test]
    fn dispatch_advances_the_id_so_ids_are_never_reused() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let _ap = sm.start().unwrap();

        // A processor whose work never returns keeps its id and cannot hand out another.
        let stuck = sm.dispatch(work_for(&counter)).unwrap();
        assert!(sm.wedge());
        assert_eq!(sm.state(), ProcessorState::Busy);
        assert!(!sm.is_finished(stuck));

        sm.set_enabled(false);
        sm.set_enabled(true);
        assert_eq!(sm.dispatch(work_for(&counter)), Err(ProcessorState::Busy));
        assert!(!sm.is_finished(stuck));
    }

    #[test]
    fn disabled_processor_can_still_finish_its_in_flight_work() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let ap = sm.start().unwrap();

        let id = sm.dispatch(work_for(&counter)).unwrap();
        // The BSP fences the processor off while it is mid-dispatch; the AP side must
        // not be blocked by the flag or it would wedge forever.
        sm.set_enabled(false);
        assert_eq!(
            ap.execute_pending_work(|| {}),
            ApAction::Executed,
            "a disabled processor still runs published work"
        );
        assert!(sm.is_finished(id));
        assert_eq!(sm.state(), ProcessorState::Disabled);

        sm.set_enabled(true);
        assert_eq!(sm.state(), ProcessorState::Ready);
    }

    #[test]
    fn reset_running_work_requires_restart_before_completion() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let _ap = sm.start().unwrap();
        let id = sm.dispatch(work_for(&counter)).unwrap();
        assert!(sm.wedge());

        sm.reset();
        assert_eq!(sm.state(), ProcessorState::NotStarted);
        assert!(!sm.transition(RunState::Running, RunState::Idle, Ordering::Release));
        assert!(!sm.is_finished(id));

        let recovered = sm.start().expect("the recovery entry restarts the reset context");
        assert!(sm.is_finished(id));
        assert_eq!(sm.state(), ProcessorState::Ready);

        let next_id = sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(recovered.execute_pending_work(|| {}), ApAction::Executed);
        assert!(next_id > id);
        assert!(sm.is_finished(next_id));
    }

    #[test]
    fn reset_cancels_work_not_yet_claimed() {
        let counter = AtomicU32::new(0);
        let sm = ApStateMachine::new();
        let _ap = sm.start().unwrap();
        let id = sm.dispatch(work_for(&counter)).unwrap();

        sm.reset();
        assert_eq!(sm.state(), ProcessorState::NotStarted);
        let recovered = sm.start().expect("the recovery entry restarts the reset context");
        assert!(sm.is_finished(id));
        assert_eq!(recovered.execute_pending_work(|| {}), ApAction::Idle);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        let next_id = sm.dispatch(work_for(&counter)).unwrap();
        assert_eq!(recovered.execute_pending_work(|| {}), ApAction::Executed);
        assert!(next_id > id);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
