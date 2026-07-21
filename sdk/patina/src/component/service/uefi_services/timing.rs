//! Timing services for Patina components.
//!
//! [`TimingServices`] exposes UEFI timing operations such as a fine-grained stall and the
//! system watchdog timer as a Rust service.
//!
//! Delays are expressed with [`core::time::Duration`] so callers never juggle raw microsecond or
//! 100ns counts.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::time::Duration;

use crate::base::error::EfiError;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Errors that can occur when using [`TimingServices`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TimingError {
    /// The underlying architectural support (metronome or watchdog) is not yet available.
    NotReady,
    /// The underlying device reported an error while performing the operation.
    DeviceError,
    /// A provided parameter was invalid.
    InvalidParameter,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<TimingError> for EfiError {
    fn from(value: TimingError) -> Self {
        match value {
            TimingError::NotReady => EfiError::NotReady,
            TimingError::DeviceError => EfiError::DeviceError,
            TimingError::InvalidParameter => EfiError::InvalidParameter,
            TimingError::Internal => EfiError::Unsupported,
        }
    }
}

impl From<EfiError> for TimingError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::NotReady => TimingError::NotReady,
            EfiError::DeviceError => TimingError::DeviceError,
            EfiError::InvalidParameter => TimingError::InvalidParameter,
            _ => TimingError::Internal,
        }
    }
}

/// Timing services providing delays and watchdog timer control.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn TimingServices>`](crate::component::service::Service) parameter to their
/// entry point.
///
/// The DXE Core only registers this service once the Metronome and Watchdog Timer Architectural
/// Protocols are both installed.
///
/// # Examples
///
/// ```rust,no_run
/// use core::time::Duration;
/// use patina::component::service::{Service, uefi_services::timing::TimingServices};
/// use patina::error::Result;
///
/// fn entry_point(timing: Service<dyn TimingServices>) -> Result<()> {
///     timing.stall(Duration::from_millis(10))?;
///     Ok(())
/// }
/// ```
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait TimingServices {
    /// Stalls execution for at least `duration`.
    ///
    /// `duration` can be expressed in any unit supported by [`core::time::Duration`].
    ///
    /// Execution of the processor is not yielded for the duration of the stall. The delay is
    /// rounded to the resolution the underlying metronome supports.
    ///
    /// # Errors
    ///
    /// Returns [`TimingError::NotReady`] if the metronome architectural support is not yet
    /// available.
    fn stall(&self, duration: Duration) -> Result<(), TimingError>;

    /// Sets the system watchdog timer.
    ///
    /// The watchdog timer will fire after `timeout_seconds` seconds unless it is reset or
    /// disabled. A `timeout_seconds` of `0` disables the watchdog timer. `watchdog_code` is a
    /// caller-defined code logged by the firmware if the watchdog fires. Codes `0x0000`-`0xffff`
    /// are reserved by the UEFI specification for firmware use.
    ///
    /// # Errors
    ///
    /// Returns [`TimingError::NotReady`] if the watchdog architectural support is not yet
    /// available, or [`TimingError::DeviceError`] if the underlying device reports an error.
    fn set_watchdog_timer(&self, timeout_seconds: u64, watchdog_code: u64) -> Result<(), TimingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timing_services_error_to_efi() {
        assert_eq!(EfiError::from(TimingError::NotReady), EfiError::NotReady);
        assert_eq!(EfiError::from(TimingError::DeviceError), EfiError::DeviceError);
        assert_eq!(EfiError::from(TimingError::InvalidParameter), EfiError::InvalidParameter);
        assert_eq!(EfiError::from(TimingError::Internal), EfiError::Unsupported);
    }

    #[test]
    fn test_timing_services_error_from_efi() {
        assert_eq!(TimingError::from(EfiError::NotReady), TimingError::NotReady);
        assert_eq!(TimingError::from(EfiError::DeviceError), TimingError::DeviceError);
        assert_eq!(TimingError::from(EfiError::InvalidParameter), TimingError::InvalidParameter);
        assert_eq!(TimingError::from(EfiError::NotFound), TimingError::Internal);
    }

    #[test]
    fn test_timing_services_mock_delegation() {
        let mut mock = MockTimingServices::new();
        mock.expect_stall().times(1).returning(|duration| {
            assert_eq!(duration, Duration::from_millis(1));
            Ok(())
        });
        mock.expect_set_watchdog_timer().times(1).returning(|_, _| Err(TimingError::NotReady));

        assert_eq!(mock.stall(Duration::from_millis(1)), Ok(()));
        assert_eq!(mock.set_watchdog_timer(5, 0), Err(TimingError::NotReady));
    }
}
