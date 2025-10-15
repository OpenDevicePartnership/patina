//! SMBIOS (System Management BIOS) component for Patina
//!
//! This crate provides safe Rust abstractions for working with SMBIOS tables in UEFI environments.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

#![no_std]
#![allow(missing_docs)] // TODO: Add comprehensive documentation

pub mod component;
/// SMBIOS manager and protocol implementation
pub mod manager;
/// SMBIOS record structures and traits
pub mod smbios_record;

pub use component::SmbiosConfiguration;

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec::Vec;

    #[test]
    fn print_record_bytes() {
        // Use the SmbiosTableHeader defined in the manager module
        let header = crate::manager::SmbiosTableHeader {
            record_type: 1,
            length: core::mem::size_of::<crate::manager::SmbiosTableHeader>() as u8,
            handle: 0x1234,
        };

        let data: Vec<u8> = Vec::from([0xAAu8, 0xBBu8, 0x00u8, 0x00u8]);

        // Serialize header bytes
        let header_size = core::mem::size_of::<crate::manager::SmbiosTableHeader>();
        let mut bytes: Vec<u8> = Vec::with_capacity(header_size + data.len());
        unsafe {
            let hb = core::slice::from_raw_parts(&header as *const _ as *const u8, header_size);
            bytes.extend_from_slice(hb);
        }
        bytes.extend_from_slice(&data);

        // Verify the handle (0x1234) little-endian bytes are present
        assert!(bytes.contains(&0x34));
        assert!(bytes.contains(&0x12));
    }
}
