//! MMRAM Regions, the Bound Over Them, and Where a Range Sits
//!
//! [`MmramRegion`] is the supervisor's view of one region. [`establish_mmram_bound`] derives
//! the MMRAM range from the descriptors the MM IPL hands down, requiring them to cover an
//! address the CPU independently proves is MMRAM. That range is what the SMRRs are later
//! programmed with.
//!
//! The rest of the module answers the question every later caller asks of that bound: where
//! does an arbitrary range sit relative to it. [`MmramPlacement`] names the three answers, and
//! keeps "partly inside" distinct from "outside" because a range crossing the boundary is a
//! configuration error a caller must not silently accept.
//!
//! The two halves are one module because neither stands alone. Establishing the bound asks
//! whether the descriptors contain the anchor, and every placement answer is computed over the
//! regions the bound was established from.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::fmt;

use crate::error::MmSupervisorResult;
use crate::state::security_state;

/// Anchor object placed in the supervisor's own image.
static IMAGE_ANCHOR: u8 = 0;

/// A region of MMRAM: a physical base address, a size in bytes, and whether it
/// was reported as pre-allocated in the HOB list.
///
/// This is the supervisor's own view of a region, derived from the producer's
/// [`SmramDescriptor`](crate::memory::page_allocator::SmramDescriptor) but not a mirror of
/// it, which is why it takes the MMRAM spelling the rest of the established region uses.
///
/// The layout is `repr(C)` and the tail gap after `pre_allocated` is declared as the
/// explicit `reserved` field rather than left as implicit padding. A value of this type
/// is copied into the `INIT_STATE.smrr_range` static, which the SEA auxiliary file
/// validates byte for byte. Implicit padding has no defined value, so a move can carry
/// stack residue into it and the validator then reports a mismatch on bytes no code ever
/// wrote. Naming the gap makes it a real field that is always initialized and that can be
/// given its own validation rule.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MmramRegion {
    /// Physical base address of the region.
    pub base: u64,
    /// Size of the region, in bytes.
    pub size: u64,
    /// Whether the region was reported as pre-allocated (`EFI_ALLOCATED`).
    pub pre_allocated: bool,
    /// Explicit tail padding, always zero. See the note on the type.
    reserved: [u8; 7],
}

impl MmramRegion {
    /// Creates a region, zeroing the explicit tail padding.
    pub(crate) const fn new(base: u64, size: u64, pre_allocated: bool) -> Self {
        Self { base, size, pre_allocated, reserved: [0; 7] }
    }
}

/// Why the incoming SMRAM descriptors cannot be used as an MMRAM bound.
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
/// `coalesce_mmram_regions` folds the scanned regions into the single range the SMRRs will
/// cover, and reports `None` when no candidate meets the SMRR base and size requirements. It is
/// taken as a parameter so the anchor check can be driven in a test without the real selection
/// rules; production passes
/// [`coalesced_smrr_range`](crate::mmcore::smrr::coalesced_smrr_range).
pub(crate) fn establish_mmram_bound(
    scanned: &[MmramRegion],
    anchor: u64,
    coalesce_mmram_regions: impl FnOnce(&[MmramRegion]) -> Option<MmramRegion>,
) -> MmSupervisorResult<MmramRegion> {
    if !regions_contain(scanned, anchor) {
        return Err(MmramBoundError::AnchorOutsideRegions { anchor }.into());
    }

    let range = coalesce_mmram_regions(scanned).ok_or(MmramBoundError::NoSmrrRange)?;
    log::info!("Discovered MMRAM range: base=0x{:08x}, size=0x{:08x}", range.base, range.size);
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
    /// Resolves this placement to whether `[base, base + size)` may be treated as MM memory.
    ///
    /// ## Panics
    ///
    /// Panics on [`MmramPlacement::PartlyInside`]. Callers are deciding whether a
    /// firmware-described buffer is MM memory, and a range crossing the boundary is neither:
    /// trusting it exposes the MMRAM half, rejecting it silently leaves a producer describing a
    /// region it does not own. There is no correct reading, so it is the configuration error it
    /// looks like.
    pub fn is_inside(self, base: u64, size: u64) -> bool {
        match self {
            Self::Inside => true,
            Self::Outside => false,
            Self::PartlyInside => {
                let end = base.saturating_add(size);
                panic!("Buffer 0x{base:016x}-0x{end:016x} crosses an MMRAM boundary");
            }
        }
    }
}

