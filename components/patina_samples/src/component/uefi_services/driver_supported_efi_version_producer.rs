//! Driver Supported EFI Version Producer Sample Component
//!
//! Demonstrates producing `EFI_DRIVER_SUPPORTED_EFI_VERSION_PROTOCOL` from a component, using
//! [`install_uefi_driver_model_driver_supported_efi_version`].
//!
//! [`install_uefi_driver_model_driver_supported_efi_version`]: patina::component::service::uefi_services::protocol::ProtocolServicesExt::install_uefi_driver_model_driver_supported_efi_version
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    component::{
        component,
        service::{
            Service,
            uefi_services::protocol::{ProtocolServices, ProtocolServicesExt},
        },
    },
    error::Result,
};

/// The UEFI Specification revision this sample conforms to.
///
/// Encoded the same way as `EFI_TABLE_HEADER.Revision`: the upper 16 bits are the major revision
/// and the lower 16 bits are the minor revision. `(2 << 16) | 0x0B` is UEFI 2.11.
const FIRMWARE_VERSION: u32 = (2 << 16) | 0x0B;

/// Installs the driver supported EFI version protocol on a generated handle.
#[derive(Default)]
pub struct DriverSupportedEfiVersionProducerSample;

#[component]
impl DriverSupportedEfiVersionProducerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        protocols.install_uefi_driver_model_driver_supported_efi_version(None, FIRMWARE_VERSION)?;
        Ok(())
    }
}
