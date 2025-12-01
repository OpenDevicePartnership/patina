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
        IntoComponent,
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
#[derive(IntoComponent)]
pub struct AdvancedLoggerComponent<S>
where
    S: SerialIO + Send + 'static,
{
    adv_logger: &'static AdvancedLogger<'static, S>,
}

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
        // SAFETY: We have no choice but to trust the caller on the buffer size. convert
        //         to a reference for internal safety.
        let data = unsafe { core::slice::from_raw_parts(buffer, num_bytes) };
        let error_level = error_level as u32;

        // SAFETY: We must trust the C code was a responsible steward of this buffer.
        let internal = unsafe { &*(this as *const AdvancedLoggerProtocolInternal<S>) };

        internal.adv_logger.log_write(error_level, data);
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
                log::error!("Failed to install Advanced Logger protocol! Status = {status:#x?}");
                Err(EfiError::ProtocolError)
            }
            Ok(_) => {
                log::info!("Advanced Logger protocol installed.");
                Ok(())
            }
        }
    }
}