/// Classifies `[addr, addr + size)` against regions given as `(base, exclusive end)` pairs.
///
/// Coverage is measured over the union of the regions, so a range spanning two adjacent regions
/// is [`MmramPlacement::Inside`] rather than partly inside. An empty range is
/// [`MmramPlacement::Outside`]; a range whose end overflows is reported as partly inside so a
/// malformed descriptor fails closed.
pub(crate) fn classify_coverage(
    addr: u64,
    size: u64,
    mut regions: impl Iterator<Item = (u64, u64)> + Clone,
) -> MmramPlacement {
    if size == 0 {
        return MmramPlacement::Outside;
    }
    let Some(end) = addr.checked_add(size) else {
        return MmramPlacement::PartlyInside;
    };

    // Extend the covered prefix one region at a time until a gap appears.
    let mut cursor = addr;
    while let Some(region_end) =
        regions.clone().find_map(|(base, region_end)| (cursor >= base && cursor < region_end).then_some(region_end))
    {
        cursor = region_end;
        if cursor >= end {
            return MmramPlacement::Inside;
        }
    }

    if cursor > addr || regions.any(|(base, region_end)| cursor < region_end && base < end) {
        MmramPlacement::PartlyInside
    } else {
        MmramPlacement::Outside
    }
}

/// Classifies `[addr, addr + size)` against SMRAM descriptors that have been scanned but not yet
/// committed to the allocator's bookkeeping.
pub(crate) fn classify_mmram_in_regions(regions: &[MmramRegion], addr: u64, size: u64) -> MmramPlacement {
    classify_coverage(
        addr,
        size,
        regions.iter().filter_map(|region| Some((region.base, region.base.checked_add(region.size)?))),
    )
}

/// Returns whether any region in `regions` contains `address`.
pub(crate) fn regions_contain(regions: &[MmramRegion], address: u64) -> bool {
    regions.iter().any(|region| {
        region.base <= address && region.base.checked_add(region.size).is_some_and(|region_end| address < region_end)
    })
}

/// Returns whether `[base, base + size)` lies entirely inside MMRAM.
///
/// Reports `false` before the regions are known, since nothing can be shown to be inside MMRAM
/// until then.
///
/// ## Panics
///
/// Panics if the range is only partly inside MMRAM; see [`MmramPlacement::is_inside`].
pub(crate) fn is_buffer_inside_mmram(base: u64, size: u64) -> bool {
    security_state().page_allocator().classify_mmram(base, size).is_some_and(|p| p.is_inside(base, size))
}

