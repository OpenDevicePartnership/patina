//! MMRAM Regions
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

/// Anchor object placed in the supervisor's own image.
static IMAGE_ANCHOR: u8 = 0;

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
/// them, so a single anchor point is what is available this early.
pub(crate) fn supervisor_image_anchor() -> u64 {
    &raw const IMAGE_ANCHOR as u64
}

/// Derives the MMRAM range from `scanned` and requires those descriptors to cover `anchor`.
///
/// Every other MMRAM containment check resolves against the producer's own description, so it
/// cannot detect a description that is wrong as a whole. Requiring the description to contain an
/// address the CPU independently proves is MMRAM is the one check that can, and combined with the
/// contiguity requirement it confines a forged HOB list to extending the span the supervisor is
/// genuinely running in.
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
    /// # Panics
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
/// # Panics
///
/// Panics when `range` is only partly inside MMRAM, as [`MmramPlacement::is_inside`] does.
pub(crate) fn is_range_inside_mmram(range: &MemoryRange) -> bool {
    security_state().page_allocator().classify_mmram(range).is_some_and(|p| p.is_inside(range))
}

/// Returns whether `range` touches MMRAM at all, including a range that only crosses a boundary.
///
/// Reports an overlap when the regions are not known yet, since nothing can be shown to lie
/// outside MMRAM before then.
pub(crate) fn range_overlaps_mmram(range: &MemoryRange) -> bool {
    !matches!(security_state().page_allocator().classify_mmram(range), Some(MmramPlacement::Outside))
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use crate::test_support;

    use super::*;
    use std::panic::catch_unwind;

    #[test]
    fn test_mmram_bound_errors_render_each_variant_distinctly() {
        // The anchor is the address the CPU proved is MMRAM, so a mismatch names it: that value
        // is what tells a platform owner which descriptors the HOB list got wrong.
        assert_eq!(
            format!("{}", MmramBoundError::AnchorOutsideRegions { anchor: 0x7000_0000 }),
            "the scanned descriptors do not cover the supervisor image anchor 0x0000000070000000"
        );
        assert_eq!(
            format!("{}", MmramBoundError::NoSmrrRange),
            "no scanned region meets the SMRR base and size requirements"
        );
        assert_ne!(
            format!("{}", MmramBoundError::AnchorOutsideRegions { anchor: 0x1000 }),
            format!("{}", MmramBoundError::AnchorOutsideRegions { anchor: 0x2000 })
        );
    }

    /// An 8 MiB region at 8 MiB, which meets the SMRR base and size requirements.
    const SMRR_REGION: MemoryRange = match MemoryRange::new(0x80_0000, 0x80_0000) {
        Ok(range) => range,
        Err(_) => panic!("the test region is a valid range"),
    };

    #[test]
    fn test_establish_mmram_bound_returns_the_derived_range() {
        let regions = [MmramRegion::new(SMRR_REGION, false)];

        assert_eq!(establish_mmram_bound(&regions, 0x80_1000), Ok(SMRR_REGION));
    }

    #[test]
    fn test_establish_mmram_bound_rejects_descriptors_that_do_not_cover_the_anchor() {
        // A HOB list describing MMRAM somewhere other than where the supervisor is executing is
        // refused outright, and before a range is selected from it. This region has no SMRR
        // range, so reporting the anchor shows that the anchor is checked first.
        let regions = [MmramRegion::new(MemoryRange::new(0x1000, 0x2000).unwrap(), false)];

        assert_eq!(
            establish_mmram_bound(&regions, 0x4000),
            Err(MmramBoundError::AnchorOutsideRegions { anchor: 0x4000 }.into())
        );
    }

    #[test]
    fn test_establish_mmram_bound_covers_a_region_end_to_end() {
        let regions = [MmramRegion::new(SMRR_REGION, false)];

        assert_eq!(establish_mmram_bound(&regions, SMRR_REGION.base()), Ok(SMRR_REGION));
        assert_eq!(establish_mmram_bound(&regions, SMRR_REGION.last()), Ok(SMRR_REGION));
        assert_eq!(
            establish_mmram_bound(&regions, SMRR_REGION.end()),
            Err(MmramBoundError::AnchorOutsideRegions { anchor: SMRR_REGION.end() }.into())
        );
    }

    #[test]
    fn test_establish_mmram_bound_rejects_regions_without_an_smrr_range() {
        // The region covers the anchor, but its size is not a power of two.
        let regions = [MmramRegion::new(MemoryRange::new(0x80_0000, 0x60_0000).unwrap(), false)];

        assert_eq!(establish_mmram_bound(&regions, 0x80_0000), Err(MmramBoundError::NoSmrrRange.into()));
    }

    #[test]
    fn test_supervisor_image_anchor_points_into_the_supervisor_image() {
        // The anchor is only meaningful if it is a real address in this image.
        assert_eq!(supervisor_image_anchor(), &raw const IMAGE_ANCHOR as u64);
        assert_ne!(supervisor_image_anchor(), 0);
    }

    /// Builds SMRAM descriptors from `(base, size, pre_allocated)` triples.
    fn regions_from(specs: &[(u64, u64, bool)]) -> Vec<MmramRegion> {
        specs.iter().map(|&(base, size, pre)| MmramRegion::new(MemoryRange::new(base, size).unwrap(), pre)).collect()
    }

    #[test]
    fn test_mmram_regions_contains_address() {
        let regions = regions_from(&[(0x1000, 0x2000, false), (0x8000, 0x1000, true)]);

        assert!(regions.contains_address(0x1000));
        assert!(regions.contains_address(0x2fff));
        // Pre-allocated regions still describe MMRAM, so they anchor just as well.
        assert!(regions.contains_address(0x8000));
        assert!(!regions.contains_address(0x0fff));
        assert!(!regions.contains_address(0x3000));
        assert!(!regions.contains_address(0x9000));
        assert!(![].contains_address(0x1000));
    }

    #[test]
    fn test_mmram_placement_from_coverage() {
        assert_eq!(MmramPlacement::from(Coverage::Full), MmramPlacement::Inside);
        assert_eq!(MmramPlacement::from(Coverage::Partial), MmramPlacement::PartlyInside);
        assert_eq!(MmramPlacement::from(Coverage::Disjoint), MmramPlacement::Outside);
    }

    #[test]
    fn test_mmram_placement_resolves_to_containment() {
        let range = MemoryRange::new(0x1000, 0x1000).unwrap();
        assert!(MmramPlacement::Inside.is_inside(&range));
        assert!(!MmramPlacement::Outside.is_inside(&range));
    }

    #[test]
    fn test_mmram_placement_rejects_a_range_crossing_the_boundary() {
        test_support::init_test_logger();

        // A range that is neither wholly in nor wholly out of MMRAM has no safe reading, so it is
        // reported as the configuration error it is rather than resolved either way.
        let range = MemoryRange::new(0x1000, 0x1000).unwrap();
        let result = catch_unwind(|| MmramPlacement::PartlyInside.is_inside(&range));
        assert!(result.is_err());
    }

    #[test]
    fn test_is_range_inside_mmram_fails_closed_before_allocator_initialization() {
        assert!(!is_range_inside_mmram(&MemoryRange::new(0x1000, 0x1000).unwrap()));
    }

    #[test]
    fn test_range_overlaps_mmram_fails_closed_before_allocator_initialization() {
        // Nothing can be shown to lie outside MMRAM before the regions are known, so a range the
        // supervisor is asked to treat as non-MM memory is reported as overlapping instead.
        assert!(range_overlaps_mmram(&MemoryRange::new(0x1000, 0x1000).unwrap()));
    }
}
