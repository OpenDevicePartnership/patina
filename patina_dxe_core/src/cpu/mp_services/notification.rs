//! Non-blocking MP dispatch notification tracking.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use patina::{component::service::perf_timer::ArchTimerFunctionality, standard::efi};
use patina_internal_cpu::mp::MpSupport;

use crate::tpl_mutex::TplMutex;

use super::{dispatch::Dispatch, services::DispatchCompletion};

/// A non-blocking dispatch awaiting completion, resolved by the periodic poll.
pub(super) struct PendingDispatch {
    dispatch: Dispatch,
    /// Reports the outcome to the caller once the dispatch resolves.
    completion: DispatchCompletion,
}

// SAFETY: the completion closure holds the caller's out-pointers, which are only
// touched while the registry `TplMutex` is held, and the MP dispatch model is
// single-BSP, so there is no concurrent cross-thread access to a `PendingDispatch`.
unsafe impl Send for PendingDispatch {}

impl PendingDispatch {
    /// Builds a pending dispatch from work that has already been started.
    pub(super) fn new(dispatch: Dispatch, completion: DispatchCompletion) -> Self {
        Self { dispatch, completion }
    }

    /// Whether this dispatch still owns `index`, either because it has not been
    /// signaled yet or because it has not finished. Such a processor must not be
    /// handed to another dispatch even when it looks idle.
    pub(super) fn claims(&self, index: usize) -> bool {
        self.dispatch.claims(index)
    }

    /// Reports the outcome to the caller. APs still queued or active are reported as
    /// failed (they did not finish before the timeout), and any that are still running
    /// are fenced off so they cannot block later work.
    fn resolve(self, mp: &MpSupport) {
        let failed = self.dispatch.finish(mp);
        (self.completion)(&failed);
    }
}

/// Registry of in-flight non-blocking dispatches, resolved by the periodic poll.
pub(super) struct NotificationRegistry {
    pending: TplMutex<Vec<PendingDispatch>>,
}

impl NotificationRegistry {
    pub(super) fn new() -> Self {
        Self { pending: TplMutex::new(efi::TPL_NOTIFY, Vec::new(), "MpNotifications") }
    }

    /// Runs `dispatch` with the registry locked.
    ///
    /// The lock doubles as the MP dispatch lock. It raises to `TPL_NOTIFY`, which the
    /// periodic poll also needs, so no processor can be selected, signaled or taken
    /// while a dispatch is being set up. `dispatch` receives the pending list so it
    /// can both see which processors are already claimed and register itself.
    pub(super) fn with_dispatch_lock<R>(&self, dispatch: impl FnOnce(&mut Vec<PendingDispatch>) -> R) -> R {
        dispatch(&mut self.pending.lock())
    }

    /// Waits for every outstanding notification to complete or reach its deadline.
    pub(super) fn finish_pending(&self, mp: &MpSupport, timer: &dyn ArchTimerFunctionality) {
        while !self.process(mp, timer) {
            core::hint::spin_loop();
        }
    }

    /// Advances every pending dispatch and resolves those that have completed or timed out.
    pub(super) fn poll(&self, mp: &MpSupport, timer: &dyn ArchTimerFunctionality) {
        self.process(mp, timer);
    }

    /// Processes pending dispatches once, returning whether the registry is empty.
    fn process(&self, mp: &MpSupport, timer: &dyn ArchTimerFunctionality) -> bool {
        let now = timer.cpu_count();
        let mut resolved: Vec<PendingDispatch> = Vec::new();

        let empty = {
            let mut pending = self.pending.lock();
            for dispatch in pending.iter_mut() {
                dispatch.dispatch.advance(mp, now);
            }

            // Prune finished dispatches.
            resolved.extend(pending.extract_if(.., |dispatch| dispatch.dispatch.is_resolved(now)));
            pending.is_empty()
        };

        // Resolve outside the lock to avoid re-entrance deadlocks.
        for dispatch in resolved {
            dispatch.resolve(mp);
        }
        empty
    }
}
