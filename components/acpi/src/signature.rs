use r_efi::efi;

/// Helpers for handling ACPI signatures

pub const FACS: u32 = 0x53434146;
pub const UEFI: u32 = 0x49464555;
pub const FACP: u32 = 0x50434146;
pub const DSDT: u32 = 0x54582350;
pub const XSDT: u32 = 0x54445358;

pub const ACPI_TABLE_GUID: efi::Guid =
    efi::Guid::from_fields(0x8868E871, 0xE4F1, 0x11D3, 0xBC, 0x22, &[0x00, 0x80, 0xC7, 0x3C, 0x88, 0x81]);

pub const ACPI_HEADER_LEN: usize = 36;
pub const MAX_INITIAL_ENTRIES: usize = 32;

pub const ACPI_RSDP_TABLE: u64 = 0x2052545020445352;
pub const ACPI_RSDP_REVISION: u8 = 2;

pub const ACPI_XSDT_REVISION: u8 = 1;

pub const ACPI_RESERVED_BYTE: u8 = 0x00;
