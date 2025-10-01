//! Patina GUID type
//!
//! Types for working with GUIDs more ergonomically in Patina.
//!
//! ## Type Overview
//!
//! - [`Guid<'a>`] - A borrowed GUID that can reference existing data or contain parsed strings
//! - [`OwnedGuid`] - A owned GUID with static lifetime, type alias for `Guid<'static>`
//! - [`GuidError`] - Error type for GUID parsing operations
//!
//! ## When to use `Guid` vs `OwnedGuid`
//!
//! ### Use `Guid<'a>` when:
//!
//! - You already have an `efi::Guid` reference and want to wrap it for display/comparison
//! - You need to work with GUIDs that have a specific lifetime tied to some data structure
//! - You're working with temporary GUID references in function parameters
//!
//! ### Use `OwnedGuid` when:
//!
//! - Creating GUIDs from string literals or user input via [`OwnedGuid::try_from_string`]
//! - Storing GUIDs in structs or collections that need to own their data
//! - Returning GUIDs from functions where you can't guarantee the lifetime of source data
//! - Working with GUIDs that need to live beyond the scope of their creation
//!
//! ## Examples
//!
//! ```rust
//! use patina_sdk::{Guid, OwnedGuid, GuidError};
//! use r_efi::efi;
//!
//! // Creating from existing efi::Guid reference
//! let efi_guid = efi::Guid::from_fields(0x550e8400, 0xe29b, 0x41d4, 0xa7, 0x16, &[0x44, 0x66, 0x55, 0x44, 0x00, 0x00]);
//! let guid_ref = Guid::from_ref(&efi_guid);
//!
//! // Creating from a 16-byte array
//! let bytes = [0x00, 0x84, 0x0e, 0x55, 0x9b, 0xe2, 0xd4, 0x41, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00, 0x00];
//! let guid_from_bytes = Guid::from_bytes(&bytes);
//!
//! // Creating an owned GUID from a string
//! let owned_guid = OwnedGuid::try_from_string("550E8400-E29B-41D4-A716-446655440000")?;
//!
//! // Error handling
//! match OwnedGuid::try_from_string("invalid") {
//!     Ok(guid) => println!("Created GUID: {}", guid),
//!     Err(GuidError::InvalidLength { expected, actual }) => {
//!         println!("Wrong length: expected {expected}, got {actual}");
//!     },
//!     Err(GuidError::InvalidHexCharacter { position, character }) => {
//!         println!("Invalid hex character '{character}' at position {position}");
//!     },
//! }
//! # Ok::<(), GuidError>(())
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::error::EfiError;
use r_efi::efi;

/// The expected number of hexadecimal characters in a valid GUID string representation
const EXPECTED_HEX_CHARS: usize = 32;

/// GUID display format dash positions
const DASH_POSITIONS: [usize; 4] = [8, 12, 16, 20];

/// Error type for GUID parsing operations
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum GuidError {
    /// The provided string does not contain exactly 32 hexadecimal characters
    InvalidLength {
        /// Expected number of hex characters
        expected: usize,
        /// Actual number of hex characters found
        actual: usize,
    },
    /// The provided string contains invalid hexadecimal characters
    InvalidHexCharacter {
        /// Position of the invalid character in the string
        position: usize,
        /// The invalid character that was found
        character: char,
    },
}

impl core::fmt::Display for GuidError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GuidError::InvalidLength { expected, actual } => {
                write!(f, "Invalid GUID length: expected {expected} hex characters, found {actual}")
            }
            GuidError::InvalidHexCharacter { position, character } => {
                write!(f, "Invalid hex character '{character}' at position {position}")
            }
        }
    }
}

impl core::error::Error for GuidError {}

impl From<GuidError> for EfiError {
    fn from(_: GuidError) -> Self {
        EfiError::InvalidParameter
    }
}

/// A wrapper type for displaying UEFI GUIDs in a human-readable format.
///
/// This type can hold either a reference to an existing `efi::Guid` or an owned validated
/// string representation. The lifetime parameter `'a` represents the lifetime of referenced data.
///
/// For most use cases, prefer [`OwnedGuid`] when creating GUIDs from strings, or use
/// [`Guid::from_ref`] when wrapping existing `efi::Guid` references from code outside Patina.
///
/// # Construction
/// - Use [`Guid::from_ref`] to wrap existing `efi::Guid` references
/// - Use [`OwnedGuid::try_from_string`] to create owned GUIDs from string representations
///
/// String construction is fallible and will return a [`GuidError`] if the input is invalid.
#[derive(Clone)]
pub enum Guid<'a> {
    /// GUID from an existing `efi::Guid` reference with lifetime `'a`
    Borrowed(&'a efi::Guid),
    /// GUID from a parsed string representation, stored as structured EFI GUID for C interoperability
    Owned(efi::Guid),
}

