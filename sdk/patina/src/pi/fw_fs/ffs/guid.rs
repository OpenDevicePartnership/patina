//! Firmware File System (FFS) Guid Definitions
//!
//! Based on the values defined in the UEFI Platform Initialization (PI) Specification V1.8A Section 3.2.2.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

// {8C8CE578-8A3D-4F1C-9935-896185C32DD3}
pub const EFI_FIRMWARE_FILE_SYSTEM2_GUID: efi::Guid =
    efi::Guid::from_fields(0x8c8ce578, 0x8a3d, 0x4f1c, 0x99, 0x35, &[0x89, 0x61, 0x85, 0xc3, 0x2d, 0xd3]);

// {5473C07A-3DCB-4DCA-BD6F-1E9689E7349A}
pub const EFI_FIRMWARE_FILE_SYSTEM3_GUID: efi::Guid =
    efi::Guid::from_fields(0x5473c07a, 0x3dcb, 0x4dca, 0xbd, 0x6f, &[0x1e, 0x96, 0x89, 0xe7, 0x34, 0x9a]);

// {1BA0062E-C779-4582-8566-336AE8F78F09}
pub const EFI_FFS_VOLUME_TOP_FILE_GUID: efi::Guid =
    efi::Guid::from_fields(0x1ba0062e, 0xc779, 0x4582, 0x85, 0x66, &[0x33, 0x6a, 0xe8, 0xf7, 0x8f, 0x9]);
