//! Serializable UEFI/PI Structs
//!
//! Contains custom definitions for serializing HOBs to and from JSON format, using `serde`.
//! This is not required by the UEFI/PI specifications, but is provided for convenience in visualizing and encoding HOBs.
//! This crate is gated behind the `serde` feature flag. Serialization is only available when the feature is enabled.
//!
//! For information on the standard HOB format, see `hob.rs`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Helper functions for serializing data as hex strings.
pub mod hex_format;
/// Serializable HOB definitions.
pub mod serializable_hob;

use r_efi::efi::Guid;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Format a GUID as a string in the standard 8-4-4-4-12 format.
/// This custom implementation is necessary because `r_efi::Guid` has private fields and cannot derive `Serialize` directly.
///
pub fn format_guid(guid: Guid) -> String {
    let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = guid.as_fields();
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        time_low,
        time_mid,
        time_hi_and_version,
        clk_seq_hi_res,
        clk_seq_low,
        node[0],
        node[1],
        node[2],
        node[3],
        node[4],
        node[5]
    )
}

/// Represents an positive integral interval [start, end).
/// In practice, this is often used to represent memory ranges in HOBs.
///
pub trait Interval: Clone + Ord {
    /// Start of the interval (inclusive).
    fn start(&self) -> u64;

    /// End of the interval (exclusive).
    fn end(&self) -> u64;

    /// Merge two overlapping or adjacent intervals.
    fn merge(&self, other: &Self) -> Self;

    /// Length of the interval.
    fn length(&self) -> u64 {
        self.end() - self.start()
    }

    /// Check if this interval fully contains another one.
    fn contains(&self, other: &Self) -> bool {
        self.start() <= other.start() && self.end() >= other.end()
    }

    /// Check if this interval overlaps with another one.
    /// ```ignore
    /// - [s] [o] - non overlapping
    /// - [s[]o] - overlapping
    /// - [s[o]] - overlapping
    /// - [o[s]] - overlapping
    /// - [o[]s] - overlapping
    /// - [o] [s] - non overlapping
    /// ```
    fn overlaps(&self, other: &Self) -> bool {
        self.start() < other.end() && other.start() < self.end()
    }

    /// Check if this interval is adjacent to another one.
    /// Adjacency means:
    /// ```ignore
    /// - [s][o] or [o][s] (end of one is exactly the start of the other)
    /// ```
    fn adjacent(&self, other: &Self) -> bool {
        self.end() == other.start() || other.end() == self.start()
    }

    /// Try to merge two intervals if they are overlapping or adjacent.
    fn try_merge(&self, other: &Self) -> Option<Self> {
        if self.overlaps(other) || self.adjacent(other) { Some(self.merge(other)) } else { None }
    }

    /// Merge a list of intervals into a minimal set of non-overlapping intervals.
    fn merge_intervals(intervals: &[&Self]) -> Vec<Self> {
        if intervals.is_empty() {
            return Vec::new();
        }

        let mut sorted = intervals.to_vec();
        sorted.sort();

        let mut result = vec![sorted[0].clone()];
        for current in sorted.into_iter().skip(1) {
            let last = result.last_mut().unwrap();
            if let Some(merged) = last.try_merge(current) {
                *last = merged;
            } else {
                result.push((*current).clone());
            }
        }

        result
    }
}
