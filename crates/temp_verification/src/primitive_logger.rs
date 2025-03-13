// get it because it's the opposite of advanced
// we will rename this later

#![no_std]

use core::fmt::{self, Write};
use log::{Level, Log, Metadata, Record, SetLoggerError};
use uefi_sdk::serial::SerialIO;

// abstract serial so we can later implement other output types
pub struct PrimitiveLogger<S: SerialIO> {
    serial: S,
}

impl<S: SerialIO> PrimitiveLogger<S> {
    /// Creates a new logger instance.
    pub const fn new(serial: S) -> Self {
        Self { serial }
    }
}

/// Implement `core::fmt::Write` to enable formatted writing.
impl<S: SerialIO> Write for PrimitiveLogger<S> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.serial.write(s.as_bytes());
        Ok(())
    }
}

const MAX_BUFFER_SIZE: usize = 256;

/// Implement the `log::Log` trait.
impl<S> Log for PrimitiveLogger<S>
where
    S: SerialIO + Send,
{
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info // figure out how to actually properly filter this
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut buffer = [0u8; MAX_BUFFER_SIZE]; // so this limits us to 256 bytes of log message (or whatever value we choose here in the end)
                                                     // if we allow memory allocation, i think we wouldn't have to limit the size
                                                     // in the other BufferedWriter, there's a workaround for this (i think?)
            let mut writer = BufferWriter::new(&mut buffer);
            let _ = writeln!(writer, "[{}] {}", record.level(), record.args());

            // Write the formatted log message to the serial port
            self.serial.write(writer.as_bytes());
        }
    }

    fn flush(&self) {}
}

// we can't use the bufferedwritter in adv_logger since we don't have access to it
// plus it requires an instance of adv_logger to actually use it
// so we will just implement our own
// according to chatgpt, buffering writes is faster than formatting output byte by byte (which makes sense)
// so we'll buffer instead of writing byte by byte
// this is assuming no memory allocation (similar to advanced logger)
// but if we have memory allocation, i think we could implement a less complicated version
struct BufferWriter<'a> {
    buffer: &'a mut [u8],
    pos: usize,
}

impl<'a> BufferWriter<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, pos: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buffer[..self.pos]
    }
}

impl<'a> Write for BufferWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let len = bytes.len().min(self.buffer.len() - self.pos);
        self.buffer[self.pos..self.pos + len].copy_from_slice(&bytes[..len]);
        self.pos += len;
        Ok(())
    }
}

// Global static logger instance
// static mut LOGGER: Option<SerialLogger<Uart16550>> = None;

// /// Initializes the logger.
// pub fn init_logger(serial: Uart16550) -> Result<(), SetLoggerError> {
//     unsafe {
//         LOGGER = Some(SerialLogger::new(serial));
//         log::set_logger(LOGGER.as_ref().unwrap())?;
//         log::set_max_level(log::LevelFilter::Info);
//     }
//     Ok(())
// }
