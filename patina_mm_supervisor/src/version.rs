//! MM Supervisor Version
//!
//! The supervisor version and patch level reported by the `VERSION_INFO` request. Both are
//! computed at compile time from the crate version in `Cargo.toml`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Parses one numeric component of the crate version. Evaluated at compile time only.
const fn parse_version_number(text: &str) -> u32 {
    match u32::from_str_radix(text, 10) {
        Ok(value) => value,
        Err(_) => panic!("the crate version component is not a 32-bit decimal number"),
    }
}

const CRATE_VERSION_MAJOR: u32 = parse_version_number(env!("CARGO_PKG_VERSION_MAJOR"));
const CRATE_VERSION_MINOR: u32 = parse_version_number(env!("CARGO_PKG_VERSION_MINOR"));
const CRATE_VERSION_PATCH: u32 = parse_version_number(env!("CARGO_PKG_VERSION_PATCH"));

/// The last decimal digit of the version's minor field: 9 marks a release build, 8 a debug build.
const BUILD_FLAG: u32 = if cfg!(debug_assertions) { 8 } else { 9 };

// The C layout gives the major version two decimal digits and the minor version three.
const _: () = assert!(
    CRATE_VERSION_MAJOR <= 99 && CRATE_VERSION_MINOR <= 999,
    "the crate version does not fit the MM Supervisor version layout"
);

/// Supervisor version, built from the crate version.
///
/// Uses the layout of the C MM Supervisor (`MmSupervisorCore.inf`):
///
/// - Bits 31:16 hold the major version.
/// - Bits 15:0 hold the minor version times ten plus a build flag.
/// - The build flag is 9 for a release build and 8 for a debug build.
///
/// For example, crate version 24.1.x reports 24.0019 in a release build.
pub(crate) const VERSION: u32 = (CRATE_VERSION_MAJOR << 16) | (CRATE_VERSION_MINOR * 10 + BUILD_FLAG);

/// Supervisor patch level, the patch component of the crate version.
pub(crate) const PATCH_LEVEL: u32 = CRATE_VERSION_PATCH;

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn version_constants_follow_the_crate_version() {
        let major: u32 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
        let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap();
        let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap();
        let flag = if cfg!(debug_assertions) { 8 } else { 9 };

        assert_eq!(VERSION >> 16, major);
        assert_eq!(VERSION & 0xFFFF, minor * 10 + flag);
        assert_eq!(PATCH_LEVEL, patch);
    }

    #[test]
    fn parse_version_number_reads_decimal_components() {
        assert_eq!(parse_version_number("0"), 0);
        assert_eq!(parse_version_number("24"), 24);
        assert_eq!(parse_version_number("999"), 999);
    }
}
