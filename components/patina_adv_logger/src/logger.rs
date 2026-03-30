//! UEFI Advanced Logger Support
//!
//! This module provides a struct that implements log::Log for writing to a SerialIO
//! and the advanced logger memory log. This module is written to be phase agnostic.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::{
    memory_log::{self, LogEntry},
    writer::AdvancedLogWriter,
};
use core::{ffi::c_void, marker::Send, ptr};
use log::Level;
use patina::{
    component::service::{Service, perf_timer::ArchTimerFunctionality},
    error::EfiError,
    log::Format,
    pi::hob::{Hob, PhaseHandoffInformationTable},
    serial::SerialIO,
};
use r_efi::efi;
use spin::RwLock;

// Exists for the debugger to find the log buffer.
#[used]
static mut DBG_ADV_LOG_BUFFER: u64 = 0;

/// Per-target filter that binds a target name prefix with its log level and optional hardware print level override.
pub struct TargetFilter<'a> {
    /// Target name prefix to match.
    pub target: &'a str,
    /// Maximum log level for this target. Messages above this are dropped entirely.
    pub log_level: log::LevelFilter,
    /// Optional override for the hardware print level for this target. Messages above this level will not be printed
    /// to the hardware port, but may still be logged to the memory log based on log_level and the overall max_level.
    /// - `None` = use global `hw_print_level` from memory log header.
    /// - `Some(level_filter)` Use the provided level filter to control hardware printing for this target, instead
    ///   of the global `hw_print_level`.
    pub hw_filter_override: Option<log::LevelFilter>,
}

/// The logger for memory/hardware port logging.
pub struct AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    hardware_port: S,
    target_filters: &'a [TargetFilter<'a>],
    max_level: log::LevelFilter,
    format: Format,
    memory_log: RwLock<Option<AdvancedLogWriter>>,
    pub(crate) timer: Service<dyn ArchTimerFunctionality>,
}

