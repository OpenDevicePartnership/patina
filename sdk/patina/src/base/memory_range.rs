//! A non-empty, contiguous `[base, base + size)` address range whose exclusive end fits in a `u64`.
//!
//! ## Example
//!
//! ```rust
//! use patina::MemoryRange;
//!
//! let range = MemoryRange::new(0x1000, 0x2000)?;
//! assert!(range.contains(0x2fff));
//! assert_eq!(range.last(), 0x2fff);
//! assert_eq!(MemoryRange::from_pages(0x1000, 2, 0x1000)?, range);
//!
//! let other = MemoryRange::new(0x2000, 0x2000)?;
//! assert_eq!(range.intersection(&other), MemoryRange::new(0x2000, 0x1000));
//! assert_eq!(range + other, MemoryRange::new(0x1000, 0x3000));
//! assert_eq!(MemoryRange::collapse(&[range, other]), MemoryRange::new(0x1000, 0x3000));
//! assert_eq!(range.extend(0x1000), MemoryRange::new(0x1000, 0x3000));
//! # Ok::<(), patina::MemoryRangeError>(())
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{
    borrow::Borrow,
    cmp::Ordering,
    fmt,
    num::NonZeroU64,
    ops::{Add, Range, RangeInclusive},
};

use crate::base::error::EfiError;

/// Errors returned by [`MemoryRange`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRangeError {
    /// `base + size` would exceed `u64::MAX`.
    Overflow,
    /// The end bound is below the start bound.
    InvalidBounds,
    /// The alignment is not a non-zero power of two.
    InvalidAlignment,
    /// The ranges neither overlap nor touch.
    Disjoint,
    /// The resulting range would have a size of zero, or no ranges were given.
    Empty,
}

impl fmt::Display for MemoryRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryRangeError::Overflow => write!(f, "memory range extends past the end of the address space"),
            MemoryRangeError::InvalidBounds => write!(f, "memory range end is below its start"),
            MemoryRangeError::InvalidAlignment => write!(f, "alignment must be a non-zero power of two"),
            MemoryRangeError::Disjoint => write!(f, "memory ranges are neither overlapping nor adjacent"),
            MemoryRangeError::Empty => write!(f, "memory range is empty"),
        }
    }
}

impl core::error::Error for MemoryRangeError {}

impl From<MemoryRangeError> for EfiError {
    fn from(_: MemoryRangeError) -> Self {
        EfiError::InvalidParameter
    }
}

/// How much of a [`MemoryRange`] a set of ranges covers. Returned by [`MemoryRange::coverage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Every address in the range is covered.
    Full,
    /// Some addresses in the range are covered and some are not.
    Partial,
    /// No address in the range is covered.
    Disjoint,
}

/// A non-empty, contiguous `[base, base + size)` address range. `size` is never zero and `base + size` never exceeds
/// `u64::MAX`.
///
/// Ranges are ordered by base, then size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryRange {
    base: u64,
    size: u64,
}

