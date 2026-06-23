//! UEFI Advanced Logger Protocol Support
//!
//! This module provides the component to initialize and publish the advanced
//! logger
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::boxed::Box;
use patina::{
    boot_services::{BootServices, StandardBootServices},
    component::{
        component,
        service::{Service, perf_timer::ArchTimerFunctionality},
    },
    error::{EfiError, Result},
    serial::SerialIO,
};
use r_efi::efi;

use crate::{logger::AdvancedLogger, protocol::AdvancedLoggerProtocol};

/// C struct for the internal Advanced Logger protocol for the component.
#[repr(C)]
struct AdvancedLoggerProtocolInternal<S>
where
    S: SerialIO + Send + 'static,
{
    // The public protocol that external callers will depend on.
    protocol: AdvancedLoggerProtocol,

    // Internal component access only! Does not exist in C definition.
    adv_logger: &'static AdvancedLogger<'static, S>,
}

/// The component that will install the Advanced Logger protocol.
pub struct AdvancedLoggerComponent<S>
where
    S: SerialIO + Send + 'static,
{
    adv_logger: &'static AdvancedLogger<'static, S>,
}

#[component]
impl<S> AdvancedLoggerComponent<S>
where
    S: SerialIO + Send + 'static,
{
    /// Creates a new AdvancedLoggerComponent.
    pub const fn new(adv_logger: &'static AdvancedLogger<S>) -> Self {
        Self { adv_logger }
    }

    /// EFI API to write to the advanced logger through the advanced logger protocol.
    extern "efiapi" fn adv_log_write(
        this: *const AdvancedLoggerProtocol,
        error_level: usize,
        buffer: *const u8,
        num_bytes: usize,
    ) -> efi::Status {
        if this.is_null() || buffer.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: `buffer` is null-checked above. We have no choice but to trust the caller
        //         on the buffer size. Convert to a slice for internal use.
        let data = unsafe { core::slice::from_raw_parts(buffer, num_bytes) };
        let error_level = error_level as u32;

        // SAFETY: `this` is null-checked above. The protocol struct is installed by Patina with
        //         `Box::leak`, so its alignment and validity are guaranteed.
        let internal = unsafe { &*(this as *const AdvancedLoggerProtocolInternal<S>) };

        internal.adv_logger.log_write(error_level, None, data);
        efi::Status::SUCCESS
    }

    /// Entry point to the AdvancedLoggerComponent.
    ///
    /// Installs the Advanced Logger Protocol for use by non-local components.
    ///
    fn entry_point(self, bs: StandardBootServices, timer: Service<dyn ArchTimerFunctionality>) -> Result<()> {
        let Some(address) = self.adv_logger.get_log_address() else {
            log::error!("Advanced logger not initialized before component entry point!");
            return Err(EfiError::NotStarted);
        };

        self.adv_logger.init_timer(timer);

        let protocol = AdvancedLoggerProtocolInternal {
            protocol: AdvancedLoggerProtocol::new(Self::adv_log_write, address),
            adv_logger: self.adv_logger,
        };

        let protocol = Box::leak(Box::new(protocol));
        match bs.install_protocol_interface(None, &mut protocol.protocol) {
            Err(status) => {
                log::error!("Failed to install Advanced Logger protocol! Status = {status}");
                Err(EfiError::ProtocolError)
            }
            Ok(_) => {
                log::info!("Advanced Logger protocol installed.");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::{
        logger::{AdvancedLogger, TargetFilter},
        writer::AdvancedLogWriter,
    };
    use patina::{
        component::service::{IntoService, Service, perf_timer::ArchTimerFunctionality},
        log::Format,
        serial::uart::UartNull,
    };
    use serial_test::serial;

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

    static TEST_LOGGER: AdvancedLogger<UartNull> = AdvancedLogger::new(
        Format::Standard,
        &[TargetFilter { target: "", log_level: log::LevelFilter::Trace, hw_filter_override: None }],
        log::LevelFilter::Trace,
        UartNull {},
    );

    /// Initializes `TEST_LOGGER` with a newly allocated memory log and a mock timer.
    fn init_test_logger() {
        const LOG_LEN: usize = 0x2000;
        let log_buff = Box::into_raw(Box::new([0_u8; LOG_LEN]));
        let log_address = log_buff as *const u8 as efi::PhysicalAddress;
        // SAFETY: This memory was just allocated, so it is valid for LOG_LEN bytes.
        unsafe { AdvancedLogWriter::initialize_memory_log(log_address, LOG_LEN as u32) };
        TEST_LOGGER.set_log_info_address(log_address);
        TEST_LOGGER.init_timer(Service::mock(Box::new(MockTimer {})));
    }

    /// Builds a leaked `AdvancedLoggerProtocolInternal` structure and returns a `*const AdvancedLoggerProtocol`
    /// that can be passed as `this` to `adv_log_write`.
    fn leak_protocol_this() -> *const AdvancedLoggerProtocol {
        let internal = Box::leak(Box::new(AdvancedLoggerProtocolInternal::<UartNull> {
            protocol: AdvancedLoggerProtocol::new(
                AdvancedLoggerComponent::<UartNull>::adv_log_write,
                TEST_LOGGER.get_log_address().unwrap_or(0),
            ),
            adv_logger: &TEST_LOGGER,
        }));
        &raw const internal.protocol
    }

    #[test]
    fn adv_log_write_null_this_returns_invalid_parameter() {
        let data = b"hello";
        let status =
            AdvancedLoggerComponent::<UartNull>::adv_log_write(core::ptr::null(), 0, data.as_ptr(), data.len());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn adv_log_write_null_buffer_returns_invalid_parameter() {
        // `this` is dangling but non-null. The function returns before dereferencing it
        // because the buffer null check is tripped first.
        let this = core::ptr::dangling::<AdvancedLoggerProtocol>();
        // Non-zero length.
        let status = AdvancedLoggerComponent::<UartNull>::adv_log_write(this, 0, core::ptr::null(), 4);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        // Zero length still invalid because `slice::from_raw_parts` requires a non-null pointer
        // even for zero-length slices.
        let status = AdvancedLoggerComponent::<UartNull>::adv_log_write(this, 0, core::ptr::null(), 0);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    // This is serialized since it mutates the `test` module-level `TEST_LOGGER` static.
    #[test]
    #[serial(adv_logger_test)]
    fn adv_log_write_normal_path_succeeds() {
        init_test_logger();
        let this = leak_protocol_this();

        let data = b"hello, advanced logger";
        let status = AdvancedLoggerComponent::<UartNull>::adv_log_write(this, 0, data.as_ptr(), data.len());
        assert_eq!(status, efi::Status::SUCCESS);
    }
}
