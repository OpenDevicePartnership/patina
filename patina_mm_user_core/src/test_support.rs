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
/// The logger reports every record as enabled and then discards it. It produces no output,
/// but it causes `log::log_enabled!()` to return `true`, so the formatting and dispatch
/// branch inside each `log::*!` macro still runs during tests and is counted by coverage
/// instrumentation. Without it, a reporting path can execute while the message it builds
/// stays unreached.
///
/// Calling this more than once, from any number of tests, is safe.
pub(crate) fn init_test_logger() {
    use std::sync::OnceLock;
    static INIT: OnceLock<()> = OnceLock::new();

    /// Logger that reports every record as enabled but discards them. Used in tests so
    /// `log::log_enabled!()` returns `true` without producing any output.
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

        // Exercise the logger once so a logger that failed to install shows up here rather
        // than as messages quietly missing from every test that relies on one.
        assert!(log::log_enabled!(log::Level::Trace), "the test logger reports every level as enabled");
        log::trace!("test logger installed");
        log::logger().flush();
    });
}
