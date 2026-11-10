//! MM Protected Region HOB
//!
//! Payload of the protected-region HOB, which names MMIO regions outside MMRAM that MM code
//! must not modify, such as the IOMMU registers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use zerocopy::FromBytes;
use zerocopy_derive::{Immutable, IntoBytes};

/// GUID of the MM protected-region HOB.
pub(crate) const MM_PROTECTED_REGION_HOB_GUID: patina::BinaryGuid =
    patina::BinaryGuid::from_string("6c0792ac-13d7-431b-a489-032f4af97380");

/// Protected region types. Only the IOMMU type is supported; HOB validation rejects any other.
pub(crate) enum ProtectedRegionType {
    /// The IOMMU MMIO registers.
    Iommu = 0,
}

/// One protected region (`MM_PROT_REGION_HOB`), the payload of a protected-region HOB.
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
