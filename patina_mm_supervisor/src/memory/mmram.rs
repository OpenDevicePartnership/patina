//! MMRAM Regions
//!
//! The MMRAM regions the HOB list describes, the single range the SMRRs cover, and the checks
//! that place an address range inside or outside MMRAM.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use patina::{MemoryRange, memory_range::Coverage};

use crate::error::MmSupervisorResult;
use crate::mmcore::smrr::coalesced_smrr_range;
use crate::state::security_state;

/// A region of MMRAM: its address range, and whether it was reported as pre-allocated in the
/// HOB list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MmramRegion {
    /// Physical address range of the region.
    pub range: MemoryRange,
    /// Whether the region was reported as pre-allocated (`EFI_ALLOCATED`).
    pub pre_allocated: bool,
}

impl MmramRegion {
    /// Creates a region.
    pub(crate) const fn new(range: MemoryRange, pre_allocated: bool) -> Self {
        Self { range, pre_allocated }
    }
}

/// Queries over a list of MMRAM regions.
pub(crate) trait MmramRegions {
    /// Returns whether any region contains `address`.
    fn contains_address(&self, address: u64) -> bool;
}

impl MmramRegions for [MmramRegion] {
    fn contains_address(&self, address: u64) -> bool {
        self.iter().any(|region| region.range.contains(address))
    }
}

/// Errors produced while establishing an MMRAM bound from the incoming SMRAM descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmramBoundError {
    /// The descriptors do not cover an address the CPU proves is MMRAM.
    AnchorOutsideRegions {
        /// The address that was expected to be covered.
        anchor: u64,
    },
    /// No scanned region meets the SMRR base and size requirements.
    NoSmrrRange,
}

impl core::error::Error for MmramBoundError {}

impl fmt::Display for MmramBoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnchorOutsideRegions { anchor } => {
                write!(f, "the scanned descriptors do not cover the supervisor image anchor 0x{anchor:016x}")
            }
            Self::NoSmrrRange => write!(f, "no scanned region meets the SMRR base and size requirements"),
        }
    }
}

/// Returns an address the CPU proves is inside MMRAM.
///
/// The supervisor executes in MM from an image the MM IPL loaded into MMRAM, so an address in its
/// own image is inside MMRAM whatever the HOB list claims. The SMRRs would be the natural source
/// for a complete bound, but the platform leaves them unprogrammed until the supervisor writes
/// them, so a single anchor point is what is available this early. The anchor is the address of
/// this function's own code.
pub(crate) fn supervisor_image_anchor() -> u64 {
    supervisor_image_anchor as fn() -> u64 as usize as u64
}

/// Derives the MMRAM range from `scanned` and requires those descriptors to cover `anchor`.
///
/// Every other MMRAM containment check resolves against the producer's own description, so it
/// cannot detect a description that is wrong as a whole. Requiring the description to contain an
/// address the CPU independently proves is MMRAM is the one check that can, and combined with the
/// contiguity requirement it confines a forged HOB list to extending the span the supervisor is
/// actually running in.
///
/// The range is the single range the SMRRs will cover, as selected by [`coalesced_smrr_range`].
pub(crate) fn establish_mmram_bound(scanned: &[MmramRegion], anchor: u64) -> MmSupervisorResult<MemoryRange> {
    if !scanned.contains_address(anchor) {
        return Err(MmramBoundError::AnchorOutsideRegions { anchor }.into());
    }

    let range = coalesced_smrr_range(scanned).ok_or(MmramBoundError::NoSmrrRange)?;
    log::info!("Discovered MMRAM range: {range}");
    Ok(range)
}

/// Where an address range sits relative to MMRAM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmramPlacement {
    /// Every byte of the range is MMRAM.
    Inside,
    /// No byte of the range is MMRAM.
    Outside,
    /// The range is partly MMRAM and partly not.
    PartlyInside,
}

impl MmramPlacement {
    /// Resolves this placement of `range` to whether `range` may be treated as MM memory.
    ///
    /// ## Panics
    ///
    /// Panics when the placement is [`MmramPlacement::PartlyInside`].
    pub fn is_inside(self, range: &MemoryRange) -> bool {
        match self {
            Self::Inside => true,
            Self::Outside => false,
            Self::PartlyInside => panic!("Buffer {range} crosses an MMRAM boundary"),
        }
    }
}

impl From<Coverage> for MmramPlacement {
    fn from(coverage: Coverage) -> Self {
        match coverage {
            Coverage::Full => Self::Inside,
            Coverage::Partial => Self::PartlyInside,
            Coverage::Disjoint => Self::Outside,
        }
    }
}

/// Returns whether `range` lies entirely inside MMRAM.
///
/// Reports `false` before the regions are known, since nothing can be shown to be inside MMRAM
/// until then.
///
/// ## Panics
///
/// Panics when `range` is only partly inside MMRAM, as [`MmramPlacement::is_inside`] does.
pub(crate) fn is_range_inside_mmram(range: &MemoryRange) -> bool {
    security_state().page_allocator().classify_mmram(range).is_some_and(|placement| placement.is_inside(range))
}

/// Returns whether `range` touches MMRAM at all, including a range that only crosses a boundary.
///
/// Reports an overlap when the regions are not known yet, since nothing can be shown to lie
/// outside MMRAM before then.
pub(crate) fn range_overlaps_mmram(range: &MemoryRange) -> bool {
    !matches!(security_state().page_allocator().classify_mmram(range), Some(MmramPlacement::Outside))
}