/// A GUID that owns its data and has no lifetime dependencies.
///
/// This is a type alias for `Guid<'static>` and is the recommended type for:
/// - Creating GUIDs from string literals or user input
/// - Storing GUIDs in structs or collections
/// - Returning GUIDs from functions
/// - Any scenario where you need to own the GUID data
pub type OwnedGuid = Guid<'static>;

impl<'a> Guid<'a> {
    /// Create a new Guid from an `efi::Guid` reference
    pub fn from_ref(guid: &'a efi::Guid) -> Self {
        Self::Borrowed(guid)
    }

    /// Create a new owned GUID from a 16-byte array.
    ///
    /// `bytes` should 16-byte array representing the GUID in little-endian
    pub fn from_bytes(bytes: &[u8; 16]) -> OwnedGuid {
        let efi_guid = efi::Guid::from_bytes(bytes);
        OwnedGuid::Owned(efi_guid)
    }

    /// Get the GUID as a 16-byte array for fast comparison and binary operations.
    /// This provides better performance for equality checks by using byte-wise comparison.
    pub fn as_bytes(&self) -> [u8; 16] {
        match self {
            Self::Borrowed(guid) => *guid.as_bytes(),
            Self::Owned(guid) => *guid.as_bytes(),
        }
    }

    /// Get the GUID fields as individual components for compatibility with the EFI GUID fields.
    /// Returns: (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node)
    pub fn as_fields(&self) -> (u32, u16, u16, u8, u8, &[u8; 6]) {
        match self {
            Self::Borrowed(guid) => guid.as_fields(),
            Self::Owned(guid) => guid.as_fields(),
        }
    }

    /// Convert this GUID to an r_efi::efi::Guid for compatibility with code that directly
    /// interacts with that interface.
    ///
    /// Creates a new r_efi::efi::Guid with the same value.
    pub fn to_efi_guid(&self) -> efi::Guid {
        match self {
            Self::Borrowed(guid) => **guid,
            Self::Owned(guid) => *guid,
        }
    }

    /// Helper function to convert a character to uppercase hex if it's a valid hex digit
    fn to_upper_hex(c: char) -> Option<char> {
        match c {
            '0'..='9' | 'A'..='F' => Some(c),
            'a'..='f' => Some((c as u8 - b'a' + b'A') as char),
            _ => None,
        }
    }

    /// Gets the canonical GUID representation as a formatted string.
    /// Provides a consistent format for both Borrowed and Owned variants.
    fn to_canonical_string(&self) -> [char; EXPECTED_HEX_CHARS] {
        // Both variants have identical underlying efi::Guid, so we can use as_fields() directly
        let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = match self {
            Self::Borrowed(guid) => guid.as_fields(),
            Self::Owned(guid) => guid.as_fields(),
        };

        let mut result = [' '; EXPECTED_HEX_CHARS];
        let mut pos = 0;

        let mut add_hex = |value: u32, digits: usize| {
            for i in (0..digits).rev() {
                let nibble = ((value >> (i * 4)) & 0xF) as u8;
                result[pos] = match nibble {
                    0..=9 => (b'0' + nibble) as char,
                    10..=15 => (b'A' + nibble - 10) as char,
                    _ => unreachable!(),
                };
                pos += 1;
            }
        };

        // Format each field as uppercase hex
        add_hex(time_low, 8);
        add_hex(time_mid as u32, 4);
        add_hex(time_hi_and_version as u32, 4);
        add_hex(clk_seq_hi_res as u32, 2);
        add_hex(clk_seq_low as u32, 2);

        for &byte in node.iter() {
            add_hex(byte as u32, 2);
        }

        result
    }
}

impl OwnedGuid {
    /// Create a new GUID from raw field values.
    ///
    /// This constant method creates GUIDs using the standard GUID fields of time_low, time_mid,
    /// time_hi_and_version, clk_seq_hi_res, clk_seq_low, and the 6-byte node array.
    pub const fn from_fields(
        time_low: u32,
        time_mid: u16,
        time_hi_and_version: u16,
        clk_seq_hi_res: u8,
        clk_seq_low: u8,
        node: [u8; 6],
    ) -> Self {
        let efi_guid =
            efi::Guid::from_fields(time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, &node);
        Guid::Owned(efi_guid)
    }

    /// A constant representing the zero GUID (00000000-0000-0000-0000-000000000000).
    /// This is useful for placeholder values and comparisons.
    pub const ZERO: OwnedGuid = Self::from_fields(0, 0, 0, 0, 0, [0; 6]);