impl<'a, S> AdvancedLogger<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new AdvancedLogger.
    ///
    /// ## Arguments
    ///
    /// * `format` - The format to use for logging.
    /// * `target_filters` - Per-target filters that control log level and optionally the hardware print filter.
    /// * `max_level` - The maximum log level to log.
    /// * `hardware_port` - The hardware port to write logs to.
    ///
    pub const fn new(
        format: Format,
        target_filters: &'a [TargetFilter<'a>],
        max_level: log::LevelFilter,
        hardware_port: S,
    ) -> Self {
        Self {
            hardware_port,
            target_filters,
            max_level,
            format,
            memory_log: RwLock::new(None),
            timer: Service::new_uninit(),
        }
    }

    /// Initializes the performance timer service for timestamping log entries.
    /// Should only be called once during setup.
    pub fn init_timer(&self, timer: Service<dyn ArchTimerFunctionality>) {
        self.timer.replace(&timer);
    }

    /// Initialize the advanced logger.
    ///
    /// Initializes the advanced logger memory log based on the provided physical hob
    /// list. The physical hob list is used so this can be initialized before memory
    /// allocations.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that the provided physical hob list pointer is valid and well structured. Failure to do
    /// so may result in unexpected memory access and undefined behavior.
    ///
    pub unsafe fn init(&self, physical_hob_list: *const c_void) -> Result<(), EfiError> {
        debug_assert!(!physical_hob_list.is_null(), "Could not initialize adv logger due to null hob list.");
        let hob_list_info =
            // SAFETY: The caller must provide a valid physical HOB list pointer.
            unsafe { (physical_hob_list as *const PhaseHandoffInformationTable).as_ref() }.ok_or_else(|| {
                log::error!("Could not initialize adv logger due to null hob list.");
                EfiError::InvalidParameter
            })?;
        let hob_list = Hob::Handoff(hob_list_info);
        for hob in &hob_list {
            if let Hob::GuidHob(guid_hob, data) = hob
                && guid_hob.name == memory_log::ADV_LOGGER_HOB_GUID
            {
                // SAFETY: The HOB will have a address of the log info
                // immediately following the HOB header.
                unsafe {
                    let address: *const efi::PhysicalAddress = ptr::from_ref(data) as *const efi::PhysicalAddress;
                    let log_info_addr = (*address) as efi::PhysicalAddress;
                    self.set_log_info_address(log_info_addr);
                };
                return Ok(());
            }
        }

        Err(EfiError::NotFound)
    }

    /// Writes a log entry to the hardware port and memory log if available.
    ///
    /// `hw_print_mask_override` optionally overrides the global hw_print_level
    /// from the memory log header, enabling per-target hardware print filtering.
    pub(crate) fn log_write(&self, error_level: u32, hw_print_mask_override: Option<u32>, data: &[u8]) {
        self.refresh_log_info_address();
        let mut hw_write = true;
        let log_guard = self.memory_log.read();
        if let Some(memory_log) = log_guard.as_ref() {
            hw_write = match hw_print_mask_override {
                Some(mask) => memory_log.hardware_write_enabled_with_mask(error_level, mask),
                None => memory_log.hardware_write_enabled(error_level),
            };
            let timestamp = self.timer.map_or(0, |timer| timer.cpu_count());
            let _ = memory_log.add_log_entry(LogEntry {
                phase: memory_log::ADVANCED_LOGGER_PHASE_DXE,
                level: error_level,
                timestamp,
                data,
            });
        }

        if hw_write {
            self.hardware_port.write(data);
        }
    }

    /// Sets the address of the advanced logger memory log.
    pub(crate) fn set_log_info_address(&self, address: efi::PhysicalAddress) {
        {
            // If already initialized with the same address, there is nothing to do
            let log_guard = self.memory_log.read();
            if log_guard.as_ref().is_some_and(|log| log.get_address() == address) {
                return;
            }
        }

        // SAFETY: The caller must ensure the address is valid for an AdvancedLogWriter type.
        if let Some(log) = unsafe { AdvancedLogWriter::adopt_memory_log(address) } {
            let current_frequency = log.get_frequency();

            {
                let mut memory_log_guard = self.memory_log.write();
                *memory_log_guard = Some(log);
            }
            // Drop the lock before logging

            log::info!("Advanced logger buffer initialized. Address = {:#x}", address);

            // The frequency may not be initialized, if not do so now.
            if current_frequency == 0 {
                let frequency = self.timer.map_or(0, |timer| timer.perf_frequency());
                // Re-acquire lock to set frequency
                let log_guard = self.memory_log.read();
                if let Some(memory_log) = log_guard.as_ref() {
                    memory_log.set_frequency(frequency);
                }
            }

            // SAFETY: This is only set for discoverability while debugging.
            unsafe {
                DBG_ADV_LOG_BUFFER = address;
            }
        } else {
            log::error!("Failed to initialize on existing advanced logger buffer!");
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_log_address(&self) -> Option<efi::PhysicalAddress> {
        let log_guard = self.memory_log.read();
        log_guard.as_ref().map(|log| log.get_address())
    }

    fn refresh_log_info_address(&self) {
        let (current_address, new_address) = {
            let log_guard = self.memory_log.read();
            let Some(log) = log_guard.as_ref() else {
                return;
            };
            (log.get_address(), log.get_new_logger_info_address())
        };

        if let Some(new_address) = new_address
            && new_address != current_address
        {
            self.set_log_info_address(new_address);
        }
    }

    /// Returns the matching target filter for the given target name, if any.
    fn target_filter(&self, target: &str) -> Option<&TargetFilter<'a>> {
        self.target_filters.iter().find(|f| target.starts_with(f.target))
    }
}

