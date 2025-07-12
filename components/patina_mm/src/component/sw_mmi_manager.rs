//! Software Management Mode (MM) Interrupt Component
//!
//! Provides the `SwMmiTrigger` service to trigger software management mode interrupts (SWMMIs) in the MM environment.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use crate::config::{MmCommunicationConfiguration, MmiPort};
use crate::service::platform_mm_control::PlatformMmControl;
use patina_sdk::component::{
    params::{Commands, Config},
    service::{IntoService, Service},
    IntoComponent,
};

#[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "x86_64")))]
use x86_64::instructions::port;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Software Management Mode (MM) Interrupt Trigger Service
///
/// Provides a mechanism to trigger software management mode interrupts (MMIs) in the MM environment. These are
/// synchronous interrupts that can be used to signal MM handlers to perform specific tasks or operations usually
/// invoking a specific MM handler registered to handle MMI requests from a correspnding driver or component outside
/// of the MM environment.
///
/// ## Safety
///
/// This trait is unsafe because an implementation needs to ensure that the service is only invoked after hardware
/// initialization for MMIs is complete and that the system is in a safe state to handle MMIs.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub unsafe trait SwMmiTrigger {
    /// Triggers a software Management Mode Interrupt (MMI).
    ///
    /// ## Safety
    ///
    /// This function is unsafe because it may cause the system to enter a state where MMIs are not handled correctly.
    /// It is the caller's responsibility to ensure that the system is in a safe state before calling this function.
    unsafe fn trigger_sw_mmi(&self, cmd_port_value: u8, data_port_value: u8) -> patina_sdk::error::Result<()>;
}

/// A component that provides the `SwMmiTrigger` service.
#[derive(Debug, IntoComponent, IntoService)]
#[service(dyn SwMmiTrigger)]
pub struct SwMmiManager {
    inner_config: MmCommunicationConfiguration,
}

impl SwMmiManager {
    /// Create a new `SwMmiManager` instance.
    pub fn new() -> Self {
        Self { inner_config: MmCommunicationConfiguration::default() }
    }

    /// Initialize the `SwMmiManager` instance.
    ///
    /// Sets up the `SwMmiManager` with the provided configuration and registers it as a service. This function expects
    /// the platform to have initialized the MM environment prior to its execution. The platform may optionally provide
    /// a `PlatformMmControl` service that will be invoked before this component makes the `SwMmiTrigger` service
    /// available.
    fn entry_point(
        mut self,
        config: Config<MmCommunicationConfiguration>,
        platform_mm_control: Option<Service<dyn PlatformMmControl>>,
        mut commands: Commands,
    ) -> patina_sdk::error::Result<()> {
        log::debug!("Initializing SwMmiManager...");

        if platform_mm_control.is_some() {
            log::debug!("Platform MM Control is available. Calling platform-specific init...");
            platform_mm_control.unwrap().init()?;
        }

        self.inner_config = config.clone();

        commands.add_service(self);

        Ok(())
    }
}

unsafe impl SwMmiTrigger for SwMmiManager {
    unsafe fn trigger_sw_mmi(&self, _cmd_port_value: u8, _data_port_value: u8) -> patina_sdk::error::Result<()> {
        log::debug!("Triggering SW MMI...");

        match self.inner_config.cmd_port {
            MmiPort::Smi(_port) => {
                cfg_if::cfg_if! {
                    if #[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "x86_64")))] {
                        log::trace!("Writing SMI command port: {:#X}", _port);
                        unsafe { port::Port::new(_port).write(_cmd_port_value); }
                    }
                }
            }
            MmiPort::Smc(_smc_port) => {
                todo!("SMC communication not implemented yet.");
            }
        }

        match self.inner_config.data_port {
            MmiPort::Smi(_port) => {
                cfg_if::cfg_if! {
                    if #[cfg(any(feature = "doc", all(target_os = "uefi", target_arch = "x86_64")))] {
                        log::trace!("Writing SMI data port: {:#X}", _port);
                        unsafe { port::Port::new(_port).write(_data_port_value); }
                    }
                }
            }
            MmiPort::Smc(_smc_port) => {
                todo!("SMC communication not implemented yet.");
            }
        }

        log::debug!("SW MMI triggered.");

        Ok(())
    }
}

impl Default for SwMmiManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MmCommunicationConfiguration;
    use crate::service::platform_mm_control::{MockPlatformMmControl, PlatformMmControl};
    use patina_sdk::component::params::Commands;

    #[test]
    fn test_sw_mmi_manager_without_platform_mm_control() {
        let sw_mmi_manager = SwMmiManager::new();
        assert!(sw_mmi_manager
            .entry_point(Config::mock(MmCommunicationConfiguration::default()), None, Commands::mock())
            .is_ok());
    }

    #[test]
    fn test_sw_mmi_manager_with_platform_mm_control() {
        let sw_mmi_manager = SwMmiManager::new();

        let mut mock_platform_mm_control = MockPlatformMmControl::new();
        mock_platform_mm_control.expect_init().once().returning(|| Ok(()));
        let platform_mm_control_service: Service<dyn PlatformMmControl> =
            Service::mock(Box::new(mock_platform_mm_control));

        assert!(sw_mmi_manager
            .entry_point(
                Config::mock(MmCommunicationConfiguration::default()),
                Some(platform_mm_control_service),
                Commands::mock()
            )
            .is_ok());
    }
}
