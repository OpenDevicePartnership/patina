//! Text-mode selection and GOP resolution selection for the graphics console.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::vec::Vec;

use patina::error::{EfiError, Result};

use super::gop::{GLYPH_HEIGHT, GLYPH_WIDTH, GopHandle};

/// Column/row and backing GOP-mode metadata for one text mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextMode {
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    /// Pixel offset of the text area's top-left corner, centering it within the GOP mode.
    pub(crate) delta_x: usize,
    pub(crate) delta_y: usize,
    pub(crate) gop_width: u32,
    pub(crate) gop_height: u32,
    pub(crate) gop_mode_number: u32,
}

/// Candidate text-mode column/row pairs considered in addition to the mandatory 80x25 and 80x50
/// modes.
///
/// - (100, 31): 800x600
/// - (128, 40): 1024x768
/// - (160, 42): 1280x800
/// - (240, 56): 1920x1080
const CANDIDATE_MODES: &[(usize, usize)] = &[(100, 31), (128, 40), (160, 42), (240, 56)];

/// Builds the list of text modes available at `horizontal_resolution` x `vertical_resolution`,
/// which is set on GOP mode `gop_mode_number`.
///
/// Mode 0 is always 80x25 and mode 1 is 80x50 (when the resolution supports it), matching the
/// minimum text modes the UEFI specification requires. Additional candidate modes, and a final
/// full-screen mode sized to the resolution, are appended when they are not already covered by an
/// earlier entry.
pub(crate) fn build_text_modes(
    horizontal_resolution: u32,
    vertical_resolution: u32,
    gop_mode_number: u32,
) -> Vec<TextMode> {
    let max_columns = (horizontal_resolution as usize) / GLYPH_WIDTH;
    let max_rows = (vertical_resolution as usize) / GLYPH_HEIGHT;

    let make_mode = |columns: usize, rows: usize| TextMode {
        columns,
        rows,
        delta_x: (horizontal_resolution as usize).saturating_sub(columns * GLYPH_WIDTH) / 2,
        delta_y: (vertical_resolution as usize).saturating_sub(rows * GLYPH_HEIGHT) / 2,
        gop_width: horizontal_resolution,
        gop_height: vertical_resolution,
        gop_mode_number,
    };

    let mut modes = alloc::vec![make_mode(80, 25)];
    if max_columns >= 80 && max_rows >= 50 {
        modes.push(make_mode(80, 50));
    }

    for &(columns, rows) in CANDIDATE_MODES {
        if columns == 0 || rows == 0 || columns > max_columns || rows > max_rows {
            continue;
        }
        if modes.iter().any(|mode| mode.columns == columns && mode.rows == rows) {
            continue;
        }
        modes.push(make_mode(columns, rows));
    }

    if !modes.iter().any(|mode| mode.columns == max_columns && mode.rows == max_rows) {
        modes.push(make_mode(max_columns, max_rows));
    }

    modes
}

/// Picks the preferred mode index, which is the entry matching `preferred`, or otherwise a running-max
/// search for the largest columns/rows seen while scanning in order.
pub(crate) fn pick_preferred_mode(modes: &[TextMode], preferred: Option<(u32, u32)>) -> usize {
    if let Some((columns, rows)) = preferred
        && let Some(index) = modes.iter().position(|mode| mode.columns as u32 == columns && mode.rows as u32 == rows)
    {
        return index;
    }

    let mut best_index = 0usize;
    let mut best_columns = 0usize;
    let mut best_rows = 0usize;
    for (index, mode) in modes.iter().enumerate() {
        if mode.columns >= best_columns && mode.rows >= best_rows {
            best_columns = mode.columns;
            best_rows = mode.rows;
            best_index = index;
        }
    }
    best_index
}