impl MemoryRange {
    /// Creates a range of `size` bytes starting at `base`.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Empty`] if `size` is zero; [`MemoryRangeError::Overflow`] if `base + size` exceeds
    /// `u64::MAX`.
    pub const fn new(base: u64, size: u64) -> Result<Self, MemoryRangeError> {
        if size == 0 {
            return Err(MemoryRangeError::Empty);
        }
        match base.checked_add(size) {
            Some(_) => Ok(Self { base, size }),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Creates a range from `first` to `last`, inclusive.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::InvalidBounds`] if `last < first`; [`MemoryRangeError::Overflow`] if `last` is
    /// `u64::MAX`.
    pub const fn from_inclusive(first: u64, last: u64) -> Result<Self, MemoryRangeError> {
        if last < first {
            return Err(MemoryRangeError::InvalidBounds);
        }
        match last.checked_add(1) {
            Some(end) => Self::new(first, end - first),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Creates a range of `pages` pages of `page_size` bytes each, starting at `base`.
    ///
    /// `base` is not required to be aligned to `page_size`; use [`MemoryRange::is_aligned`] when that matters.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Empty`] if `pages` or `page_size` is zero; [`MemoryRangeError::Overflow`] if
    /// `pages * page_size` or `base + pages * page_size` exceeds `u64::MAX`.
    pub const fn from_pages(base: u64, pages: u64, page_size: u64) -> Result<Self, MemoryRangeError> {
        match pages.checked_mul(page_size) {
            Some(size) => Self::new(base, size),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Returns the first address.
    pub const fn base(&self) -> u64 {
        self.base
    }

    /// Returns the size in bytes, which is never zero.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Returns the base and the size as a `(base, size)` tuple.
    ///
    /// ```rust
    /// # use patina::MemoryRange;
    /// let (base, size) = MemoryRange::new(0x1000, 0x2000)?.into_parts();
    /// assert_eq!((base, size), (0x1000, 0x2000));
    /// # Ok::<(), patina::MemoryRangeError>(())
    /// ```
    pub const fn into_parts(self) -> (u64, u64) {
        (self.base, self.size)
    }

    /// Returns the exclusive end, `base + size`.
    pub const fn end(&self) -> u64 {
        // Cannot overflow: the constructors reject any `base + size` above `u64::MAX`.
        self.base + self.size
    }

    /// Returns the last address, `end - 1`.
    pub const fn last(&self) -> u64 {
        // Cannot underflow: `size` is never zero, so `end > base`.
        self.end() - 1
    }

    /// Returns `true` if `address` is in the range.
    pub const fn contains(&self, address: u64) -> bool {
        address >= self.base && address < self.end()
    }

    /// Returns `true` if `other` lies within `self` (`s` = `self`, `o` = `other`):
    ///
    /// ```text
    /// [s[o]]   - contained
    /// [s=o]    - contained (identical)
    /// [s[]o]   - not contained (o extends past s)
    /// [o[]s]   - not contained (o starts before s)
    /// [o[s]]   - not contained (o is larger)
    /// [s] [o]  - not contained
    /// ```
    pub const fn contains_range(&self, other: &MemoryRange) -> bool {
        other.base >= self.base && other.end() <= self.end()
    }

    /// Returns `true` if the ranges share an address (`s` = `self`, `o` = `other`):
    ///
    /// ```text
    /// [s] [o]  - not overlapping
    /// [s][o]   - not overlapping (adjacent)
    /// [s[]o]   - overlapping
    /// [s[o]]   - overlapping
    /// [o[s]]   - overlapping
    /// [o[]s]   - overlapping
    /// [o][s]   - not overlapping (adjacent)
    /// [o] [s]  - not overlapping
    /// ```
    pub const fn overlaps(&self, other: &MemoryRange) -> bool {
        self.base < other.end() && other.base < self.end()
    }

    /// Returns the shared part of the ranges (`s` = `self`, `o` = `other`, `[]` = shared part):
    ///
    /// ```text
    /// [s] [o]  - Err(Empty)
    /// [s][o]   - Err(Empty) (adjacent)
    /// [s[]o]   - Ok([])
    /// [s[o]]   - Ok(o)
    /// [o[s]]   - Ok(s)
    /// [o[]s]   - Ok([])
    /// ```
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Empty`] if the ranges don't overlap.
    pub const fn intersection(&self, other: &MemoryRange) -> Result<MemoryRange, MemoryRangeError> {
        let base = if self.base > other.base { self.base } else { other.base };
        let end = if self.end() < other.end() { self.end() } else { other.end() };
        // Without overlap `end <= base`, so the size saturates to zero and `new` reports `Empty`.
        Self::new(base, end.saturating_sub(base))
    }

    /// Returns `true` if one range ends exactly where the other begins (`s` = `self`, `o` = `other`):
    ///
    /// ```text
    /// [s][o]   - adjacent
    /// [o][s]   - adjacent
    /// [s] [o]  - not adjacent (gap)
    /// [s[]o]   - not adjacent (overlapping)
    /// ```
    pub const fn is_adjacent(&self, other: &MemoryRange) -> bool {
        self.end() == other.base || other.end() == self.base
    }

    /// Merges two overlapping or adjacent ranges; also available as `a + b` (`s` = `self`, `o` = `other`):
    ///
    /// ```text
    /// [s][o]   - Ok([s  o])
    /// [s[]o]   - Ok([s  o])
    /// [s[o]]   - Ok(s)
    /// [o[s]]   - Ok(o)
    /// [s] [o]  - Err(Disjoint)
    /// ```
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Disjoint`] if the ranges don't touch.
    pub const fn union(&self, other: &MemoryRange) -> Result<MemoryRange, MemoryRangeError> {
        if !self.overlaps(other) && !self.is_adjacent(other) {
            return Err(MemoryRangeError::Disjoint);
        }
        let base = if self.base < other.base { self.base } else { other.base };
        let end = if self.end() > other.end() { self.end() } else { other.end() };
        Self::new(base, end - base)
    }

    /// Returns the single range covering all of `ranges`, from the lowest base to the highest end. Gaps between the
    /// ranges are included:
    ///
    /// ```text
    /// input:   [c]   [a]  [b[]d]   (any order)
    /// output:  [c            d]
    /// ```
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Empty`] if `ranges` is empty.
    pub fn collapse<I>(ranges: I) -> Result<MemoryRange, MemoryRangeError>
    where
        I: IntoIterator,
        I::Item: Borrow<MemoryRange>,
    {
        let mut ranges = ranges.into_iter();
        let first = *ranges.next().ok_or(MemoryRangeError::Empty)?.borrow();
        let (base, end) = ranges.fold((first.base, first.end()), |(base, end), range| {
            let range = range.borrow();
            (base.min(range.base), end.max(range.end()))
        });
        Self::new(base, end - base)
    }

    /// Like [`collapse`](Self::collapse), but fails if the ranges leave any gap:
    ///
    /// ```text
    /// [a][b[]c]     - Ok([a     c])
    /// [a[d]] [b]    - Err(Disjoint)
    /// ```
    ///
    /// Runs in O(n²) time without allocating.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Disjoint`] if the ranges are not contiguous; [`MemoryRangeError::Empty`] if `ranges` is
    /// empty.
    pub fn coalesce<I>(ranges: I) -> Result<MemoryRange, MemoryRangeError>
    where
        I: IntoIterator,
        I::IntoIter: Clone,
        I::Item: Borrow<MemoryRange>,
    {
        let ranges = ranges.into_iter();
        let covering = Self::collapse(ranges.clone())?;
        // A gap always starts where some range ends, so the ranges are contiguous exactly when every range end below
        // the covering end falls inside another range.
        let has_gap = ranges.clone().any(|range| {
            let end = range.borrow().end();
            end < covering.end() && !ranges.clone().any(|other| other.borrow().contains(end))
        });
        if has_gap { Err(MemoryRangeError::Disjoint) } else { Ok(covering) }
    }

    /// Returns the first pair of ranges that overlap, in input order, or `None` if no two ranges share an address.
    /// Adjacent ranges do not overlap:
    ///
    /// ```text
    /// [a][b]  [c]   - None
    /// [a[]b]  [c]   - Some((a, b))
    /// [a]  [b[c]]   - Some((b, c))
    /// ```
    ///
    /// Runs in O(n²) time without allocating.
    pub fn find_overlap<I>(ranges: I) -> Option<(MemoryRange, MemoryRange)>
    where
        I: IntoIterator,
        I::IntoIter: Clone,
        I::Item: Borrow<MemoryRange>,
    {
        let ranges = ranges.into_iter();
        ranges.clone().enumerate().find_map(|(index, first)| {
            let first = *first.borrow();
            ranges
                .clone()
                .skip(index + 1)
                .map(|second| *second.borrow())
                .find(|second| first.overlaps(second))
                .map(|second| (first, second))
        })
    }

    /// Returns how much of `self` the union of `ranges` covers. Ranges that only touch each other still cover
    /// contiguously (`s` = `self`, `r` = ranges):
    ///
    /// ```text
    /// [r[s]]        - Full
    /// [r][r]        - Full when `s` spans both
    /// [r[]s]        - Partial
    /// [r] [r]       - Partial when `s` spans the gap
    /// [r] [s]       - Disjoint
    /// ```
    ///
    /// Runs in O(n²) time without allocating.
    pub fn coverage<I>(&self, ranges: I) -> Coverage
    where
        I: IntoIterator,
        I::IntoIter: Clone,
        I::Item: Borrow<MemoryRange>,
    {
        let ranges = ranges.into_iter();
        // Extend the covered prefix of `self` one range at a time until a gap appears. Each step moves `cursor`
        // forward, because the range found contains it and so ends above it.
        let mut cursor = self.base;
        while let Some(end) =
            ranges.clone().map(|range| *range.borrow()).find(|range| range.contains(cursor)).map(|r| r.end())
        {
            if end >= self.end() {
                return Coverage::Full;
            }
            cursor = end;
        }

        if cursor > self.base || ranges.clone().any(|range| self.overlaps(range.borrow())) {
            Coverage::Partial
        } else {
            Coverage::Disjoint
        }
    }

    /// Returns `true` if base and size are multiples of `align`.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::InvalidAlignment`] if `align` is not a non-zero power of two.
    pub const fn is_aligned(&self, align: u64) -> Result<bool, MemoryRangeError> {
        if !align.is_power_of_two() {
            return Err(MemoryRangeError::InvalidAlignment);
        }
        let mask = align - 1;
        Ok(self.base & mask == 0 && self.size & mask == 0)
    }

    /// Returns `true` if the size is a power of two and the base is a multiple of the size.
    ///
    /// Hardware that describes a region with a base and a mask, such as an MTRR or a large page
    /// mapping, can only describe a range like this.
    pub const fn is_naturally_aligned(&self) -> bool {
        // `size` is never zero, so `size - 1` cannot underflow.
        self.size.is_power_of_two() && self.base & (self.size - 1) == 0
    }

    /// Rounds the size up to a multiple of `align`, keeping the base fixed.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::InvalidAlignment`] if `align` is not a non-zero power of two;
    /// [`MemoryRangeError::Overflow`] if `base + size` would exceed `u64::MAX`.
    pub const fn extend_to_align(&self, align: u64) -> Result<MemoryRange, MemoryRangeError> {
        if !align.is_power_of_two() {
            return Err(MemoryRangeError::InvalidAlignment);
        }
        let mask = align - 1;
        match self.size.checked_add(mask) {
            Some(size) => MemoryRange::new(self.base, size & !mask),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Returns the range with `additional` bytes added to its size.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::Overflow`] if `base + size` would exceed `u64::MAX`.
    pub const fn extend(&self, additional: u64) -> Result<MemoryRange, MemoryRangeError> {
        match self.size.checked_add(additional) {
            Some(size) => MemoryRange::new(self.base, size),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Returns the smallest range aligned to `align` that contains `self`: the base is rounded down and the end is
    /// rounded up to a multiple of `align`.
    ///
    /// ```text
    /// align:   |       |       |       |
    /// self:        [s      ]
    /// output:  [               ]
    /// ```
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::InvalidAlignment`] if `align` is not a non-zero power of two;
    /// [`MemoryRangeError::Overflow`] if the rounded end would exceed `u64::MAX`.
    pub const fn align_outward(&self, align: u64) -> Result<MemoryRange, MemoryRangeError> {
        if !align.is_power_of_two() {
            return Err(MemoryRangeError::InvalidAlignment);
        }
        let mask = align - 1;
        let base = self.base & !mask;
        match self.end().checked_add(mask) {
            Some(end) => MemoryRange::new(base, (end & !mask) - base),
            None => Err(MemoryRangeError::Overflow),
        }
    }

    /// Returns the addresses `base`, `base + step`, `base + 2 * step`, ... that lie in the range.
    ///
    /// The first address is always `base`. Chunks of `step` bytes starting at these addresses cover the range exactly
    /// when the size is a multiple of `step`.
    pub fn step_by(&self, step: NonZeroU64) -> impl Iterator<Item = u64> + use<> {
        let (step, end) = (step.get(), self.end());
        core::iter::successors(Some(self.base), move |&address| address.checked_add(step).filter(|&next| next < end))
    }
}

impl Add for MemoryRange {
    type Output = Result<MemoryRange, MemoryRangeError>;

    /// See [`MemoryRange::union`].
    fn add(self, rhs: MemoryRange) -> Self::Output {
        self.union(&rhs)
    }
}

impl Ord for MemoryRange {
    /// Orders by base, then size. `a < b` does not mean `a` lies wholly below `b`.
    fn cmp(&self, other: &Self) -> Ordering {
        self.base.cmp(&other.base).then(self.size.cmp(&other.size))
    }
}

impl PartialOrd for MemoryRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl TryFrom<Range<u64>> for MemoryRange {
    type Error = MemoryRangeError;

    /// Converts `start..end`.
    ///
    /// # Errors
    ///
    /// [`MemoryRangeError::InvalidBounds`] if `end < start`; [`MemoryRangeError::Empty`] if `end == start`.
    fn try_from(range: Range<u64>) -> Result<Self, Self::Error> {
        if range.end < range.start {
            return Err(MemoryRangeError::InvalidBounds);
        }
        MemoryRange::new(range.start, range.end - range.start)
    }
}

impl TryFrom<RangeInclusive<u64>> for MemoryRange {
    type Error = MemoryRangeError;

    /// See [`MemoryRange::from_inclusive`].
    fn try_from(range: RangeInclusive<u64>) -> Result<Self, Self::Error> {
        MemoryRange::from_inclusive(*range.start(), *range.end())
    }
}

impl fmt::Display for MemoryRange {
    /// Formats as `0xBASE..=0xLAST`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}..={:#x}", self.base, self.last())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    const MAX: u64 = u64::MAX;

    fn mr(base: u64, size: u64) -> MemoryRange {
        MemoryRange::new(base, size).unwrap()
    }

    #[test]
    fn test_memory_range_new_accepts_valid_ranges() {
        let range = mr(0x1000, 0x2000);
        assert_eq!(range.base(), 0x1000);
        assert_eq!(range.size(), 0x2000);
        assert_eq!(range.end(), 0x3000);
        assert_eq!(range.last(), 0x2fff);

        let single = mr(0, 1);
        assert_eq!(single.last(), 0);

        assert_eq!(range.into_parts(), (0x1000, 0x2000));
    }

    #[test]
    fn test_memory_range_new_rejects_empty() {
        assert_eq!(MemoryRange::new(0x1000, 0), Err(MemoryRangeError::Empty));
        assert_eq!(MemoryRange::new(MAX, 0), Err(MemoryRangeError::Empty));
    }

    #[test]
    fn test_memory_range_new_allows_end_at_u64_max() {
        let top = mr(MAX - 0x1000, 0x1000);
        assert_eq!(top.end(), MAX);
        assert_eq!(top.last(), MAX - 1);
        assert!(top.contains(MAX - 1));
        assert!(!top.contains(MAX));
        assert_eq!(mr(0, MAX).end(), MAX);
    }

    #[test]
    fn test_memory_range_new_rejects_overflow() {
        assert_eq!(MemoryRange::new(MAX, 1), Err(MemoryRangeError::Overflow));
        assert_eq!(MemoryRange::new(1, MAX), Err(MemoryRangeError::Overflow));
        assert_eq!(MemoryRange::new(MAX - 0xfff, 0x1000), Err(MemoryRangeError::Overflow));
    }

    #[test]
    fn test_memory_range_from_inclusive() {
        assert_eq!(MemoryRange::from_inclusive(0x1000, 0x1fff), Ok(mr(0x1000, 0x1000)));
        assert_eq!(MemoryRange::from_inclusive(5, 5), Ok(mr(5, 1)));
        assert_eq!(MemoryRange::from_inclusive(0, MAX - 1), Ok(mr(0, MAX)));
        assert_eq!(MemoryRange::from_inclusive(2, 1), Err(MemoryRangeError::InvalidBounds));
        assert_eq!(MemoryRange::from_inclusive(0, MAX), Err(MemoryRangeError::Overflow));
        assert_eq!(MemoryRange::from_inclusive(MAX, MAX), Err(MemoryRangeError::Overflow));
    }

    #[test]
    fn test_memory_range_from_pages() {
        assert_eq!(MemoryRange::from_pages(0x1000, 3, 0x1000), Ok(mr(0x1000, 0x3000)));
        assert_eq!(MemoryRange::from_pages(0x1234, 1, 0x200), Ok(mr(0x1234, 0x200)));
        assert_eq!(MemoryRange::from_pages(MAX - 0x2000, 2, 0x1000), Ok(mr(MAX - 0x2000, 0x2000)));
        assert_eq!(MemoryRange::from_pages(0x1000, 0, 0x1000), Err(MemoryRangeError::Empty));
        assert_eq!(MemoryRange::from_pages(0x1000, 4, 0), Err(MemoryRangeError::Empty));
        assert_eq!(MemoryRange::from_pages(0, MAX, 0x1000), Err(MemoryRangeError::Overflow));
        assert_eq!(MemoryRange::from_pages(MAX - 0xfff, 1, 0x1000), Err(MemoryRangeError::Overflow));
    }

    #[test]
    fn test_memory_range_try_from_std_ranges() {
        assert_eq!(MemoryRange::try_from(0x1000..0x3000), Ok(mr(0x1000, 0x2000)));
        assert_eq!(MemoryRange::try_from(0..MAX), Ok(mr(0, MAX)));
        assert_eq!(MemoryRange::try_from(7..7), Err(MemoryRangeError::Empty));
        #[allow(clippy::reversed_empty_ranges)]
        let reversed = 3..2;
        assert_eq!(MemoryRange::try_from(reversed), Err(MemoryRangeError::InvalidBounds));

        assert_eq!(MemoryRange::try_from(0x1000..=0x1fff), Ok(mr(0x1000, 0x1000)));
        assert_eq!(MemoryRange::try_from(0..=MAX), Err(MemoryRangeError::Overflow));
    }

    #[test]
    fn test_memory_range_contains() {
        let range = mr(0x1000, 0x1000);
        assert!(!range.contains(0xfff));
        assert!(range.contains(0x1000));
        assert!(range.contains(0x1fff));
        assert!(!range.contains(0x2000));
    }

    #[test]
    fn test_memory_range_contains_range() {
        let outer = mr(0x1000, 0x3000);
        assert!(outer.contains_range(&mr(0x1000, 0x3000)));
        assert!(outer.contains_range(&mr(0x2000, 0x1000)));
        assert!(!outer.contains_range(&mr(0x3000, 0x2000)));
        assert!(!outer.contains_range(&mr(0x0, 0x2000)));
        assert!(!outer.contains_range(&mr(0x0, 0x8000)));
        assert!(!outer.contains_range(&mr(0x8000, 0x10)));

        let top = mr(MAX - 0x1000, 0x1000);
        assert!(top.contains_range(&mr(MAX - 1, 1)));
    }

    #[test]
    fn test_memory_range_overlaps() {
        let a = mr(0x1000, 0x1000);
        assert!(a.overlaps(&mr(0x1fff, 0x10)));
        assert!(a.overlaps(&mr(0x0, 0x1001)));
        assert!(a.overlaps(&mr(0x1800, 0x10)));
        assert!(a.overlaps(&mr(0x0, 0x8000)));
        assert!(!a.overlaps(&mr(0x2000, 0x1000)));
        assert!(!a.overlaps(&mr(0x0, 0x1000)));
        assert!(!a.overlaps(&mr(0x8000, 0x10)));
    }

    #[test]
    fn test_memory_range_intersection() {
        let a = mr(0x1000, 0x2000);
        let b = mr(0x2000, 0x2000);
        assert_eq!(a.intersection(&b), Ok(mr(0x2000, 0x1000)));
        assert_eq!(b.intersection(&a), a.intersection(&b));
        assert_eq!(a.intersection(&mr(0x3000, 0x10)), Err(MemoryRangeError::Empty));
        assert_eq!(a.intersection(&mr(0x5000, 0x10)), Err(MemoryRangeError::Empty));
        assert_eq!(mr(0x5000, 0x10).intersection(&a), Err(MemoryRangeError::Empty));
        assert_eq!(a.intersection(&mr(0x1800, 0x10)), Ok(mr(0x1800, 0x10)));

        let top = mr(MAX - 0x1000, 0x1000);
        let wide = mr(MAX - 0x2000, 0x1800);
        assert_eq!(top.intersection(&wide), Ok(mr(MAX - 0x1000, 0x800)));
        assert_eq!(top.intersection(&top), Ok(top));
    }

    #[test]
    fn test_memory_range_is_adjacent() {
        let a = mr(0x1000, 0x1000);
        assert!(a.is_adjacent(&mr(0x2000, 0x10)));
        assert!(a.is_adjacent(&mr(0x0, 0x1000)));
        assert!(!a.is_adjacent(&mr(0x2001, 0x10)));
        assert!(!a.is_adjacent(&mr(0x1800, 0x1000)));

        let top = mr(MAX - 0x1000, 0x1000);
        assert!(top.is_adjacent(&mr(MAX - 0x2000, 0x1000)));
    }

    #[test]
    fn test_memory_range_union_overlapping() {
        let a = mr(0x1000, 0x2000);
        let b = mr(0x2000, 0x2000);
        assert_eq!(a.union(&b), Ok(mr(0x1000, 0x3000)));
        assert_eq!(b.union(&a), Ok(mr(0x1000, 0x3000)));

        let inner = mr(0x1800, 0x100);
        assert_eq!(a.union(&inner), Ok(a));
        assert_eq!(inner.union(&a), Ok(a));
        assert_eq!(a.union(&a), Ok(a));
    }

    #[test]
    fn test_memory_range_union_adjacent() {
        let a = mr(0x1000, 0x1000);
        let b = mr(0x2000, 0x1000);
        assert_eq!(a.union(&b), Ok(mr(0x1000, 0x2000)));
        assert_eq!(b.union(&a), Ok(mr(0x1000, 0x2000)));

        let top = mr(MAX - 0x1000, 0x1000);
        let below = mr(MAX - 0x2000, 0x1000);
        assert_eq!(top.union(&below), Ok(mr(MAX - 0x2000, 0x2000)));
    }

    #[test]
    fn test_memory_range_union_disjoint() {
        let a = mr(0x1000, 0x1000);
        let b = mr(0x2001, 0x1000);
        assert_eq!(a.union(&b), Err(MemoryRangeError::Disjoint));
        assert_eq!(b.union(&a), Err(MemoryRangeError::Disjoint));
    }

    #[test]
    fn test_memory_range_union_spans_address_space() {
        assert_eq!(mr(0, 0x1000).union(&mr(0x1000, MAX - 0x1000)), Ok(mr(0, MAX)));
    }

    #[test]
    fn test_memory_range_add_operator() {
        let a = mr(0x1000, 0x1000);
        let b = mr(0x2000, 0x1000);
        let c = mr(0x2800, 0x1000);
        assert_eq!(a + b, Ok(mr(0x1000, 0x2000)));
        assert_eq!((a + b).and_then(|ab| ab + c), Ok(mr(0x1000, 0x2800)));
        assert_eq!(a + c, Err(MemoryRangeError::Disjoint));
    }

    #[test]
    fn test_memory_range_collapse_empty_and_single() {
        assert_eq!(MemoryRange::collapse(core::iter::empty::<MemoryRange>()), Err(MemoryRangeError::Empty));
        assert_eq!(MemoryRange::collapse([mr(0x1000, 0x10)]), Ok(mr(0x1000, 0x10)));
    }

    #[test]
    fn test_memory_range_collapse_covers_gaps() {
        let input = [mr(0x5000, 0x10), mr(0x1000, 0x10), mr(0x3000, 0x10)];
        assert_eq!(MemoryRange::collapse(input), Ok(mr(0x1000, 0x4010)));
    }

    #[test]
    fn test_memory_range_collapse_overlapping_adjacent_and_contained() {
        let input = [
            mr(0x6000, 0x1000), // [c]
            mr(0x1000, 0x1000), // [a]
            mr(0x3000, 0x1000), // [b
            mr(0x3800, 0x1000), //   []d] overlaps b
            mr(0x2000, 0x1000), // adjacent to a
            mr(0x6100, 0x10),   // contained in c
        ];
        assert_eq!(MemoryRange::collapse(input), Ok(mr(0x1000, 0x6000)));
    }

    #[test]
    fn test_memory_range_collapse_highest_end_is_not_highest_base() {
        let input = [mr(0x1000, 0x8000), mr(0x2000, 0x10)];
        assert_eq!(MemoryRange::collapse(input), Ok(mr(0x1000, 0x8000)));
    }

    #[test]
    fn test_memory_range_collapse_at_top_of_address_space() {
        let input = [mr(MAX - 0x1000, 0x1000), mr(0, 0x1000)];
        assert_eq!(MemoryRange::collapse(input), Ok(mr(0, MAX)));
    }

    #[test]
    fn test_memory_range_coalesce_empty_and_single() {
        assert_eq!(MemoryRange::coalesce(core::iter::empty::<MemoryRange>()), Err(MemoryRangeError::Empty));
        assert_eq!(MemoryRange::coalesce([mr(0x1000, 0x10)]), Ok(mr(0x1000, 0x10)));
    }

    #[test]
    fn test_memory_range_collapse_and_coalesce_accept_owned_iterators() {
        let ranges = [(0x2000, 0x1000), (0x1000, 0x1000)];
        let owned = || ranges.iter().map(|&(base, size)| mr(base, size));
        assert_eq!(MemoryRange::collapse(owned()), Ok(mr(0x1000, 0x2000)));
        assert_eq!(MemoryRange::coalesce(owned()), Ok(mr(0x1000, 0x2000)));
    }

    #[test]
    fn test_memory_range_find_overlap() {
        assert_eq!(MemoryRange::find_overlap(core::iter::empty::<MemoryRange>()), None);
        assert_eq!(MemoryRange::find_overlap([mr(0x1000, 0x10)]), None);

        // Adjacent and separate ranges do not overlap.
        assert_eq!(MemoryRange::find_overlap([mr(0x1000, 0x1000), mr(0x2000, 0x1000), mr(0x5000, 0x10)]), None);

        // The pair is reported in input order, even when the second range has the lower base.
        let (a, b) = (mr(0x2000, 0x1000), mr(0x1800, 0x1000));
        assert_eq!(MemoryRange::find_overlap([a, b]), Some((a, b)));

        // A range nested in an earlier, larger range is found even when other ranges sit between them.
        let (outer, gap, inner) = (mr(0x1000, 0x8000), mr(0x10000, 0x10), mr(0x4000, 0x10));
        assert_eq!(MemoryRange::find_overlap([outer, gap, inner]), Some((outer, inner)));

        // The first overlapping pair wins.
        let (c, d) = (mr(0x7000, 0x100), mr(0x70f0, 0x100));
        assert_eq!(MemoryRange::find_overlap([mr(0x100, 0x10), c, d, a, b]), Some((c, d)));

        // Identical ranges overlap.
        assert_eq!(MemoryRange::find_overlap([a, a]), Some((a, a)));
    }

    #[test]
    fn test_memory_range_coverage() {
        let ranges = [mr(0x1000, 0x1000), mr(0x2000, 0x1000), mr(0x5000, 0x1000)];

        assert_eq!(mr(0x1000, 0x1000).coverage(ranges), Coverage::Full);
        // Adjacent ranges cover the range jointly.
        assert_eq!(mr(0x1800, 0x1000).coverage(ranges), Coverage::Full);
        assert_eq!(mr(0x1000, 0x2000).coverage(ranges), Coverage::Full);
        // The query runs past the covered prefix.
        assert_eq!(mr(0x2800, 0x1000).coverage(ranges), Coverage::Partial);
        // The query starts before any range.
        assert_eq!(mr(0x0800, 0x1000).coverage(ranges), Coverage::Partial);
        // The query spans a gap between two covered parts.
        assert_eq!(mr(0x2800, 0x3000).coverage(ranges), Coverage::Partial);
        assert_eq!(mr(0x3000, 0x1000).coverage(ranges), Coverage::Disjoint);
        assert_eq!(mr(0x3000, 0x2000).coverage(ranges), Coverage::Disjoint);
        assert_eq!(mr(0x1000, 0x10).coverage(core::iter::empty::<MemoryRange>()), Coverage::Disjoint);

        // The order of the ranges does not matter.
        let reversed = [ranges[2], ranges[1], ranges[0]];
        assert_eq!(mr(0x1000, 0x2000).coverage(reversed), Coverage::Full);

        // A range ending at the top of the address space can cover a query that does too.
        let top = mr(MAX - 0x1000, 0x1000);
        assert_eq!(mr(MAX - 0x10, 0x10).coverage([top]), Coverage::Full);
    }

    #[test]
    fn test_memory_range_align_outward() {
        assert_eq!(mr(0x1234, 0x1000).align_outward(0x1000), Ok(mr(0x1000, 0x2000)));
        assert_eq!(mr(0x1000, 0x1000).align_outward(0x1000), Ok(mr(0x1000, 0x1000)));
        assert_eq!(mr(0x1001, 1).align_outward(0x1000), Ok(mr(0x1000, 0x1000)));
        // Matches the documented `align_range` example.
        assert_eq!(mr(1023, 2048).align_outward(512), Ok(mr(512, 2560)));

        // An unaligned range near the top of the address space rounds to the last whole pages below it.
        let top = mr(MAX - 0x1fef, 0xff0);
        assert_eq!(top.align_outward(0x1000), Ok(mr(MAX - 0x1fff, 0x1000)));
        // Rounding an end at the top of the address space up would pass `u64::MAX`.
        assert_eq!(mr(MAX - 0x10, 0x10).align_outward(0x1000), Err(MemoryRangeError::Overflow));

        assert_eq!(mr(0x1000, 0x10).align_outward(0), Err(MemoryRangeError::InvalidAlignment));
        assert_eq!(mr(0x1000, 0x10).align_outward(0x1800), Err(MemoryRangeError::InvalidAlignment));
    }

    #[test]
    fn test_memory_range_step_by() {
        let step = NonZeroU64::new(0x1000).unwrap();
        assert_eq!(mr(0x1000, 0x3000).step_by(step).collect::<Vec<_>>(), vec![0x1000, 0x2000, 0x3000]);
        // A size that is not a multiple of the step still yields the base and every later step inside the range.
        assert_eq!(mr(0x1000, 0x1001).step_by(step).collect::<Vec<_>>(), vec![0x1000, 0x2000]);
        assert_eq!(mr(0x1000, 1).step_by(step).collect::<Vec<_>>(), vec![0x1000]);

        // The last step below the top of the address space does not wrap.
        let top = mr(MAX - 0x1fff, 0x1fff);
        assert_eq!(top.step_by(step).collect::<Vec<_>>(), vec![MAX - 0x1fff, MAX - 0xfff]);
        assert_eq!(mr(0, MAX).step_by(NonZeroU64::new(1 << 63).unwrap()).collect::<Vec<_>>(), vec![0, 1 << 63]);
    }

    #[test]
    fn test_memory_range_coalesce_contiguous() {
        let input = [
            mr(0x3000, 0x1000), // adjacent to the second range
            mr(0x1000, 0x2000),
            mr(0x3800, 0x1000), // overlaps the first range
            mr(0x1800, 0x100),  // contained in the second range
        ];
        assert_eq!(MemoryRange::coalesce(input), Ok(mr(0x1000, 0x3800)));
    }

    #[test]
    fn test_memory_range_coalesce_rejects_gaps() {
        assert_eq!(MemoryRange::coalesce([mr(0x1000, 0x1000), mr(0x2001, 0x10)]), Err(MemoryRangeError::Disjoint));
        assert_eq!(MemoryRange::coalesce([mr(0x3000, 0x10), mr(0x1000, 0x1000)]), Err(MemoryRangeError::Disjoint));

        // The range with the highest end is not the one followed by the gap.
        let input = [mr(0x1000, 0x8000), mr(0x1100, 0x10), mr(0xa000, 0x10)];
        assert_eq!(MemoryRange::coalesce(input), Err(MemoryRangeError::Disjoint));
    }

    #[test]
    fn test_memory_range_coalesce_at_top_of_address_space() {
        let input = [mr(MAX - 0x1000, 0x1000), mr(0, MAX - 0x1000)];
        assert_eq!(MemoryRange::coalesce(input), Ok(mr(0, MAX)));
    }

    #[test]
    fn test_memory_range_is_aligned() {
        let range = mr(0x2000, 0x1000);
        assert_eq!(range.is_aligned(0x1000), Ok(true));
        assert_eq!(range.is_aligned(0x2000), Ok(false));
        assert_eq!(mr(0x1000, 0x10).is_aligned(0x1000), Ok(false));
        assert_eq!(range.is_aligned(0), Err(MemoryRangeError::InvalidAlignment));
        assert_eq!(range.is_aligned(0x1800), Err(MemoryRangeError::InvalidAlignment));
    }

    #[test]
    fn test_memory_range_is_naturally_aligned() {
        assert!(mr(0, 1).is_naturally_aligned());
        assert!(mr(0x8000, 0x1000).is_naturally_aligned());
        assert!(mr(0x80_0000, 0x80_0000).is_naturally_aligned());
        assert!(mr(1 << 62, 1 << 62).is_naturally_aligned());
        // The size is not a power of two.
        assert!(!mr(0, 0x3000).is_naturally_aligned());
        // The base is not a multiple of the size.
        assert!(!mr(0x1000, 0x2000).is_naturally_aligned());
        assert!(!mr(0x80_1000, 0x80_0000).is_naturally_aligned());
    }

    #[test]
    fn test_memory_range_extend_to_align() {
        let range = mr(0x1001, 0x1001);
        assert_eq!(range.extend_to_align(0x1000), Ok(mr(0x1001, 0x2000)));

        let aligned = mr(0x1000, 0x1000);
        assert_eq!(aligned.extend_to_align(0x1000), Ok(aligned));
        assert_eq!(mr(0x1000, 1).extend_to_align(0x1000), Ok(mr(0x1000, 0x1000)));

        let top = mr(MAX - 0x1000, 0x11);
        assert_eq!(top.extend_to_align(0x1000), Ok(mr(MAX - 0x1000, 0x1000)));
        assert_eq!(mr(MAX - 0x10, 0x10).extend_to_align(0x1000), Err(MemoryRangeError::Overflow));
        assert_eq!(mr(0, MAX).extend_to_align(0x1000), Err(MemoryRangeError::Overflow));

        assert_eq!(range.extend_to_align(3), Err(MemoryRangeError::InvalidAlignment));
    }

    #[test]
    fn test_memory_range_extend() {
        let range = mr(0x1000, 0x1000);
        assert_eq!(range.extend(0x2000), Ok(mr(0x1000, 0x3000)));
        assert_eq!(range.extend(0), Ok(range));

        let top = mr(MAX - 0x1000, 0x800);
        assert_eq!(top.extend(0x800), Ok(mr(MAX - 0x1000, 0x1000)));
        assert_eq!(top.extend(0x801), Err(MemoryRangeError::Overflow));
        assert_eq!(mr(0, MAX).extend(1), Err(MemoryRangeError::Overflow));
    }

    #[test]
    fn test_memory_range_ordering() {
        let a = mr(0x1000, 0x2000);
        let b = mr(0x1000, 0x3000);
        let c = mr(0x2000, 0x10);
        assert!(a < b);
        assert!(b < c);
        assert!(a < c);
        assert!(c > a);
        let same = mr(0x1000, 0x2000);
        assert!(a <= same);
        assert!(a >= same);
        assert_eq!(a.cmp(&same), Ordering::Equal);
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(c.cmp(&b), Ordering::Greater);
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
        assert_eq!(a.max(c), c);
        assert_eq!(b.min(c), b);
    }

    #[test]
    fn test_memory_range_ordering_sorts_by_base_then_size() {
        let small = mr(0x1000, 0x10);
        let large = mr(0x1000, 0x1000);
        let low = mr(0x0, MAX);
        let top = mr(MAX - 1, 1);

        let mut ranges = vec![top, large, low, small];
        ranges.sort();
        assert_eq!(ranges, vec![low, small, large, top]);
    }

    #[test]
    fn test_memory_range_display() {
        assert_eq!(format!("{}", mr(0x1000, 0x1000)), "0x1000..=0x1fff");
        assert_eq!(format!("{}", mr(0x1000, 1)), "0x1000..=0x1000");
    }

    #[test]
    fn test_memory_range_error_display_and_conversion() {
        assert_eq!(format!("{}", MemoryRangeError::Overflow), "memory range extends past the end of the address space");
        assert_eq!(format!("{}", MemoryRangeError::InvalidBounds), "memory range end is below its start");
        assert_eq!(format!("{}", MemoryRangeError::InvalidAlignment), "alignment must be a non-zero power of two");
        assert_eq!(format!("{}", MemoryRangeError::Disjoint), "memory ranges are neither overlapping nor adjacent");
        assert_eq!(format!("{}", MemoryRangeError::Empty), "memory range is empty");
        assert_eq!(EfiError::from(MemoryRangeError::Overflow), EfiError::InvalidParameter);
    }
}
