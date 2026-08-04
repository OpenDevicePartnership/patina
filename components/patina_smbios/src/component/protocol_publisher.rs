//! SMBIOS Protocol Publisher Component
//!
//! Defines the component that installs the SMBIOS protocol for C/EDK II driver compatibility.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::service::Smbios;
use patina::{
    component::{
        component,
        service::{Service, uefi_services::protocol::ProtocolServices},
    },
    error::Result,
};

/// Installs the SMBIOS protocol so C drivers can add and query SMBIOS records.
///
/// Depends on [`Service<dyn Smbios>`](Smbios), produced by
/// [`SmbiosProvider`](crate::component::provider::SmbiosProvider).
///
/// # Example
///
/// ```ignore
/// commands.add_component(SmbiosProtocolPublisher::new());
/// ```
#[derive(Default)]
pub struct SmbiosProtocolPublisher;

#[component]
impl SmbiosProtocolPublisher {
    /// Creates a new instance of the component.
    pub const fn new() -> Self {
        Self
    }

    /// Installs the SMBIOS protocol.
    fn entry_point(self, smbios: Service<dyn Smbios>, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        let (major_version, minor_version) = smbios.version();
        crate::manager::install_smbios_protocol(major_version, minor_version, smbios, &protocols)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    extern crate std;
    use alloc::boxed::Box;
    use patina::component::service::uefi_services::protocol::{Handle, MockProtocolServices};

    use crate::service::MockSmbios;

    #[test]
    fn test_smbios_protocol_publisher_entry_point_installs_protocol() {
        let mut smbios_mock = MockSmbios::new();
        smbios_mock.expect_version().return_const((3u8, 9u8));

        let mut protocols = MockProtocolServices::new();
        protocols
            .expect_install_interface()
            .once()
            .returning(|_, _, _| Ok(Handle::from_raw(core::ptr::dangling_mut::<core::ffi::c_void>()).unwrap()));

        let result = SmbiosProtocolPublisher::new()
            .entry_point(Service::mock(Box::new(smbios_mock)), Service::mock(Box::new(protocols)));

        assert!(result.is_ok());
    }
}
