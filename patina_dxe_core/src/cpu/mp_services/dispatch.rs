//! Shared MP dispatch lifecycle tracking.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use patina::component::service::perf_timer::ArchTimerFunctionality;
use patina_internal_cpu::mp::{ApWorkItem, MpDispatcher, MpSupport};

use super::services::ap_to_processor_index;

/// A dispatch that owns its queued, active, and failed APs until completion.
pub(super) struct Dispatch {
    work: ApWorkItem,
    single_thread: bool,
    /// APs not yet signaled.
    queued: Vec<usize>,
    /// APs currently signaled and monitored for completion.
    active: Vec<(usize, u64)>,
    /// APs that could not be signaled or did not finish before the deadline.
    failed: Vec<usize>,
    /// Performance-counter tick at which the dispatch times out.
    deadline_tick: Option<u64>,
}

impl Dispatch {
    pub(super) fn new(work: ApWorkItem, single_thread: bool, queued: Vec<usize>, deadline_tick: Option<u64>) -> Self {
        Self { work, single_thread, queued, active: Vec::new(), failed: Vec::new(), deadline_tick }
    }

    /// Signals the initial APs for this dispatch.
    pub(super) fn start(&mut self, mp: &MpSupport) {
        if self.single_thread {
            self.signal_next(mp);
        } else {
            for index in self.queued.drain(..) {
                match mp.signal_ap(index, self.work) {
                    Some(work_id) => self.active.push((index, work_id)),
                    None => self.failed.push(index),
                }
            }
        }
    }

    /// Whether this dispatch still owns `index`, either because it has not been
    /// signaled yet or because it has not finished.
    pub(super) fn claims(&self, index: usize) -> bool {
        self.queued.contains(&index) || self.active.iter().any(|&(active_index, _)| active_index == index)
    }

    /// Advances a dispatch without blocking.
    pub(super) fn advance(&mut self, mp: &MpSupport, now: u64) {
        self.active.retain(|&(index, work_id)| !mp.ap_finished(index, work_id));
        if !self.deadline_passed(now) && self.single_thread && self.active.is_empty() {
            self.signal_next(mp);
        }
    }

    /// Whether the dispatch has finished all APs or reached its timeout tick.
    pub(super) fn is_resolved(&self, now: u64) -> bool {
        (self.queued.is_empty() && self.active.is_empty()) || self.deadline_passed(now)
    }

    /// Waits for this dispatch to finish and returns processor indices that failed.
    pub(super) fn complete_blocking(mut self, mp: &MpSupport, timer: &dyn ArchTimerFunctionality) -> Vec<usize> {
        loop {
            for (index, work_id) in core::mem::take(&mut self.active) {
                if !wait_ap_until(mp, timer, self.deadline_tick, index, work_id) {
                    abort_or_fence(mp, index, work_id);
                    self.failed.push(index);
                }
            }

            if self.deadline_passed(timer.cpu_count()) || self.queued.is_empty() {
                break;
            }

            self.signal_next(mp);
        }

        self.finish(mp)
    }

    /// Finalizes a dispatch and returns its failed UEFI processor indices.
    pub(super) fn finish(self, mp: &MpSupport) -> Vec<usize> {
        for &(index, work_id) in &self.active {
            abort_or_fence(mp, index, work_id);
        }
        self.failed
            .iter()
            .copied()
            .chain(self.queued.iter().copied())
            .chain(self.active.iter().map(|&(index, _)| index))
            .map(ap_to_processor_index)
            .collect()
    }

    fn signal_next(&mut self, mp: &MpSupport) {
        if self.active.is_empty() && !self.queued.is_empty() {
            let index = self.queued.remove(0);
            match mp.signal_ap(index, self.work) {
                Some(work_id) => self.active.push((index, work_id)),
                None => self.failed.push(index),
            }
        }
    }

    fn deadline_passed(&self, now: u64) -> bool {
        self.deadline_tick.is_some_and(|deadline| now >= deadline)
    }
}

fn abort_or_fence(mp: &MpSupport, index: usize, work_id: u64) {
    if !mp.abort_ap(index, work_id) {
        fence_off(mp, index);
    }
}

pub(super) fn fence_off(mp: &MpSupport, index: usize) {
    let processor_index = ap_to_processor_index(index);
    log::warn!("Processor {processor_index} did not finish its dispatch in time and will be fenced off.");
    mp.set_ap_enabled(index, false, Some(false));
}

pub(super) fn wait_ap_until(
    mp: &MpSupport,
    timer: &dyn ArchTimerFunctionality,
    deadline: Option<u64>,
    index: usize,
    work_id: u64,
) -> bool {
    if index >= mp.ap_count() {
        return false;
    }

    while !mp.ap_finished(index, work_id) {
        if deadline.is_some_and(|deadline| timer.cpu_count() >= deadline) {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

pub(super) fn deadline_for(timer: &dyn ArchTimerFunctionality, timeout_us: usize) -> Option<u64> {
    let freq = timer.perf_frequency();
    if timeout_us == 0 || freq == 0 {
        return None;
    }

    let delta = (timeout_us as u128 * u128::from(freq) / 1_000_000) as u64;
    Some(timer.cpu_count().saturating_add(delta))
}
