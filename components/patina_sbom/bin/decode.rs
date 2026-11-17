//! Decodes a saved ACPI SBOM table.
//!
//! Copyright (C) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use patina::uefi::acpi_table::{
    ACPI_DESCRIPTION_HEADER_LENGTH,
    sbom::{self, SbomTable},
};
use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    process::ExitCode,
};
use zerocopy::IntoBytes;

fn decode(bytes: &[u8], output: &mut impl Write) -> Result<(), Box<dyn Error>> {
    let (table, trailing) = SbomTable::parse_prefix(bytes)?;
    if !trailing.is_empty() {
        return Err("bytes remain after the declared ACPI table".into());
    }
    // Validate the entire stream before producing a success-shaped partial report.
    let mut count = 0;
    for entry in table.entries() {
        entry?;
        count += 1;
    }
    let header = table.header;
    writeln!(output, "Signature: SBOM")?;
    writeln!(output, "Length: 0x{:X}", u32::from_le(header.length))?;
    writeln!(output, "Revision: 0x{:X}", header.revision)?;
    writeln!(output, "Checksum: 0x{:02X} (valid)", header.checksum)?;
    writeln!(output, "OEM ID: {}", String::from_utf8_lossy(&header.oem_id))?;
    writeln!(output, "OEM table ID: {}", String::from_utf8_lossy(&header.oem_table_id))?;
    writeln!(output, "OEM revision: 0x{:08X}", u32::from_le(header.oem_revision))?;
    writeln!(output, "Creator ID: {}", String::from_utf8_lossy(&u32::from_le(header.creator_id).to_le_bytes()))?;
    writeln!(output, "Creator revision: 0x{:08X}", u32::from_le(header.creator_revision))?;
    writeln!(output, "Entries: {count}")?;
    let mut offset = ACPI_DESCRIPTION_HEADER_LENGTH;
    for (index, entry) in table.entries().enumerate() {
        let entry = entry?;
        writeln!(output, "Entry {index}: offset=0x{offset:X} length=0x{:X}", entry.as_bytes().len())?;
        writeln!(
            output,
            "  revision=0x{:X} header_length=0x{:X} payload_length=0x{:X}",
            entry.revision,
            u16::from_le(entry.header_length),
            u32::from_le(entry.payload_length)
        )?;
        let flags = if entry.flags == sbom::EFI_ACPI_SBOM_TABLE_ENTRY_FLAG_NONE { "None" } else { "Reserved" };
        let format = match entry.format {
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_COSWID => "CoSWID",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_CYCLONEDX => "CycloneDX",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_SPDX => "SPDX",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_FORMAT_VENDOR => "Vendor-defined",
            _ => "Unknown",
        };
        let compression = match entry.compression {
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_NONE => "None",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_ZLIB => "zlib",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_LZMA => "LZMA",
            sbom::EFI_ACPI_SBOM_TABLE_ENTRY_SBOM_COMPRESSION_VENDOR => "Vendor-defined",
            _ => "Unknown",
        };
        writeln!(
            output,
            "  flags={flags} (0x{:02X}) format={format} (0x{:02X}) compression={compression} (0x{:02X})",
            entry.flags, entry.format, entry.compression
        )?;
        for chunk in entry.payload.chunks(16) {
            write!(output, "  ")?;
            for byte in chunk {
                write!(output, "{byte:02X} ")?;
            }
            writeln!(output)?;
        }
        offset += entry.as_bytes().len();
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("usage: sbom-acpi-decode <table.bin>")?;
    if args.next().is_some() {
        return Err("usage: sbom-acpi-decode <table.bin>".into());
    }
    decode(&fs::read(path)?, &mut io::stdout().lock())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sbom-acpi-decode: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(entries: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; ACPI_DESCRIPTION_HEADER_LENGTH];
        bytes.splice(0..4, *b"SBOM");
        let length = u32::try_from(bytes.len() + entries.len()).unwrap();
        bytes.splice(4..8, length.to_le_bytes());
        bytes[8] = 1;
        bytes.extend_from_slice(entries);
        bytes[9] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
        bytes
    }

    #[test]
    fn test_sbom_decode_headers_entries_and_payload() {
        let entry = [4, 10, 0, 3, 0, 0, 0, 0, 2, 0, b'a', b'b', b'c'];
        let mut output = Vec::new();
        decode(&table(&entry), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Length: 0x31"));
        assert!(text.contains("Revision: 0x1"));
        assert!(text.contains("Entry 0: offset=0x24 length=0xD"));
        assert!(text.contains("revision=0x4 header_length=0xA payload_length=0x3"));
        assert!(text.contains("61 62 63"));
        assert!(text.contains("Entries: 1"));
        assert!(text.find("Entries: 1").unwrap() < text.find("Entry 0:").unwrap());
        assert!(text.contains("flags=None (0x00) format=SPDX (0x02) compression=None (0x00)"));
    }

    #[test]
    fn test_sbom_decode_metadata_names() {
        for (value, format, compression) in [
            (0, "CoSWID", "None"),
            (1, "CycloneDX", "zlib"),
            (2, "SPDX", "LZMA"),
            (0xFF, "Vendor-defined", "Vendor-defined"),
            (0x7F, "Unknown", "Unknown"),
        ] {
            let entry = [4, 10, 0, 0, 0, 0, 0, 1, value, value];
            let mut output = Vec::new();
            decode(&table(&entry), &mut output).unwrap();
            let text = String::from_utf8(output).unwrap();
            assert!(text.contains("flags=Reserved (0x01)"));
            assert!(text.contains(&format!("format={format} (0x{value:02X})")));
            assert!(text.contains(&format!("compression={compression} (0x{value:02X})")));
        }
    }

    #[test]
    fn test_sbom_decode_rejects_malformed_input() {
        let mut bytes = table(&[]);
        assert!(decode(&bytes, &mut Vec::new()).is_ok());
        bytes.push(0);
        assert!(decode(&bytes, &mut Vec::new()).is_err());
        assert!(decode(&table(&[4]), &mut Vec::new()).is_err());
        assert!(decode(b"SBOM", &mut Vec::new()).is_err());
        bytes[9] ^= 1;
        assert!(decode(&bytes[..36], &mut Vec::new()).is_err());
    }
}
