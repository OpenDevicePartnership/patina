//! A module for core UEFI decompression functionality.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::boxed::Box;
use patina::{
    boot_services::BootServices,
    component::{Storage, component},
    error::EfiError,
    uefi_protocol::decompress,
};

/// Component to install the UEFI Decompress Protocol.
#[derive(Default)]
pub(crate) struct DecompressProtocolInstaller;

#[component]
impl DecompressProtocolInstaller {
    fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        let protocol = Box::new(decompress::EfiDecompressProtocol::new());

        match storage.boot_services().install_protocol_interface(None, protocol) {
            Ok(_) => Ok(()),
            Err(err) => EfiError::status_to_result(err),
        }
    }
}