    /// Create a new Guid from a string representation, validating that it contains exactly 32 hex characters
    pub fn try_from_string(s: &str) -> core::result::Result<OwnedGuid, GuidError> {
        // Extract hex digits and convert them to uppercase
        let mut hex_chars = [' '; EXPECTED_HEX_CHARS];
        let mut hex_count = 0;

        for (char_position, c) in s.chars().enumerate() {
            let char_position = char_position + 1;
            if let Some(upper_c) = Guid::to_upper_hex(c) {
                if hex_count < EXPECTED_HEX_CHARS {
                    hex_chars[hex_count] = upper_c;
                    hex_count += 1;
                } else {
                    // More than 32 hex chars found - this is invalid
                    return Err(GuidError::InvalidLength { expected: EXPECTED_HEX_CHARS, actual: hex_count + 1 });
                }
            } else if !c.is_ascii_whitespace() && c != '-' {
                // Invalid character that's not whitespace or a dash
                return Err(GuidError::InvalidHexCharacter { position: char_position, character: c });
            }
        }

        // Exactly 32 hex digits should be present
        if hex_count != EXPECTED_HEX_CHARS {
            return Err(GuidError::InvalidLength { expected: EXPECTED_HEX_CHARS, actual: hex_count });
        }

        // Parse the hex characters into GUID fields and convert to bytes immediately
        let time_low = Self::parse_hex::<u32>(&hex_chars[0..8]);
        let time_mid = Self::parse_hex::<u16>(&hex_chars[8..12]);
        let time_hi_and_version = Self::parse_hex::<u16>(&hex_chars[12..16]);
        let clk_seq_hi_res = Self::parse_hex::<u8>(&hex_chars[16..18]);
        let clk_seq_low = Self::parse_hex::<u8>(&hex_chars[18..20]);
        let node = [
            Self::parse_hex::<u8>(&hex_chars[20..22]),
            Self::parse_hex::<u8>(&hex_chars[22..24]),
            Self::parse_hex::<u8>(&hex_chars[24..26]),
            Self::parse_hex::<u8>(&hex_chars[26..28]),
            Self::parse_hex::<u8>(&hex_chars[28..30]),
            Self::parse_hex::<u8>(&hex_chars[30..32]),
        ];

        let efi_guid =
            r_efi::efi::Guid::from_fields(time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, &node);

        Ok(Guid::Owned(efi_guid))
    }

    /// Parse a hex string slice into a numeric value of type T (e.g., u8, u16, u32).
    fn parse_hex<T>(hex_chars: &[char]) -> T
    where
        T: From<u8> + core::ops::Shl<usize, Output = T> + core::ops::BitOr<Output = T>,
    {
        let mut result = T::from(0);
        for &c in hex_chars {
            result = (result << 4) | T::from(Self::hex_char_to_value(c));
        }
        result
    }

    /// Convert a hex character to its numeric value (0-15).
    /// Assumes the character is already validated as a valid hex digit.
    fn hex_char_to_value(c: char) -> u8 {
        match c {
            '0'..='9' => c as u8 - b'0',
            'A'..='F' => c as u8 - b'A' + 10,
            'a'..='f' => c as u8 - b'a' + 10,
            _ => 0, // Should never happen with validated input
        }
    }
}
impl core::fmt::Display for Guid<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let hex_chars = self.to_canonical_string();

        // Format as: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
        for (i, &c) in hex_chars.iter().enumerate() {
            if DASH_POSITIONS.contains(&i) {
                write!(f, "-")?;
            }
            write!(f, "{}", c)?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for Guid<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Use the Display format for Debug as well, since this is more useful for GUIDs
        write!(f, "{}", self)
    }
}

impl PartialEq for Guid<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Guid<'_> {}

impl PartialOrd for Guid<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Guid<'_> {
    /// Compares two GUIDs using byte order.
    ///
    /// # Important Note
    ///
    /// This ordering is **purely for internal implementation purposes** mostly to
    /// enable the use of GUIDs as keys in `BTreeMap` collections. GUIDs do not have an
    /// inherent semantic ordering as they're globally unique identifiers, not values
    /// that are "less than" or "greater than" each other.
    ///
    /// This implementation provides a consistent, deterministic ordering based on
    /// the byte representation of the GUID for scenarios like:
    /// - Storing GUIDs as keys in sorted collections like `BTreeMap`
    /// - Deterministic iteration order for debugging purposes
    ///
    /// GUIDs should only be explicitly compared with the equality operator.
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_bytes().cmp(&other.as_bytes())
    }
}

impl<'a> From<&'a efi::Guid> for Guid<'a> {
    fn from(guid: &'a efi::Guid) -> Self {
        Self::Borrowed(guid)
    }
}

impl From<efi::Guid> for OwnedGuid {
    fn from(guid: efi::Guid) -> Self {
        Self::Owned(guid)
    }
}

impl<'a> TryFrom<&'a str> for OwnedGuid {
    type Error = GuidError;

