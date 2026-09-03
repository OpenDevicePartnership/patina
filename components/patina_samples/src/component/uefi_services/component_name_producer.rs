//! Component Name Producer Sample Component
//!
//! Demonstrates producing `EFI_COMPONENT_NAME2_PROTOCOL` from a component, using
//! [`install_uefi_driver_model_component_name`].
//!
//! [`install_uefi_driver_model_component_name`]: patina::component::service::uefi_services::protocol::ProtocolServicesExt::install_uefi_driver_model_component_name
//!
//! Note that this is used to produce the Component Name as defined in the UEFI Specification for
//! code participating in the UEFI Driver Model. "Component" in this context is not referring to
//! a Patina "component". It is just an unfortunate naming collision.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::{
    char16,
    component::{
        component,
        service::{
            Service,
            uefi_services::{
                component_name::UefiDriverModelComponentName,
                language::{LanguageEntry, LanguageTable},
                protocol::{ProtocolServices, ProtocolServicesExt},
            },
        },
    },
    error::Result,
};

static DRIVER_NAMES: LanguageTable =
    LanguageTable(&[LanguageEntry { iso639: b"eng", rfc4646: "en", name: char16!("Sample Driver") }]);

/// Reports this component's name in English.
///
/// `controller_name` is left at its default, which reports that this driver does not name any
/// controller. A bus driver that manages controllers would override it as well.
struct SampleComponentName;

impl UefiDriverModelComponentName for SampleComponentName {
    fn driver_name(&self) -> &'static LanguageTable {
        &DRIVER_NAMES
    }
}

/// Installs a component name protocol for a single, fixed driver name.
#[derive(Default)]
pub struct ComponentNameProducerSample;

#[component]
impl ComponentNameProducerSample {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(self, protocols: Service<dyn ProtocolServices>) -> Result<()> {
        protocols.install_uefi_driver_model_component_name(None, SampleComponentName)?;
        Ok(())
    }
}
