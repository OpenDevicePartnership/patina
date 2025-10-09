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

//! # Patina SMBIOS Component
//!
//! Safe, allocation-aware management of SMBIOS tables for Patina / UEFI
//! environments. Provides:
//!
//! * `SmbiosManager` – in-memory validated record store with handle reuse
//! * `SmbiosRecords` trait – safe interface (add/update/remove/enumerate)
//! * Optional C protocol installation (for EDKII compatibility)
//! * Record builder & strongly-typed high‑level record structs in
//!   [`smbios_record`]
//!
//! ## Quick Start
//!
//! ```ignore
//! use patina_smbios::smbios_derive::{SmbiosManager, SmbiosTableHeader, SMBIOS_HANDLE_PI_RESERVED};
//! use patina_smbios::smbios_derive::SmbiosRecords;
//!
//! // Create a manager for SMBIOS 3.9
//! let mut mgr = SmbiosManager::new(3, 9);
//!
//! // Minimal record: Type 1 (System Information) with no strings
//! let record: [u8; 6] = [1, 4, 0, 0, 0, 0]; // type, length, handle(0,0 placeholder), double null
//! let handle = mgr.add_from_bytes(None, &record).unwrap();
//! assert_eq!(handle, 1);
//!
//! // Enumerate
//! let mut enum_handle = SMBIOS_HANDLE_PI_RESERVED; // start enumeration
//! let (hdr, _prod) = mgr.get_next(&mut enum_handle, None).unwrap();
//! assert_eq!(hdr.record_type, 1);
//! ```
//!
//! ## String Update Example
//! ```ignore
//! use patina_smbios::smbios_derive::{SmbiosManager, SmbiosRecords};
//! let mut mgr = SmbiosManager::new(3, 9);
//! // Type 1 with a single string "Old" ("Old\0\0")
//! let rec = [1,4,0,0,b'O',b'l',b'd',0,0];
//! let h = mgr.add_from_bytes(None, &rec).unwrap();
//! mgr.update_string(h, 1, "New").unwrap();
//! ```
//!
//! ## Safety Notes
//! * All validation (header length, string pool shape, string length) is performed
//!   in `add_from_bytes`.
//! * String updates rebuild only the string region, preserving structured bytes.
//! * Handle allocation reuses freed handles in O(1) time (free list) avoiding
//!   unbounded growth.
pub mod component;
/// SMBIOS derive functionality and manager
pub mod smbios_derive;
/// SMBIOS record structures and traits
pub mod smbios_record;

pub use component::SmbiosConfiguration;

#[cfg(test)]
mod tests {
    extern crate alloc;
    use crate::smbios_derive::{SMBIOS_HANDLE_PI_RESERVED, SmbiosManager, SmbiosRecords};
    use alloc::vec::Vec;

    // Helper to create a minimal record (no strings) of given type.
    fn minimal_record(record_type: u8) -> [u8; 6] {
        // type, length(=4), handle placeholder (0,0), double null terminator
        [record_type, 4, 0, 0, 0, 0]
    }

    #[test]
    fn print_record_bytes() {
        // Use the SmbiosTableHeader defined in the smbios_derive module
        let header = crate::smbios_derive::SmbiosTableHeader {
            record_type: 0x01,
            length: core::mem::size_of::<crate::smbios_derive::SmbiosTableHeader>() as u8,
            handle: 0x1234,
        };

        let data: Vec<u8> = Vec::from([0xAAu8, 0xBBu8, 0x00u8, 0x00u8]);

        // Serialize header bytes
        let header_size = core::mem::size_of::<crate::smbios_derive::SmbiosTableHeader>();
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

    #[test]
    fn add_minimal_and_enumerate() {
        let mgr = SmbiosManager::new(3, 9);
        let rec = minimal_record(1);
        let handle = mgr.add_from_bytes(None, &rec).expect("add minimal record");
        assert_eq!(handle, 1, "First allocated handle should be 1");

        // Enumerate starting from reserved value
        let mut enum_handle = SMBIOS_HANDLE_PI_RESERVED;
        let (hdr, _prod) = mgr.get_next(&mut enum_handle, None).expect("get_next should find record");
        // Copy out of packed struct to avoid unaligned reference errors
        let rt = hdr.record_type;
        let h_copy = hdr.handle;
        assert_eq!(rt, 1);
        assert_eq!(h_copy, handle);
        assert_eq!(mgr.version(), (3, 9));
    }

    #[test]
    fn add_with_string_and_update() {
        let mgr = SmbiosManager::new(3, 9);
        // Type 1 + one string "Old" => header(4 bytes) + "Old\0" + final \0
        let rec = [1, 4, 0, 0, b'O', b'l', b'd', 0, 0];
        let h = mgr.add_from_bytes(None, &rec).unwrap();
        // Update first string
        mgr.update_string(h, 1, "NewName").expect("update string 1");
        // Updating non-existent 2nd string should error
        assert!(mgr.update_string(h, 2, "Other").is_err());
    }

    #[test]
    fn handle_reuse_after_remove() {
        let mgr = SmbiosManager::new(3, 9);
        let h1 = mgr.add_from_bytes(None, &minimal_record(1)).unwrap();
        let h2 = mgr.add_from_bytes(None, &minimal_record(2)).unwrap();
        assert_eq!((h1, h2), (1, 2));
        mgr.remove(h1).expect("remove first");
        let h3 = mgr.add_from_bytes(None, &minimal_record(3)).unwrap();
        assert_eq!(h3, h1, "Expect freed handle to be reused");
    }
}