    fn try_from(s: &'a str) -> core::result::Result<Self, Self::Error> {
        OwnedGuid::try_from_string(s)
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use core::mem::{align_of, size_of};
    use r_efi::base as r_efi_base;

    const TEST_GUID_FIELDS: (u32, u16, u16, u8, u8, &[u8; 6]) =
        (0x550e8400, 0xe29b, 0x41d4, 0xa7, 0x16, &[0x44, 0x66, 0x55, 0x44, 0x00, 0x00]);

    const TEST_GUID_STRING: &str = "550e8400-e29b-41d4-a716-446655440000";
    const TEST_GUID_STRING_UPPER: &str = "550E8400-E29B-41D4-A716-446655440000";
    const TEST_GUID_STRING_NO_DASHES: &str = "550e8400e29b41d4a716446655440000";
    const TEST_GUID_STRING_MIXED: &str = "550E8400-e29b-41D4-A716-446655440000";

    fn create_test_r_efi_guid() -> r_efi_base::Guid {
        let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = TEST_GUID_FIELDS;
        r_efi_base::Guid::from_fields(time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node)
    }

    #[test]
    fn memory_layout_compatible_with_r_efi_guid() {
        let r_efi_guid = create_test_r_efi_guid();
        let patina_guid = Guid::from_ref(&r_efi_guid);

        // Verify the underlying assumption the GUIDs in r_efi are 16 bytes in size
        assert_eq!(size_of::<r_efi_base::Guid>(), 16);

        // The underlying memory representation should match
        if let Guid::Borrowed(guid_ref) = patina_guid {
            assert_eq!(guid_ref.as_bytes(), r_efi_guid.as_bytes());
            assert_eq!(guid_ref.as_fields(), r_efi_guid.as_fields());
        }
    }

    #[test]
    fn memory_compatibility_and_size() {
        // Note: create_test_r_efi_guid() uses TEST_GUID_FIELDS which is the same GUID as TEST_GUID_STRING
        let r_efi_guid = create_test_r_efi_guid();
        let patina_guid_from_ref = Guid::from_ref(&r_efi_guid);
        let patina_guid_from_string = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        // Both variants must return exactly 16 bytes
        assert_eq!(patina_guid_from_ref.as_bytes().len(), 16);
        assert_eq!(patina_guid_from_string.as_bytes().len(), 16);

        // Both variants must produce identical byte representation
        assert_eq!(patina_guid_from_ref.as_bytes(), patina_guid_from_string.as_bytes());

        // Memory layout with r_efi::efi::Guid should be the same
        assert_eq!(patina_guid_from_ref.as_bytes(), *r_efi_guid.as_bytes());
        assert_eq!(patina_guid_from_string.as_bytes(), *r_efi_guid.as_bytes());

        // The Owned variant should contain the same fields as the original
        match patina_guid_from_string {
            Guid::Owned(guid) => {
                assert_eq!(size_of::<efi::Guid>(), 16);
                assert_eq!(guid.as_fields(), TEST_GUID_FIELDS);
            }
            _ => panic!("Expected Owned variant"),
        }

        let bytes_from_patina = patina_guid_from_ref.as_bytes();
        let roundtrip_r_efi = r_efi::efi::Guid::from_bytes(&bytes_from_patina);
        assert_eq!(roundtrip_r_efi.as_bytes(), r_efi_guid.as_bytes());
    }

    #[test]
    fn patina_guid_roundtrip_consistency() {
        // Create a Patina GUID from string
        let original_string_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        // Convert to a string with Display and back to a Patina GUID
        let display_string = format!("{}", original_string_guid);
        let roundtrip_guid = OwnedGuid::try_from_string(&display_string).unwrap();

        assert_eq!(original_string_guid.as_bytes(), roundtrip_guid.as_bytes());
        assert_eq!(original_string_guid, roundtrip_guid);

        // Test the other direction
        let r_efi_guid = create_test_r_efi_guid();
        let ref_guid = Guid::from_ref(&r_efi_guid);
        let ref_display = format!("{}", ref_guid);
        let bytes_guid = OwnedGuid::try_from_string(&ref_display).unwrap();

        assert_eq!(ref_guid.as_bytes(), bytes_guid.as_bytes());
        assert_eq!(ref_guid, bytes_guid);
    }

    #[test]
    fn patina_guid_api_methods() {
        let test_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();
        let r_efi_guid = create_test_r_efi_guid();
        let ref_guid = Guid::from_ref(&r_efi_guid);

        // Test as_bytes() method consistency
        let bytes_from_string = test_guid.as_bytes();
        let bytes_from_ref = ref_guid.as_bytes();

        // Both should produce valid 16-byte arrays for the same GUID
        assert_eq!(bytes_from_string.len(), 16);
        assert_eq!(bytes_from_ref.len(), 16);
        assert_eq!(bytes_from_string, bytes_from_ref);

        // Test Display formatting consistency
        let display_from_string = format!("{}", test_guid);
        let display_from_ref = format!("{}", ref_guid);
        assert_eq!(display_from_string, display_from_ref);
        assert_eq!(display_from_string, TEST_GUID_STRING_UPPER);
    }

    #[test]
    fn patina_guid_memory_efficiency() {
        // Test that our Patina GUID wrapper doesn't add excessive overhead
        let patina_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        // Verify that as_bytes() is consistent
        let bytes1 = patina_guid.as_bytes();
        let bytes2 = patina_guid.as_bytes();
        assert_eq!(bytes1, bytes2);

        // Test that both Patina GUID variants produce identical byte output for the same GUID
        let r_efi_guid = create_test_r_efi_guid();
        let ref_guid = Guid::from_ref(&r_efi_guid);
        assert_eq!(patina_guid.as_bytes(), ref_guid.as_bytes());

        // The enum should be the 16 bytes of the GUID plus space for the enum discriminant
        let guid_size = 16; // Core GUID data size
        let patina_size = size_of::<OwnedGuid>();

        // Allow some overhead for enum discriminant and alignment, but should be minimal
        assert!(
            patina_size <= guid_size + 8,
            "Patina GUID size ({}) is within expected limits ({})",
            patina_size,
            guid_size
        );
    }

    #[test]
    fn patina_guid_variant_behavior() {
        let r_efi_guid = create_test_r_efi_guid();

        let ref_guid = Guid::from_ref(&r_efi_guid);
        match ref_guid {
            Guid::Borrowed(_) => {}
            _ => panic!("Expected Borrowed variant"),
        }

        let bytes_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();
        match bytes_guid {
            Guid::Owned(_) => {}
            _ => panic!("Expected Owned variant"),
        }

        assert_eq!(ref_guid.as_bytes(), bytes_guid.as_bytes());
        assert_eq!(ref_guid, bytes_guid);
        assert_eq!(format!("{}", ref_guid), format!("{}", bytes_guid));

        let ref_guid_clone = ref_guid.clone();
        let bytes_guid_clone = bytes_guid.clone();
        assert_eq!(ref_guid, ref_guid_clone);
        assert_eq!(bytes_guid, bytes_guid_clone);

        let debug_ref = format!("{:?}", ref_guid);
        let debug_bytes = format!("{:?}", bytes_guid);
        assert_eq!(debug_ref, debug_bytes);
        assert_eq!(debug_ref, TEST_GUID_STRING_UPPER);
    }

    #[test]
    fn from_ref_construction() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid = Guid::from_ref(&r_efi_guid);

        match guid {
            Guid::Borrowed(guid_ref) => {
                assert_eq!(guid_ref.as_fields(), TEST_GUID_FIELDS);
            }
            _ => panic!("Expected Borrowed variant"),
        }
    }

