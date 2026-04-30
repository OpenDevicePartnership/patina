//! UEFI RAM Disk Protocol bindings.
//!
//! Used to install a contiguous range of system memory as a virtual block device the system
//! firmware can boot from.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use r_efi::efi;

use crate::uefi_protocol::ProtocolInterface;

/// Well-known `RamDiskType` GUID for a generic virtual disk.
pub const RAM_DISK_VIRTUAL_DISK_GUID: efi::Guid =
    efi::Guid::from_fields(0x77ab535a, 0x45fc, 0x624b, 0x55, 0x60, &[0xf7, 0xb2, 0x81, 0xd1, 0xf9, 0x6e]);

/// Well-known `RamDiskType` GUID for a virtual CD (ISO9660) image.
pub const RAM_DISK_VIRTUAL_CD_GUID: efi::Guid =
    efi::Guid::from_fields(0x3d5abd30, 0x4175, 0x87ce, 0x6d, 0x64, &[0xd2, 0xad, 0xe5, 0x23, 0xc4, 0xbb]);

/// Well-known `RamDiskType` GUID for a persistent virtual disk.
pub const RAM_DISK_PERSISTENT_VIRTUAL_DISK_GUID: efi::Guid =
    efi::Guid::from_fields(0x5cea02c9, 0x4d07, 0x69d3, 0x26, 0x9f, &[0x44, 0x96, 0xfb, 0xe0, 0x96, 0xf9]);

/// Well-known `RamDiskType` GUID for a persistent virtual CD.
pub const RAM_DISK_PERSISTENT_VIRTUAL_CD_GUID: efi::Guid =
    efi::Guid::from_fields(0x08018188, 0x42cd, 0xbb48, 0x10, 0x0f, &[0x53, 0x87, 0xd5, 0x3d, 0xed, 0x3d]);

/// FFI type for `EFI_RAM_DISK_REGISTER_RAMDISK`.
///
/// Installs a `[ram_disk_base, ram_disk_base + ram_disk_size)` memory range as a RAM disk of
/// type `ram_disk_type`. When `parent_device_path` is null, the protocol creates a virtual
/// device-path root; otherwise the new RAM disk is appended under the supplied path. On
/// success the resulting device path is written to `*device_path` (caller-owned, do not free
/// before calling `Unregister`).
pub type RegisterFn = extern "efiapi" fn(
    ram_disk_base: u64,
    ram_disk_size: u64,
    ram_disk_type: *mut efi::Guid,
    parent_device_path: *mut efi::protocols::device_path::Protocol,
    device_path: *mut *mut efi::protocols::device_path::Protocol,
) -> efi::Status;

/// FFI type for `EFI_RAM_DISK_UNREGISTER_RAMDISK`.
///
/// Removes a previously-installed RAM disk identified by the device path returned from
/// `Register`. The backing memory is **not** freed by this call — the caller owns its
/// lifetime.
pub type UnregisterFn = extern "efiapi" fn(device_path: *mut efi::protocols::device_path::Protocol) -> efi::Status;

/// FFI binding for `EFI_RAM_DISK_PROTOCOL`.
#[repr(C)]
pub struct Protocol {
    /// Installs a host memory range as a RAM disk and produces its device path.
    pub register: RegisterFn,
    /// Removes a previously-installed RAM disk identified by its device path.
    pub unregister: UnregisterFn,
}

// SAFETY: Layout matches the UEFI RAM Disk Protocol struct (two efiapi function pointers).
// PROTOCOL_GUID matches the UEFI-spec value (AB38A0DF-6873-44A9-87E6-D4EB56148449).
unsafe impl ProtocolInterface for Protocol {
    const PROTOCOL_GUID: crate::BinaryGuid = crate::BinaryGuid::from_string("AB38A0DF-6873-44A9-87E6-D4EB56148449");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi_protocol::ProtocolInterface;

    #[test]
    fn protocol_guid_matches_uefi_spec() {
        // EFI_RAM_DISK_PROTOCOL_GUID per the UEFI specification.
        let expected =
            efi::Guid::from_fields(0xab38a0df, 0x6873, 0x44a9, 0x87, 0xe6, &[0xd4, 0xeb, 0x56, 0x14, 0x84, 0x49]);
        assert_eq!(
            *Protocol::PROTOCOL_GUID.0.as_bytes(),
            *expected.as_bytes(),
            "RAM Disk protocol GUID must match the UEFI specification"
        );
    }

    #[test]
    fn ram_disk_type_guids_are_distinct() {
        let virtual_disk = *RAM_DISK_VIRTUAL_DISK_GUID.as_bytes();
        let virtual_cd = *RAM_DISK_VIRTUAL_CD_GUID.as_bytes();
        let persistent = *RAM_DISK_PERSISTENT_VIRTUAL_DISK_GUID.as_bytes();
        let persistent_cd = *RAM_DISK_PERSISTENT_VIRTUAL_CD_GUID.as_bytes();

        assert_ne!(virtual_disk, virtual_cd);
        assert_ne!(virtual_disk, persistent);
        assert_ne!(virtual_disk, persistent_cd);
        assert_ne!(virtual_cd, persistent);
        assert_ne!(virtual_cd, persistent_cd);
        assert_ne!(persistent, persistent_cd);
    }
}