/// Returns whether `[base, base + size)` touches MMRAM at all, including a range that only
/// crosses a boundary.
///
/// Reports an overlap when the regions are not known yet, since nothing can be shown to lie
/// outside MMRAM before then.
pub(crate) fn buffer_overlaps_mmram(base: u64, size: u64) -> bool {
    !matches!(security_state().page_allocator().classify_mmram(base, size), Some(MmramPlacement::Outside))
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

    #[test]
    fn test_establish_mmram_bound_returns_the_derived_range() {
        let regions = [MmramRegion::new(0x1000, 0x2000, false)];
        let range = MmramRegion::new(0x1000, 0x2000, false);

        assert_eq!(establish_mmram_bound(&regions, 0x1500, |_| Some(range)), Ok(range));
    }

    #[test]
    fn test_establish_mmram_bound_rejects_descriptors_that_do_not_cover_the_anchor() {
        // A HOB list describing MMRAM somewhere other than where the supervisor is executing is
        // refused outright, and before a range is selected from it.
        let regions = [MmramRegion::new(0x1000, 0x2000, false)];
        let selected = core::cell::Cell::new(false);

        let result = establish_mmram_bound(&regions, 0x4000, |_| {
            selected.set(true);
            Some(MmramRegion::new(0x1000, 0x2000, false))
        });

        assert_eq!(result, Err(MmramBoundError::AnchorOutsideRegions { anchor: 0x4000 }.into()));
        assert!(!selected.get(), "a range was selected from descriptors that had already failed");
    }

    #[test]
    fn test_establish_mmram_bound_covers_a_region_end_to_end() {
        let regions = [MmramRegion::new(0x1000, 0x2000, false)];
        let range = MmramRegion::new(0x1000, 0x2000, false);

        assert!(establish_mmram_bound(&regions, 0x1000, |_| Some(range)).is_ok());
        assert!(establish_mmram_bound(&regions, 0x2fff, |_| Some(range)).is_ok());
        assert_eq!(
            establish_mmram_bound(&regions, 0x3000, |_| Some(range)),
            Err(MmramBoundError::AnchorOutsideRegions { anchor: 0x3000 }.into())
        );
    }

    #[test]
    fn test_establish_mmram_bound_rejects_regions_without_an_smrr_range() {
        let regions = [MmramRegion::new(0x1000, 0x2000, false)];

        assert_eq!(establish_mmram_bound(&regions, 0x1000, |_| None), Err(MmramBoundError::NoSmrrRange.into()));
    }

    #[test]
    fn test_supervisor_image_anchor_points_into_the_supervisor_image() {
        // The anchor is only meaningful if it is a real address in this image.
        assert_eq!(supervisor_image_anchor(), &raw const IMAGE_ANCHOR as u64);
        assert_ne!(supervisor_image_anchor(), 0);
    }

    /// Builds SMRAM descriptors from `(base, size, pre_allocated)` triples.
    fn regions_from(specs: &[(u64, u64, bool)]) -> Vec<MmramRegion> {
        specs.iter().map(|&(base, size, pre)| MmramRegion::new(base, size, pre)).collect()
    }

    #[test]
    fn test_regions_contain_address() {
        let regions = regions_from(&[(0x1000, 0x2000, false), (0x8000, 0x1000, true)]);

        assert!(regions_contain(&regions, 0x1000));
        assert!(regions_contain(&regions, 0x2fff));
        // Pre-allocated regions still describe MMRAM, so they anchor just as well.
        assert!(regions_contain(&regions, 0x8000));
        assert!(!regions_contain(&regions, 0x0fff));
        assert!(!regions_contain(&regions, 0x3000));
        assert!(!regions_contain(&regions, 0x9000));
        assert!(!regions_contain(&[], 0x1000));
    }

    #[test]
    fn test_regions_contain_address_does_not_overflow() {
        let regions = regions_from(&[(u64::MAX - 0xfff, 0x2000, false)]);

        // The declared extent wraps, so nothing can be shown to be inside it.
        assert!(!regions_contain(&regions, u64::MAX));
    }

    #[test]
    fn test_classify_mmram_in_regions() {
        let regions = regions_from(&[(0x1000, 0x1000, false), (0x2000, 0x1000, true)]);

        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0x1000), MmramPlacement::Inside);
        // Adjacent regions cover the range jointly, so this is wholly inside.
        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0x2000), MmramPlacement::Inside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x4000, 0x1000), MmramPlacement::Outside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x2800, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x0800, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&regions, 0x1000, 0), MmramPlacement::Outside);
        // A range whose end overflows fails closed.
        assert_eq!(classify_mmram_in_regions(&regions, u64::MAX, 0x1000), MmramPlacement::PartlyInside);
        assert_eq!(classify_mmram_in_regions(&[], 0x1000, 0x1000), MmramPlacement::Outside);
    }

    #[test]
    fn test_mmram_placement_resolves_to_containment() {
        assert!(MmramPlacement::Inside.is_inside(0x1000, 0x1000));
        assert!(!MmramPlacement::Outside.is_inside(0x1000, 0x1000));
    }

    #[test]
    fn test_mmram_placement_rejects_a_range_crossing_the_boundary() {
        test_support::init_test_logger();

        // A range that is neither wholly in nor wholly out of MMRAM has no safe reading, so it is
        // reported as the configuration error it is rather than resolved either way.
        let result = catch_unwind(|| MmramPlacement::PartlyInside.is_inside(0x1000, 0x1000));
        assert!(result.is_err());

        // A size that overflows the end address still produces a message rather than a second panic.
        let result = catch_unwind(|| MmramPlacement::PartlyInside.is_inside(u64::MAX, 0x1000));
        assert!(result.is_err());
    }

    #[test]
    fn test_is_buffer_inside_mmram_fails_closed_before_allocator_initialization() {
        assert!(!is_buffer_inside_mmram(0x1000, 0x1000));
        assert!(!is_buffer_inside_mmram(u64::MAX, 1));
    }
    #[test]
    fn test_buffer_overlaps_mmram_fails_closed_before_allocator_initialization() {
        // Nothing can be shown to lie outside MMRAM before the regions are known, so a buffer the
        // supervisor is asked to treat as non-MM memory is reported as overlapping instead.
        assert!(buffer_overlaps_mmram(0x1000, 0x1000));
        assert!(buffer_overlaps_mmram(u64::MAX, 1));
    }
}