impl<S> log::Log for AdvancedLogger<'_, S>
where
    S: SerialIO + Send,
{
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let max_level = self.target_filter(metadata.target()).map(|f| f.log_level).unwrap_or(self.max_level);
        metadata.level().to_level_filter() <= max_level
    }

    fn log(&self, record: &log::Record) {
        let filter = self.target_filter(record.target());
        let max_level = filter.map(|f| f.log_level).unwrap_or(self.max_level);

        if record.metadata().level().to_level_filter() <= max_level {
            let level = log_level_to_debug_level(record.metadata().level());
            let hw_print_mask_override = filter.and_then(|f| f.hw_filter_override).map(log_level_filter_to_debug_mask);
            let mut writer = BufferedWriter::new(level, hw_print_mask_override, self);
            self.format.write(&mut writer, record);
            writer.flush();
        }
    }

    fn flush(&self) {
        // Do nothing
    }
}

/// Converts a log::Level to a EFI Debug Level.
const fn log_level_to_debug_level(level: Level) -> u32 {
    match level {
        Level::Error => memory_log::DEBUG_LEVEL_ERROR,
        Level::Warn => memory_log::DEBUG_LEVEL_WARNING,
        Level::Info => memory_log::DEBUG_LEVEL_INFO,
        Level::Trace => memory_log::DEBUG_LEVEL_VERBOSE,
        Level::Debug => memory_log::DEBUG_LEVEL_INFO,
    }
}

/// Converts a `log::LevelFilter` to a hardware print mask.
const fn log_level_filter_to_debug_mask(level_filter: log::LevelFilter) -> u32 {
    match level_filter {
        log::LevelFilter::Error => memory_log::DEBUG_LEVEL_ERROR,
        log::LevelFilter::Warn => memory_log::DEBUG_LEVEL_ERROR | memory_log::DEBUG_LEVEL_WARNING,
        log::LevelFilter::Info => {
            memory_log::DEBUG_LEVEL_ERROR | memory_log::DEBUG_LEVEL_WARNING | memory_log::DEBUG_LEVEL_INFO
        }
        log::LevelFilter::Debug | log::LevelFilter::Trace => {
            memory_log::DEBUG_LEVEL_ERROR
                | memory_log::DEBUG_LEVEL_WARNING
                | memory_log::DEBUG_LEVEL_INFO
                | memory_log::DEBUG_LEVEL_VERBOSE
        }
        log::LevelFilter::Off => 0,
    }
}

/// Size of the buffer for the buffered writer.
const WRITER_BUFFER_SIZE: usize = 128;

/// A wrapper for buffering and redirecting writes from the formatter.
struct BufferedWriter<'a, S>
where
    S: SerialIO + Send,
{
    level: u32,
    hw_print_mask_override: Option<u32>,
    writer: &'a AdvancedLogger<'a, S>,
    buffer: [u8; WRITER_BUFFER_SIZE],
    buffer_size: usize,
}

impl<'a, S> BufferedWriter<'a, S>
where
    S: SerialIO + Send,
{
    /// Creates a new BufferedWriter with the specified log level, optional hardware print mask override, and writer.
    const fn new(level: u32, hw_print_mask_override: Option<u32>, writer: &'a AdvancedLogger<'a, S>) -> Self {
        Self { level, hw_print_mask_override, writer, buffer: [0; WRITER_BUFFER_SIZE], buffer_size: 0 }
    }

    /// Flushes the current buffer to the underlying writer.
    fn flush(&mut self) {
        if self.buffer_size == 0 {
            return;
        }

        let data = &self.buffer[0..self.buffer_size];
        self.writer.log_write(self.level, self.hw_print_mask_override, data);
        self.buffer_size = 0;
    }
}

