//! SMBIOS components.
//!
//! SMBIOS support is split into two components:
//!
//! - [`provider::SmbiosProvider`] creates the SMBIOS manager and registers the `Service<dyn Smbios>` that platform
//!   components use to add and publish SMBIOS records.
//! - [`protocol_publisher::SmbiosProtocolPublisher`] installs the SMBIOS protocol for C driver
//!   compatibility. It depends on the `Service<dyn Smbios>` that `SmbiosProvider` registers.
//!
//! A platform registers both:
//!
//! ```ignore
//! commands.add_component(SmbiosProvider::new(3, 9));
//! commands.add_component(SmbiosProtocolPublisher::new());
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

pub mod protocol_publisher;
pub mod provider;
