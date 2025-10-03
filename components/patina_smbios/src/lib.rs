//! SMBIOS (System Management BIOS) component for Patina
//!
//! This crate provides safe Rust abstractions for working with SMBIOS tables in UEFI environments.

#![no_std]
#![allow(missing_docs)] // TODO: Add comprehensive documentation

pub mod component;
/// SMBIOS derive functionality and manager
pub mod smbios_derive;
/// SMBIOS record structures and traits
pub mod smbios_record;

pub use component::SmbiosConfiguration;
pub use patina_smbios_macro::SmbiosRecord;

// Simplified test: construct a header, serialize it to bytes, append data and print
#[cfg(test)]
mod tests {
    extern crate std;
    // Bring test-friendly std items into scope
    use std::{print, println, vec::Vec};

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

        // Print bytes as hex; run tests with `-- --nocapture` to see this output
        print!("Record bytes ({}):", bytes.len());
        for b in &bytes {
            print!(" {:02X}", b);
        }
        println!();

        // Verify the handle (0x1234) little-endian bytes are present
        assert!(bytes.contains(&0x34));
        assert!(bytes.contains(&0x12));
    }
}