/// Selects (and if necessary sets) the GOP mode matching `resolution`, or the highest-resolution
/// mode available if `resolution` is `None`. Returns the active mode's number, horizontal
/// resolution, and vertical resolution.
///
/// If a resolution is requested that GOP does not support, resolution falls back to 800x600, and
/// finally to whatever mode is already active.
pub(crate) fn select_gop_mode(gop: &mut GopHandle, resolution: Option<(u32, u32)>) -> Result<(u32, u32, u32)> {
    let (mode_number, horizontal, vertical) = match resolution {
        None => highest_resolution_mode(gop)?,
        Some((horizontal, vertical)) => resolution_or_fallback(gop, horizontal, vertical)?,
    };

    if gop.current_mode_number() != mode_number {
        gop.set_mode(mode_number)?;
    }

    Ok((mode_number, horizontal, vertical))
}

/// Finds a mode matching `horizontal` x `vertical`, falling back to 800x600 and finally to
/// whatever mode is already active.
fn resolution_or_fallback(gop: &mut GopHandle, horizontal: u32, vertical: u32) -> Result<(u32, u32, u32)> {
    if let Some(found) = find_supported_mode(gop, horizontal, vertical)? {
        return Ok(found);
    }
    if let Some(found) = find_supported_mode(gop, 800, 600)? {
        return Ok(found);
    }
    let current = gop.current_resolution()?;
    Ok((gop.current_mode_number(), current.horizontal, current.vertical))
}

/// Scans every mode GOP supports for the highest resolution, preferring width, then height.
fn highest_resolution_mode(gop: &GopHandle) -> Result<(u32, u32, u32)> {
    let mut best = (0u32, 0u32, 0u32); // (mode_number, horizontal, vertical)
    for mode_number in 0..gop.max_mode() {
        let Ok(info) = gop.query_mode(mode_number) else { continue };
        if info.horizontal > best.1 || (info.horizontal == best.1 && info.vertical > best.2) {
            best = (mode_number, info.horizontal, info.vertical);
        }
    }
    if best.1 == 0 || best.2 == 0 {
        return Err(EfiError::Unsupported);
    }
    Ok(best)
}

