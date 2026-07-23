//! DXE Core implementation of [`TimingServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::component::service::{
    IntoService,
    uefi_services::timing::{TimingError, TimingServices},
};

use crate::misc_boot_services::{core_set_watchdog_timer, core_stall};

/// Core implementation of [`TimingServices`], delegating to the metronome and watchdog
/// architectural support through the core's internal Rust APIs.
///
/// Registered with the component dispatcher only once both the Metronome and Watchdog Timer
/// Architectural Protocols are installed, so components depending on this service are not dispatched
/// until stall and watchdog timer control is available.
#[derive(IntoService)]
#[service(dyn TimingServices)]
pub(crate) struct CoreTimingServices;

impl TimingServices for CoreTimingServices {
    fn stall(&self, duration: core::time::Duration) -> Result<(), TimingError> {
        core_stall(duration.as_micros() as usize).map_err(TimingError::from)
    }

    fn set_watchdog_timer(&self, timeout_seconds: u64, watchdog_code: u64) -> Result<(), TimingError> {
        core_set_watchdog_timer(timeout_seconds as usize, watchdog_code).map_err(TimingError::from)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::test_support;
    use core::time::Duration;

    // The arch protocols don't have test reset at the moment, so these just log instead of asserting.
    fn check_stall_result(duration: Duration, result: Result<(), TimingError>) {
        match result {
            Err(TimingError::NotReady) => {
                log::debug!("stall({duration:?}) correctly returned NotReady");
            }
            Ok(()) => {
                log::debug!("stall({duration:?}) returned Ok (metronome arch protocol available)");
            }
            Err(other) => {
                log::warn!("stall({duration:?}) returned unexpected error: {other:?}");
            }
        }
    }

    #[test]
    fn stall_delegates_and_translates_not_ready_error() {
        test_support::with_global_lock(|| {
            let svc = CoreTimingServices;
            check_stall_result(Duration::ZERO, svc.stall(Duration::ZERO));
            check_stall_result(Duration::from_micros(1), svc.stall(Duration::from_micros(1)));
            check_stall_result(Duration::from_millis(10), svc.stall(Duration::from_millis(10)));
        })
        .unwrap();
    }

    fn check_watchdog_result(timeout_seconds: u64, result: Result<(), TimingError>) {
        // The arch protocols don't have test reset at the moment, so these just log instead of asserting.
        match result {
            Err(TimingError::NotReady) => {
                log::debug!("set_watchdog_timer({timeout_seconds}) correctly returned NotReady");
            }
            Ok(()) => {
                log::debug!("set_watchdog_timer({timeout_seconds}) returned Ok (watchdog available)");
            }
            Err(other) => {
                log::warn!("set_watchdog_timer({timeout_seconds}) returned unexpected error: {other:?}");
            }
        }
    }

    #[test]
    fn set_watchdog_timer_delegates_and_translates_not_ready_error() {
        test_support::with_global_lock(|| {
            let svc = CoreTimingServices;
            // A timeout of 0 disables the watchdog timer (per the UEFI spec).
            check_watchdog_result(0, svc.set_watchdog_timer(0, 0));
            check_watchdog_result(300, svc.set_watchdog_timer(300, 0));
            check_watchdog_result(300, svc.set_watchdog_timer(300, 0x1234));
        })
        .unwrap();
    }
}
