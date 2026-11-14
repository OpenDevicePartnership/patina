//! MM User Core Test Support
//!
//! Code to help support testing.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Installs a silent logger for the current test process.
///
/// The logger reports every level as enabled and discards every record. Because
/// `log::log_enabled!()` then returns `true`, the formatting branch inside each `log::*!`
/// macro runs during tests and counts toward coverage.
///
/// Safe to call any number of times, from any test.
pub(crate) fn init_test_logger() {
    use std::sync::OnceLock;
    static INIT: OnceLock<()> = OnceLock::new();

    /// Reports every record as enabled and discards it.
    struct AlwaysEnabledSilentLogger;

    impl log::Log for AlwaysEnabledSilentLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, _record: &log::Record<'_>) {}
        fn flush(&self) {}
    }

    static SILENT_LOGGER: AlwaysEnabledSilentLogger = AlwaysEnabledSilentLogger;

    INIT.get_or_init(|| {
        let _ = log::set_logger(&SILENT_LOGGER);
        log::set_max_level(log::LevelFilter::Trace);

        // Log once so a logger that failed to install shows up here, not as missing
        // messages in every test that relies on it.
        assert!(log::log_enabled!(log::Level::Trace), "the test logger reports every level as enabled");
        log::trace!("test logger installed");
        log::logger().flush();
    });
}
