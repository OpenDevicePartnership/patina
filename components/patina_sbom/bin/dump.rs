//! Captures the running system's ACPI SBOM table.
//!
//! Copyright (C) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use patina::uefi::acpi_table::sbom::SbomTable;
use std::{env, error::Error, fs::OpenOptions, io::Write, path::Path, process::ExitCode};

#[cfg(target_os = "linux")]
fn system_table() -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(std::fs::read("/sys/firmware/acpi/tables/SBOM")?)
}

#[cfg(target_os = "windows")]
fn system_table() -> Result<Vec<u8>, Box<dyn Error>> {
    use std::{ffi::c_void, io, ptr};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemFirmwareTable(provider: u32, table: u32, buffer: *mut c_void, size: u32) -> u32;
    }

    let provider = u32::from_be_bytes(*b"ACPI");
    let table = u32::from_le_bytes(*b"SBOM");
    // SAFETY: A null buffer and zero size query the required allocation length.
    let size = unsafe { GetSystemFirmwareTable(provider, table, ptr::null_mut(), 0) };
    if size == 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut bytes = vec![0u8; size as usize];
    // SAFETY: bytes is writable for exactly size bytes and remains alive during the call.
    let written = unsafe { GetSystemFirmwareTable(provider, table, bytes.as_mut_ptr().cast(), size) };
    if written == 0 {
        return Err(io::Error::last_os_error().into());
    }
    if written != size {
        return Err(format!("firmware table length changed from {size} to {written}; retry capture").into());
    }
    Ok(bytes)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn system_table() -> Result<Vec<u8>, Box<dyn Error>> {
    Err("system table capture is supported on Windows and Linux".into())
}

fn save_table(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let (table, trailing) = SbomTable::parse_prefix(bytes)?;
    if !trailing.is_empty() {
        return Err("bytes remain after the declared ACPI table".into());
    }
    for entry in table.entries() {
        entry?;
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let path = args.next().ok_or("usage: sbom-acpi-dump <output.bin>")?;
    if args.next().is_some() {
        return Err("usage: sbom-acpi-dump <output.bin>".into());
    }
    let bytes = system_table()?;
    save_table(Path::new(&path), &bytes)?;
    println!("Saved {} bytes to {}", bytes.len(), Path::new(&path).display());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sbom-acpi-dump: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sbom_dump_invalid_table_does_not_create_file() {
        let path = env::temp_dir().join(format!("patina-sbom-invalid-{}.bin", std::process::id()));
        assert!(save_table(&path, b"invalid").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn test_sbom_dump_saves_exact_bytes_and_preserves_existing_file() {
        let path = env::temp_dir().join(format!("patina-sbom-valid-{}.bin", std::process::id()));
        let mut bytes = vec![0u8; 36];
        bytes.splice(0..4, *b"SBOM");
        bytes[4] = 36;
        bytes[8] = 1;
        bytes[9] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
        save_table(&path, &bytes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(save_table(&path, &bytes).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }
}
