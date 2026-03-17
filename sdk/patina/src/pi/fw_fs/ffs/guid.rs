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

// {8C8CE578-8A3D-4F1C-9935-896185C32DD3}
/// Firmware File System version 2 GUID identifier per PI Specification
pub const EFI_FIRMWARE_FILE_SYSTEM2_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("8C8CE578-8A3D-4F1C-9935-896185C32DD3");

// {5473C07A-3DCB-4DCA-BD6F-1E9689E7349A}
/// Firmware File System version 3 GUID identifier per PI Specification
pub const EFI_FIRMWARE_FILE_SYSTEM3_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("5473C07A-3DCB-4DCA-BD6F-1E9689E7349A");

// {1BA0062E-C779-4582-8566-336AE8F78F09}
/// GUID for the file at the top of a firmware volume
pub const EFI_FFS_VOLUME_TOP_FILE_GUID: crate::BinaryGuid =
    crate::BinaryGuid::from_string("1BA0062E-C779-4582-8566-336AE8F78F09");