impl<S> core::fmt::Write for BufferedWriter<'_, S>
where
    S: SerialIO + Send,
{
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let data = s.as_bytes();
        let len = data.len();

        // buffer the message if it will fit.
        if len < WRITER_BUFFER_SIZE {
            // If it will not fit with the current data, flush the current data.
            if len > WRITER_BUFFER_SIZE - self.buffer_size {
                self.flush();
            }
            self.buffer[self.buffer_size..self.buffer_size + len].copy_from_slice(data);
            self.buffer_size += len;
        } else {
            // this message is too big to buffer, flush then write the message.
            self.flush();
            self.writer.log_write(self.level, self.hw_print_mask_override, data);
        }

        Ok(())
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use core::{ffi::c_void, ptr};

    use alloc::boxed::Box;
    use log::Log;
    use patina::{
        component::service::{IntoService, perf_timer::ArchTimerFunctionality},
        log::Format,
        pi::hob::{GUID_EXTENSION, GuidHob, header},
        serial::uart::UartNull,
    };
    use r_efi::efi;

    use crate::{
        logger::{AdvancedLogger, TargetFilter},
        memory_log,
        writer::AdvancedLogWriter,
    };

    #[derive(IntoService)]
    #[service(dyn ArchTimerFunctionality)]
    struct MockTimer {}

    impl ArchTimerFunctionality for MockTimer {
        fn perf_frequency(&self) -> u64 {
            100
        }
        fn cpu_count(&self) -> u64 {
            200
        }
    }

    #[test]
    fn test_uninit() {
        let serial = UartNull {};
        let logger_uninit = AdvancedLogger::<UartNull>::new(
            Format::Standard,
            &[TargetFilter { target: "test_target", log_level: log::LevelFilter::Info, hw_filter_override: None }],
            log::LevelFilter::Debug,
            serial,
        );
        assert!(logger_uninit.timer.map_or(0, |timer| timer.cpu_count()) == 0);
    }

    #[test]
    fn test_init() {
        let serial = UartNull {};
        let logger_uninit = AdvancedLogger::<UartNull>::new(
            Format::Standard,
            &[TargetFilter { target: "test_target", log_level: log::LevelFilter::Info, hw_filter_override: None }],
            log::LevelFilter::Debug,
            serial,
        );
        logger_uninit.init_timer(patina::component::service::Service::mock(Box::new(MockTimer {})));
        assert!(logger_uninit.timer.cpu_count() > 0);
    }

    static TEST_LOGGER: AdvancedLogger<UartNull> =
        AdvancedLogger::new(patina::log::Format::Standard, &[], log::LevelFilter::Trace, UartNull {});

    fn create_adv_logger_hob_list() -> (u64, *const c_void) {
        const LOG_LEN: usize = 0x2000;
        let log_buff = Box::into_raw(Box::new([0_u8; LOG_LEN]));
        let log_address = log_buff as *const u8 as efi::PhysicalAddress;

        // initialize the log so it's valid for the hob list
        //
        // SAFETY: We just allocated this memory so it's valid.
        unsafe { AdvancedLogWriter::initialize_memory_log(log_address, LOG_LEN as u32) };

        const HOB_LEN: usize = size_of::<GuidHob>() + size_of::<efi::PhysicalAddress>();
        let hob_buff = Box::into_raw(Box::new([0_u8; HOB_LEN]));
        let hob = hob_buff as *mut GuidHob;

        // SAFETY: We just allocated this memory so it's valid.
        unsafe {
            ptr::write(
                hob,
                GuidHob {
                    header: header::Hob { r#type: GUID_EXTENSION, length: HOB_LEN as u16, reserved: 0 },
                    name: memory_log::ADV_LOGGER_HOB_GUID,
                },
            )
        };

        // SAFETY: Space for the additional physical address was explicitly allocated.
        let address: *mut efi::PhysicalAddress = unsafe { hob.add(1) } as *mut efi::PhysicalAddress;
        // SAFETY: There is space for this address, writing it out of the structure as the C implementation does.
        unsafe { (*address) = log_address };
        (log_address, hob_buff as *const c_void)
    }

    #[test]
    fn component_test() {
        let (log_address, hob_list) = create_adv_logger_hob_list();

        // SAFETY: The hob list created is valid for this test.
        let res = unsafe { TEST_LOGGER.init(hob_list) };
        assert_eq!(res, Ok(()));

        assert!(TEST_LOGGER.get_log_address().is_some_and(|addr| addr == log_address));

        // TODO: Need to mock the protocol interface but requires final component interface.
    }

    // Helper to build Metadata for a given target and level.
    fn metadata(target: &str, level: log::Level) -> log::Metadata<'_> {
        log::Metadata::builder().target(target).level(level).build()
    }

    // === Global level filtering (no target filters) ===

    #[test]
    fn enabled_respects_global_max_level() {
        let logger = AdvancedLogger::new(Format::Standard, &[], log::LevelFilter::Info, UartNull {});

        assert!(logger.enabled(&metadata("any", log::Level::Error)));
        assert!(logger.enabled(&metadata("any", log::Level::Warn)));
        assert!(logger.enabled(&metadata("any", log::Level::Info)));
        assert!(!logger.enabled(&metadata("any", log::Level::Debug)));
        assert!(!logger.enabled(&metadata("any", log::Level::Trace)));
    }

    #[test]
    fn enabled_at_trace_allows_everything() {
        let logger = AdvancedLogger::new(Format::Standard, &[], log::LevelFilter::Trace, UartNull {});

        assert!(logger.enabled(&metadata("x", log::Level::Error)));
        assert!(logger.enabled(&metadata("x", log::Level::Warn)));
        assert!(logger.enabled(&metadata("x", log::Level::Info)));
        assert!(logger.enabled(&metadata("x", log::Level::Debug)));
        assert!(logger.enabled(&metadata("x", log::Level::Trace)));
    }

    #[test]
    fn enabled_at_off_blocks_everything() {
        let logger = AdvancedLogger::new(Format::Standard, &[], log::LevelFilter::Off, UartNull {});

        assert!(!logger.enabled(&metadata("x", log::Level::Error)));
        assert!(!logger.enabled(&metadata("x", log::Level::Trace)));
    }

    // === Target filter level overrides ===

    #[test]
    fn target_filter_overrides_global_to_be_more_permissive() {
        let filters = [TargetFilter { target: "my_mod", log_level: log::LevelFilter::Info, hw_filter_override: None }];
        let logger = AdvancedLogger::new(Format::Standard, &filters, log::LevelFilter::Error, UartNull {});

        // Matching target uses the filter's level (Info)
        assert!(logger.enabled(&metadata("my_mod", log::Level::Info)));
        assert!(logger.enabled(&metadata("my_mod", log::Level::Error)));
        assert!(!logger.enabled(&metadata("my_mod", log::Level::Debug)));

        // Non-matching target falls back to global (Error)
        assert!(!logger.enabled(&metadata("other", log::Level::Info)));
        assert!(logger.enabled(&metadata("other", log::Level::Error)));
    }

    #[test]
    fn target_filter_restricts_below_global() {
        let filters = [TargetFilter { target: "noisy", log_level: log::LevelFilter::Error, hw_filter_override: None }];
        let logger = AdvancedLogger::new(Format::Standard, &filters, log::LevelFilter::Trace, UartNull {});

        // "noisy" target is restricted to Error only
        assert!(!logger.enabled(&metadata("noisy", log::Level::Info)));
        assert!(!logger.enabled(&metadata("noisy", log::Level::Warn)));
        assert!(logger.enabled(&metadata("noisy", log::Level::Error)));

        // Other targets use global Trace (everything passes)
        assert!(logger.enabled(&metadata("other", log::Level::Trace)));
    }

    #[test]
    fn target_filter_matches_by_prefix() {
        let filters =
            [TargetFilter { target: "my_crate", log_level: log::LevelFilter::Info, hw_filter_override: None }];
        // Global is Off so anything not matching the filter is blocked.
        let logger = AdvancedLogger::new(Format::Standard, &filters, log::LevelFilter::Off, UartNull {});

        // Prefix match
        assert!(logger.enabled(&metadata("my_crate::submod", log::Level::Info)));
        // Exact match (also a valid prefix)
        assert!(logger.enabled(&metadata("my_crate", log::Level::Info)));
        // No match → falls to global Off
        assert!(!logger.enabled(&metadata("other_crate", log::Level::Error)));
    }
}
