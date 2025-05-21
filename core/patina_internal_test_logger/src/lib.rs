use std::io::{self, Write};

use log::Log;

#[allow(dead_code)]
#[cfg_attr(test, ctor::ctor)]
fn setup_test_logger() {
    _ = log::set_logger(&TestLogger);
    log::set_max_level(log::LevelFilter::Trace);
}

pub struct TestLogger;

impl Log for TestLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        cfg!(test)
    }

    fn log(&self, record: &log::Record) {
        _ = io::stdout().write_fmt(format_args!("{}\n", record.args()))
    }

    fn flush(&self) {
        _ = io::stdout().flush()
    }
}
