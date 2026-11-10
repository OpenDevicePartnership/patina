//! MM Protected Region HOB
//!
//! Defines the payload of the protected-region HOB, which names MMIO regions outside MMRAM that
//! MM code must not modify, such as the IOMMU's registers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use zerocopy::FromBytes;
use zerocopy_derive::{Immutable, IntoBytes};

/// GUID for the MM protected-region HOB (`gMmProtectedRegionHobGuid`).
///
/// `{ 0x6c0792ac, 0x13d7, 0x431b, { 0xa4, 0x89, 0x03, 0x2f, 0x4a, 0xf9, 0x73, 0x80 } }`
pub(crate) const MM_PROTECTED_REGION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("6c0792ac-13d7-431b-a489-032f4af97380");

/// Types of protected region (`MM_PROTECTED_REGION_TYPE`).
///
/// The producer only applies the IOMMU type. It rejects `MM_PROT_MMIO_SEC_BIO_T` and any other
/// value, so those are not listed.
pub(crate) enum ProtectedRegionType {
    /// The IOMMU's MMIO registers (`MM_PROT_MMIO_IOMMU_T`).
    Iommu = 0,
}

/// One protected region (`MM_PROT_REGION_HOB`), the payload of each protected-region HOB.
#[repr(C)]
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable)]
pub(crate) struct MmProtectedRegion {
    /// Type of the protected region, one of [`ProtectedRegionType`].
    pub protected_region_type: u64,
    /// Base address of the protected region.
    pub protected_region_base: u64,
    /// Size of the protected region in UEFI pages.
    pub protected_region_pages: u64,
}