    #[test]
    fn try_from_string_valid() {
        let test_cases = [TEST_GUID_STRING, TEST_GUID_STRING_UPPER, TEST_GUID_STRING_NO_DASHES, TEST_GUID_STRING_MIXED];

        for input in test_cases {
            let result = OwnedGuid::try_from_string(input);
            assert!(result.is_ok(), "Failed to parse valid GUID string: {}", input);

            match result.unwrap() {
                Guid::Owned(_) => {}
                _ => panic!("Expected Owned variant"),
            }
        }
    }

    #[test]
    fn try_from_string_invalid_length() {
        let invalid_cases =
            [("550e8400-e29b-41d4-a716-4466554400", 30), ("", 0), ("550e8400-e29b-41d4-a716-44665544000000", 34)];

        for (input, _expected_count) in invalid_cases {
            let result = OwnedGuid::try_from_string(input);
            assert!(result.is_err(), "Should have failed for invalid length: {}", input);

            match result.unwrap_err() {
                GuidError::InvalidLength { .. } => {}
                other => panic!("Expected InvalidLength error for: {}, got: {:?}", input, other),
            }
        }
    }

    #[test]
    fn try_from_string_invalid_mixed_cases() {
        let invalid_cases = ["too-short", "not-a-guid-at-all"];

        for input in invalid_cases {
            let result = OwnedGuid::try_from_string(input);
            assert!(result.is_err(), "Should have failed for invalid input: {}", input);
        }
    }

    #[test]
    fn try_from_string_invalid_characters() {
        let invalid_cases =
            ["550e8400-e29b-41d4-a716-44665544000g", "not-a-guid-at-all", "550e8400-e29b-41d4-a716-44665544000z"];

        for input in invalid_cases {
            let result = OwnedGuid::try_from_string(input);
            assert!(result.is_err(), "Should have failed for invalid character: {}", input);

            match result.unwrap_err() {
                GuidError::InvalidHexCharacter { .. } => {}
                _ => panic!("Expected InvalidHexCharacter error for: {}", input),
            }
        }
    }

