//! MMRAM (SMRAM) Memory Reserve Definitions
//!
//! Definitions describing the MMRAM regions that the HOB producer phase reports
//! to the MM foundation: the per-region descriptor, the `RegionState` bits that
//! qualify it, the ! GUID HOB payload that carries an array of descriptors, and
//! the GUIDs that identify those HOBs.
//!
//! These correspond to `EFI_MMRAM_DESCRIPTOR` / `EFI_SMRAM_DESCRIPTOR` and
//! `EFI_MMRAM_HOB_DESCRIPTOR_BLOCK` in the Platform Initialization (PI) Specification, and to
//! `gEfiSmmSmramMemoryGuid` / `gEfiMmPeiMmramMemoryReserveGuid` in EDK II.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::BinaryGuid;
use crate::standard::efi;
use zerocopy_derive::{FromBytes, Immutable};

/// `EFI_ALLOCATED` bit in [`SmramDescriptor::region_state`].
///
/// Set when the region has already been allocated by the HOB producer phase, so the MM core must
/// not hand it out again.
pub const EFI_ALLOCATED: u64 = 0x0000000000000010;

/// `gEfiSmmSmramMemoryGuid`
///
/// Identifies a GUID HOB whose payload is a [`SmramReserveHobData`] header followed by an array
/// of [`SmramDescriptor`] entries.
///
/// (`6DADF1D1-D4CC-4910-BB6E-82B1FD80FF3D`)
/// ```
/// # use patina::pi::mmram::SMM_SMRAM_MEMORY_GUID;
/// # assert_eq!("6DADF1D1-D4CC-4910-BB6E-82B1FD80FF3D", format!("{}", SMM_SMRAM_MEMORY_GUID));
/// ```
pub const SMM_SMRAM_MEMORY_GUID: BinaryGuid = BinaryGuid::from_string("6dadf1d1-d4cc-4910-bb6e-82b1fd80ff3d");

/// `gEfiMmPeiMmramMemoryReserveGuid`
///
/// The MM-era spelling of [`SMM_SMRAM_MEMORY_GUID`]; it identifies a HOB with the same payload
/// layout.
///
/// (`0703F912-BF8D-4E2A-BE07-AB272525C592`)
/// ```
/// # use patina::pi::mmram::MM_PEI_MMRAM_MEMORY_RESERVE_GUID;
/// # assert_eq!("0703F912-BF8D-4E2A-BE07-AB272525C592", format!("{}", MM_PEI_MMRAM_MEMORY_RESERVE_GUID));
/// ```
pub const MM_PEI_MMRAM_MEMORY_RESERVE_GUID: BinaryGuid =
    BinaryGuid::from_string("0703f912-bf8d-4e2a-be07-ab272525c592");

/// `EFI_SMRAM_DESCRIPTOR` / `EFI_MMRAM_DESCRIPTOR`
///
/// Describes a single region of MMRAM.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, FromBytes, Immutable)]
pub struct SmramDescriptor {
    /// Physical start address of the MMRAM region.
    pub physical_start: efi::PhysicalAddress,
    /// CPU start address (may differ from physical for remapping).
    pub cpu_start: efi::PhysicalAddress,
    /// Size of the MMRAM region in bytes.
    pub physical_size: u64,
    /// Region state flags ([`EFI_ALLOCATED`], etc.).
    pub region_state: u64,
}

/// `EFI_SMRAM_HOB_DESCRIPTOR_BLOCK` / `EFI_MMRAM_HOB_DESCRIPTOR_BLOCK`
///
/// The header of the payload carried by a GUID HOB named [`SMM_SMRAM_MEMORY_GUID`] or
/// [`MM_PEI_MMRAM_MEMORY_RESERVE_GUID`]. An array of `number_of_smram_regions`
/// [`SmramDescriptor`] entries follows immediately after it.
#[repr(C)]
#[derive(Clone, Copy, Debug, FromBytes, Immutable)]
pub struct SmramReserveHobData {
    /// Number of MMRAM descriptors that follow.
    pub number_of_smram_regions: u32,
    /// Reserved for alignment.
    pub reserved: u32,
    // SmramDescriptor array follows immediately after
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smram_structures_match_the_c_layout() {
        assert_eq!(core::mem::size_of::<SmramDescriptor>(), 32);
        assert_eq!(core::mem::offset_of!(SmramDescriptor, physical_start), 0);
        assert_eq!(core::mem::offset_of!(SmramDescriptor, cpu_start), 8);
        assert_eq!(core::mem::offset_of!(SmramDescriptor, physical_size), 16);
        assert_eq!(core::mem::offset_of!(SmramDescriptor, region_state), 24);

        assert_eq!(core::mem::size_of::<SmramReserveHobData>(), 8);
        assert_eq!(core::mem::offset_of!(SmramReserveHobData, number_of_smram_regions), 0);
        assert_eq!(core::mem::offset_of!(SmramReserveHobData, reserved), 4);
    }
}