/// Returns the mode number matching `horizontal` x `vertical`, if GOP supports it.
///
/// A mode already active at that resolution is reported without calling `set_mode` again.
/// Otherwise the first matching mode is confirmed by actually setting it, since two modes can
/// report the same resolution but only one may currently be settable.
fn find_supported_mode(gop: &mut GopHandle, horizontal: u32, vertical: u32) -> Result<Option<(u32, u32, u32)>> {
    for mode_number in 0..gop.max_mode() {
        let Ok(info) = gop.query_mode(mode_number) else { continue };
        if info.horizontal != horizontal || info.vertical != vertical {
            continue;
        }
        if gop.current_mode_number() == mode_number || gop.set_mode(mode_number).is_ok() {
            return Ok(Some((mode_number, horizontal, vertical)));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_build_text_modes_800x600_has_mandatory_modes() {
        let modes = build_text_modes(800, 600, 0);
        // 80 columns * 8px = 640; (800-640)/2 = 80. 25 rows * 19px = 475; (600-475)/2 = 62.
        assert_eq!(
            modes[0],
            TextMode {
                columns: 80,
                rows: 25,
                delta_x: 80,
                delta_y: 62,
                gop_width: 800,
                gop_height: 600,
                gop_mode_number: 0
            }
        );
        // 800x600 / (8x19) = 100x31, so 80x50 does not fit (31 rows available < 50 required).
        assert!(!modes.iter().any(|m| m.columns == 80 && m.rows == 50));
    }

    #[test]
    fn test_build_text_modes_1920x1080_includes_80x50_and_candidates() {
        let modes = build_text_modes(1920, 1080, 3);
        assert!(modes.iter().any(|m| m.columns == 80 && m.rows == 50));
        assert!(modes.iter().any(|m| m.columns == 240 && m.rows == 56));
        // No duplicate (columns, rows) pairs.
        for (i, a) in modes.iter().enumerate() {
            for b in &modes[i + 1..] {
                assert!(a.columns != b.columns || a.rows != b.rows, "duplicate mode {a:?} / {b:?}");
            }
        }
    }

    #[test]
    fn test_pick_preferred_mode_matches_exact_preference() {
        let modes = build_text_modes(1920, 1080, 0);
        let index = pick_preferred_mode(&modes, Some((80, 25)));
        assert_eq!(modes[index].columns, 80);
        assert_eq!(modes[index].rows, 25);
    }

    #[test]
    fn test_pick_preferred_mode_falls_back_to_largest() {
        let modes = build_text_modes(1920, 1080, 0);
        let index = pick_preferred_mode(&modes, None);
        // The full-screen mode is always appended last and is the largest by construction.
        assert_eq!(modes[index].columns, modes.last().unwrap().columns);
        assert_eq!(modes[index].rows, modes.last().unwrap().rows);
    }

    #[test]
    fn test_pick_preferred_mode_unmatched_preference_falls_back() {
        let modes = build_text_modes(800, 600, 0);
        let index = pick_preferred_mode(&modes, Some((999, 999)));
        assert_eq!(index, pick_preferred_mode(&modes, None));
    }

    #[test]
    fn test_select_gop_mode_none_picks_highest_resolution() {
        let fake = crate::test_support::FakeGop::new(&[(640, 480), (1920, 1080), (800, 600)]);
        let mut gop = fake.gop_handle();

        let (mode_number, horizontal, vertical) = select_gop_mode(&mut gop, None).unwrap();

        assert_eq!((mode_number, horizontal, vertical), (1, 1920, 1080));
        assert_eq!(gop.current_mode_number(), 1);
    }

    #[test]
    fn test_select_gop_mode_exact_match_avoids_redundant_set_mode() {
        let fake = crate::test_support::FakeGop::new(&[(800, 600)]);
        let mut gop = fake.gop_handle();

        let result = select_gop_mode(&mut gop, Some((800, 600))).unwrap();

        assert_eq!(result, (0, 800, 600));
        assert_eq!(fake.set_mode_calls(), 0);
    }

    #[test]
    fn test_select_gop_mode_falls_back_to_800x600() {
        let fake = crate::test_support::FakeGop::new(&[(640, 480), (800, 600)]);
        let mut gop = fake.gop_handle();

        let result = select_gop_mode(&mut gop, Some((1234, 5678))).unwrap();

        assert_eq!(result, (1, 800, 600));
        assert_eq!(gop.current_mode_number(), 1);
    }

    #[test]
    fn test_select_gop_mode_falls_back_to_current_when_nothing_matches() {
        let fake = crate::test_support::FakeGop::new(&[(640, 480)]);
        let mut gop = fake.gop_handle();

        let result = select_gop_mode(&mut gop, Some((1234, 5678))).unwrap();

        assert_eq!(result, (0, 640, 480));
        assert_eq!(fake.set_mode_calls(), 0);
    }

    #[test]
    fn test_highest_resolution_mode_errors_when_no_modes_available() {
        let fake = crate::test_support::FakeGop::new(&[]);
        let gop = fake.gop_handle();

        assert_eq!(highest_resolution_mode(&gop).unwrap_err(), EfiError::Unsupported);
    }

    #[test]
    fn test_find_supported_mode_returns_none_when_no_match() {
        let fake = crate::test_support::FakeGop::new(&[(800, 600)]);
        let mut gop = fake.gop_handle();

        assert_eq!(find_supported_mode(&mut gop, 1024, 768).unwrap(), None);
    }

    #[test]
    fn test_find_supported_mode_reuses_active_mode_without_setting() {
        let fake = crate::test_support::FakeGop::new(&[(800, 600)]);
        let mut gop = fake.gop_handle();

        let found = find_supported_mode(&mut gop, 800, 600).unwrap();

        assert_eq!(found, Some((0, 800, 600)));
        assert_eq!(fake.set_mode_calls(), 0);
    }

    #[test]
    fn test_find_supported_mode_skips_unsettable_mode_and_finds_next() {
        let fake = crate::test_support::FakeGop::with_unsettable_modes(&[(640, 480), (800, 600), (800, 600)], &[1]);
        let mut gop = fake.gop_handle();

        let found = find_supported_mode(&mut gop, 800, 600).unwrap();

        assert_eq!(found, Some((2, 800, 600)));
        // One failed attempt on mode 1, one successful attempt on mode 2.
        assert_eq!(fake.set_mode_calls(), 2);
    }
}