    #[test]
    fn try_from_trait_implementations() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid_from_ref: Guid = (&r_efi_guid).into();

        let guid_from_string_result: core::result::Result<OwnedGuid, GuidError> = TEST_GUID_STRING.try_into();
        assert!(guid_from_string_result.is_ok());
        let guid_from_string = guid_from_string_result.unwrap();

        assert!(matches!(guid_from_ref, Guid::Borrowed(_)));
        assert!(matches!(guid_from_string, Guid::Owned(_)));
    }

    #[test]
    fn display_from_ref() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid = Guid::from_ref(&r_efi_guid);
        let display_string = format!("{}", guid);

        assert_eq!(display_string, TEST_GUID_STRING_UPPER);
    }

    #[test]
    fn display_from_valid_string() {
        let test_cases = [TEST_GUID_STRING, TEST_GUID_STRING_UPPER, TEST_GUID_STRING_NO_DASHES, TEST_GUID_STRING_MIXED];

        for input in test_cases {
            let guid = OwnedGuid::try_from_string(input).expect("Valid GUID string should parse");
            let display_string = format!("{}", guid);
            assert_eq!(display_string, TEST_GUID_STRING_UPPER);
        }
    }

    #[test]
    fn debug_format() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid = Guid::from_ref(&r_efi_guid);
        let debug_string = format!("{:?}", guid);

        assert_eq!(debug_string, TEST_GUID_STRING_UPPER);
    }

    #[test]
    fn equality_same_variants() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid1 = Guid::from_ref(&r_efi_guid);
        let guid2 = Guid::from_ref(&r_efi_guid);

        assert_eq!(guid1, guid2);

        let guid3 = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();
        let guid4 = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        assert_eq!(guid3, guid4);
    }

    #[test]
    fn equality_different_variants() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid_from_ref = Guid::from_ref(&r_efi_guid);

        let test_cases = [TEST_GUID_STRING, TEST_GUID_STRING_UPPER, TEST_GUID_STRING_NO_DASHES, TEST_GUID_STRING_MIXED];

        for input in test_cases {
            let guid_from_string = OwnedGuid::try_from_string(input).unwrap();
            assert_eq!(guid_from_ref, guid_from_string, "Failed for input: {}", input);
        }
    }

    #[test]
    fn inequality_different_guids() {
        let r_efi_guid1 = create_test_r_efi_guid();
        let r_efi_guid2 = r_efi_base::Guid::from_fields(
            0x12345678,
            0x1234,
            0x5678,
            0x90,
            0xab,
            &[0xcd, 0xef, 0x12, 0x34, 0x56, 0x78],
        );

        let guid1 = Guid::from_ref(&r_efi_guid1);
        let guid2 = Guid::from_ref(&r_efi_guid2);

        assert_ne!(guid1, guid2);
    }

    #[test]
    fn canonical_string_conversion() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid_from_ref = Guid::from_ref(&r_efi_guid);
        let guid_from_string = OwnedGuid::try_from_string(TEST_GUID_STRING_MIXED).unwrap();

        let canonical1 = guid_from_ref.to_canonical_string();
        let canonical2 = guid_from_string.to_canonical_string();

        assert_eq!(canonical1, canonical2);
    }

    #[test]
    fn hex_character_validation() {
        assert_eq!(Guid::to_upper_hex('0'), Some('0'));
        assert_eq!(Guid::to_upper_hex('9'), Some('9'));
        assert_eq!(Guid::to_upper_hex('a'), Some('A'));
        assert_eq!(Guid::to_upper_hex('f'), Some('F'));
        assert_eq!(Guid::to_upper_hex('A'), Some('A'));
        assert_eq!(Guid::to_upper_hex('F'), Some('F'));
        assert_eq!(Guid::to_upper_hex('g'), None);
        assert_eq!(Guid::to_upper_hex('-'), None);
        assert_eq!(Guid::to_upper_hex(' '), None);
    }

    #[test]
    fn clone_functionality() {
        let r_efi_guid = create_test_r_efi_guid();
        let guid1 = Guid::from_ref(&r_efi_guid);
        let guid2 = guid1.clone();

        assert_eq!(guid1, guid2);

        let guid3 = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();
        let guid4 = guid3.clone();

        assert_eq!(guid3, guid4);
    }

    #[test]
    fn r_efi_guid_fields_consistency() {
        let r_efi_guid = create_test_r_efi_guid();
        let fields = r_efi_guid.as_fields();

        assert_eq!(fields, TEST_GUID_FIELDS);

        let bytes = r_efi_guid.as_bytes();
        let reconstructed = r_efi_base::Guid::from_bytes(bytes);

        assert_eq!(reconstructed.as_fields(), TEST_GUID_FIELDS);
    }

    #[test]
    fn whitespace_handling() {
        let spaced_guid = " 550e8400-e29b-41d4-a716-446655440000 ";
        let guid = OwnedGuid::try_from_string(spaced_guid).expect("Should handle whitespace");
        assert_eq!(format!("{}", guid), TEST_GUID_STRING_UPPER);
    }

    #[test]
    fn error_conversion_to_efi_error() {
        let error = GuidError::InvalidLength { expected: 32, actual: 30 };
        let efi_error: EfiError = error.into();
        assert_eq!(efi_error, EfiError::InvalidParameter);

        let error = GuidError::InvalidHexCharacter { position: 5, character: 'z' };
        let efi_error: EfiError = error.into();
        assert_eq!(efi_error, EfiError::InvalidParameter);
    }

    #[test]
    fn error_display() {
        let error = GuidError::InvalidLength { expected: 32, actual: 30 };
        let display = format!("{}", error);
        assert_eq!(display, "Invalid GUID length: expected 32 hex characters, found 30");

        let error = GuidError::InvalidHexCharacter { position: 5, character: 'z' };
        let display = format!("{}", error);
        assert_eq!(display, "Invalid hex character 'z' at position 5");
    }

    #[test]
    fn c_interop_from_ref_variant() {
        let r_efi_guid = create_test_r_efi_guid();
        let patina_guid = Guid::from_ref(&r_efi_guid);

        let patina_bytes = patina_guid.as_bytes();
        let r_efi_bytes = r_efi_guid.as_bytes();

        assert_eq!(patina_bytes, *r_efi_bytes);
        assert_eq!(patina_bytes.len(), 16);

        let patina_fields = match patina_guid {
            Guid::Borrowed(guid) => guid.as_fields(),
            _ => panic!("Expected Borrowed variant"),
        };
        let r_efi_fields = r_efi_guid.as_fields();

        assert_eq!(patina_fields, r_efi_fields);
        assert_eq!(patina_fields, TEST_GUID_FIELDS);

        let r_efi_ptr = &r_efi_guid as *const r_efi_base::Guid;
        let patina_ptr = match patina_guid {
            Guid::Borrowed(guid) => guid as *const efi::Guid,
            _ => panic!("Expected Borrowed variant"),
        };

        assert_eq!(r_efi_ptr as *const u8, patina_ptr as *const u8);

        unsafe {
            let r_efi_slice = core::slice::from_raw_parts(r_efi_ptr as *const u8, 16);
            let patina_slice = core::slice::from_raw_parts(patina_ptr as *const u8, 16);
            assert_eq!(r_efi_slice, patina_slice);
        }
    }

    #[test]
    fn c_interop_from_bytes_variant() {
        let patina_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        let patina_bytes = patina_guid.as_bytes();
        assert_eq!(patina_bytes.len(), 16);

        let r_efi_guid = create_test_r_efi_guid();
        let r_efi_bytes = r_efi_guid.as_bytes();

        assert_eq!(patina_bytes, *r_efi_bytes);

        let patina_fields = match patina_guid {
            Guid::Owned(ref guid) => guid.as_fields(),
            _ => panic!("Expected Owned variant"),
        };
        let r_efi_fields = r_efi_guid.as_fields();

        assert_eq!(patina_fields, r_efi_fields);
        assert_eq!(patina_fields, TEST_GUID_FIELDS);

        let patina_as_efi = match &patina_guid {
            Guid::Owned(guid) => *guid,
            _ => panic!("Expected Owned variant"),
        };

        assert_eq!(core::mem::size_of_val(&patina_as_efi), 16);
        assert_eq!(patina_as_efi.as_bytes(), r_efi_guid.as_bytes());

        let patina_ptr = &patina_as_efi as *const efi::Guid;
        unsafe {
            let patina_slice = core::slice::from_raw_parts(patina_ptr as *const u8, 16);
            let r_efi_slice = core::slice::from_raw_parts(&r_efi_guid as *const _ as *const u8, 16);
            assert_eq!(patina_slice, r_efi_slice);
        }
    }

    #[test]
    fn c_interop_cross_variant_compatibility() {
        let r_efi_guid = create_test_r_efi_guid();
        let from_ref_guid = Guid::from_ref(&r_efi_guid);
        let from_bytes_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        let ref_bytes = from_ref_guid.as_bytes();
        let bytes_bytes = from_bytes_guid.as_bytes();

        assert_eq!(ref_bytes, bytes_bytes);
        assert_eq!(ref_bytes.len(), 16);
        assert_eq!(bytes_bytes.len(), 16);

        let ref_fields = match from_ref_guid {
            Guid::Borrowed(guid) => guid.as_fields(),
            _ => panic!("Expected Borrowed variant"),
        };
        let bytes_fields = match from_bytes_guid {
            Guid::Owned(ref guid) => guid.as_fields(),
            _ => panic!("Expected Owned variant"),
        };

        assert_eq!(ref_fields, bytes_fields);
        assert_eq!(ref_fields, TEST_GUID_FIELDS);

        let ref_c_guid = match from_ref_guid {
            Guid::Borrowed(guid) => guid,
            _ => panic!("Expected Borrowed variant"),
        };
        let bytes_c_guid = match &from_bytes_guid {
            Guid::Owned(guid) => *guid,
            _ => panic!("Expected Owned variant"),
        };

        assert_eq!(ref_c_guid.as_bytes(), bytes_c_guid.as_bytes());
        assert_eq!(core::mem::size_of_val(ref_c_guid), core::mem::size_of_val(&bytes_c_guid));
    }

    #[test]
    fn c_interop_memory_alignment() {
        let r_efi_guid = create_test_r_efi_guid();
        let from_ref_guid = Guid::from_ref(&r_efi_guid);
        let from_bytes_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        assert_eq!(align_of::<r_efi_base::Guid>(), align_of::<efi::Guid>());
        assert_eq!(size_of::<r_efi_base::Guid>(), size_of::<efi::Guid>());
        assert_eq!(size_of::<r_efi_base::Guid>(), 16);

        let ref_c_guid = match from_ref_guid {
            Guid::Borrowed(guid) => guid,
            _ => panic!("Expected Borrowed variant"),
        };
        let bytes_c_guid = match &from_bytes_guid {
            Guid::Owned(guid) => *guid,
            _ => panic!("Expected Owned variant"),
        };

        let ref_ptr = ref_c_guid as *const efi::Guid;
        let bytes_ptr = &bytes_c_guid as *const efi::Guid;

        assert_eq!(ref_ptr as usize % align_of::<efi::Guid>(), 0);
        assert_eq!(bytes_ptr as usize % align_of::<efi::Guid>(), 0);

        assert_eq!((ref_ptr as usize) % align_of::<r_efi_base::Guid>(), 0);
        assert_eq!((bytes_ptr as usize) % align_of::<r_efi_base::Guid>(), 0);
    }

    #[test]
    fn c_interop_uefi_byte_order() {
        let r_efi_guid = create_test_r_efi_guid();
        let from_ref_guid = Guid::from_ref(&r_efi_guid);
        let from_bytes_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        // Breaking TEST_GUID_STRING into its little-endian byte representation
        let expected_bytes = [
            0x00, 0x84, 0x0e, 0x55, // time_low (0x550e8400) little-endian
            0x9b, 0xe2, // time_mid (0xe29b) little-endian
            0xd4, 0x41, // time_hi_and_version (0x41d4) little-endian
            0xa7, // clk_seq_hi_res (0xa7)
            0x16, // clk_seq_low (0x16)
            0x44, 0x66, 0x55, 0x44, 0x00, 0x00, // node array
        ];

        assert_eq!(from_ref_guid.as_bytes(), expected_bytes);
        assert_eq!(from_bytes_guid.as_bytes(), expected_bytes);
        assert_eq!(r_efi_guid.as_bytes(), &expected_bytes);

        unsafe {
            let r_efi_ptr = &r_efi_guid as *const r_efi_base::Guid;
            let r_efi_slice = core::slice::from_raw_parts(r_efi_ptr as *const u8, 16);
            assert_eq!(r_efi_slice, expected_bytes);
        }
    }

    #[test]
    fn from_bytes_method() {
        let test_bytes = [
            0x00, 0x84, 0x0e, 0x55, // time_low (0x550e8400) little-endian
            0x9b, 0xe2, // time_mid (0xe29b) little-endian
            0xd4, 0x41, // time_hi_and_version (0x41d4) little-endian
            0xa7, // clk_seq_hi_res (0xa7)
            0x16, // clk_seq_low (0x16)
            0x44, 0x66, 0x55, 0x44, 0x00, 0x00, // node array
        ];

        let guid_from_bytes = Guid::from_bytes(&test_bytes);
        let guid_from_string = OwnedGuid::try_from_string(TEST_GUID_STRING).unwrap();

        // Both should produce the same result
        assert_eq!(guid_from_bytes, guid_from_string);
        assert_eq!(guid_from_bytes.as_bytes(), test_bytes);
        assert_eq!(guid_from_bytes.as_bytes(), guid_from_string.as_bytes());

        match guid_from_bytes {
            Guid::Owned(_) => {}
            _ => panic!("Expected Owned variant from from_bytes"),
        }

        // Verify the fields match expected values
        let fields = guid_from_bytes.as_fields();
        assert_eq!(fields, TEST_GUID_FIELDS);

        // Verify display formatting
        assert_eq!(format!("{}", guid_from_bytes), TEST_GUID_STRING_UPPER);
    }
}
