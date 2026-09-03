//! Patina UEFI string types
//!
//! Idiomatic, safe wrappers for the two string encodings defined by the UEFI Specification:
//!
//! - **CHAR8** ([`Char8Str`] / [`Char8String`] / [`Char8Array`]): A one byte (exactly 8-bit)
//!   character string using the ASCII / ISO-Latin-1 (ISO-8859-1) character set. Every byte in the
//!   range `0x01..=0xFF` is a valid character, so any byte value maps directly to a Unicode scalar
//!   in `U+0001..=U+00FF`.
//! - **CHAR16** ([`Char16Str`] / [`Char16String`] / [`Char16Array`]): A two byte character string
//!   using the UCS-2 encoding (as defined by Unicode 2.1 and ISO/IEC 10646). UCS-2 is a fixed width
//!   encoding. Characters that would require a UTF-16 surrogate pair (code points above `U+FFFF`)
//!   and lone surrogate code units (`U+D800..=U+DFFF`) are not valid UCS-2 and are rejected.
//!
//! ## NUL termination
//!
//! All string types in this module follow the same model as [`core::ffi::CStr`] and
//! [`alloc::ffi::CString`]: the value is guaranteed to be terminated by a single NUL and to contain
//! no interior NUL. This makes [`Char8Str::as_ptr`] and [`Char16Str::as_ptr`] always safe to hand to
//! `extern "efiapi"` interfaces that expect a NUL-terminated string.
//!
//! ## Type overview
//!
//! Each encoding provides three types that mirror the roles of [`str`], [`alloc::string::String`],
//! and a fixed size array:
//!
//! | Role                                       | CHAR8           | CHAR16           |
//! |--------------------------------------------|-----------------|------------------|
//! | Borrowed, unsized view (works in `no_std`) | [`Char8Str`]    | [`Char16Str`]    |
//! | Owned, heap-backed (requires `alloc`)      | [`Char8String`] | [`Char16String`] |
//! | Owned, fixed capacity (usable in `const`)  | [`Char8Array`]  | [`Char16Array`]  |
//!
//! ## When to use which type
//!
//! - Use [`Char16Str`] / [`Char8Str`] to borrow an existing NUL-terminated buffer (for example, a
//!   value received across an FFI boundary) or to accept a string in a function parameter.
//! - Use [`Char16String`] / [`Char8String`] to build or own a string on the heap, typically by
//!   converting from a Rust [`str`].
//! - Use [`Char16Array`] / [`Char8Array`] for compile-time string constants and for `#[repr(C)]`
//!   structure fields that hold a fixed size character array (a common UEFI layout).
//!
//! ## Compile-time literals
//!
//! The [`char16!`](crate::char16) and [`char8!`](crate::char8) macros build a `&'static` string from
//! a string literal, validating the encoding at compile time:
//!
//! ```rust
//! use patina::{char16, char8};
//!
//! let ascii = char8!("Firmware");
//! let wide = char16!("Firmware");
//!
//! assert_eq!(ascii.len(), 8);
//! assert_eq!(wide.len(), 8);
//! assert!(wide == "Firmware");
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::base::error::EfiError;

pub mod char16;
pub mod char8;

pub use char8::{Char8Array, Char8Str};
pub use char16::{Char16Array, Char16Str};

#[cfg(any(test, feature = "alloc"))]
pub use char8::Char8String;
#[cfg(any(test, feature = "alloc"))]
pub use char16::Char16String;

#[doc(hidden)]
pub use char8::latin1_capacity;
#[doc(hidden)]
pub use char16::ucs2_capacity;

/// Decodes the UTF-8 sequence starting at `bytes[index]`, returning the code point and its length in
/// bytes.
///
/// This assumes the input is valid UTF-8, which is guaranteed for the bytes of a Rust [`str`].
///
/// # Note
///
/// This function is provided because [`str::chars()`] and [`str::char_indices()`] are not `const fn`s
/// and return iterators. [`core::iter::Iterator`] is not a `const` trait on stable Rust, so this function
/// provides UTF-8 decoding in `const` context.
#[allow(clippy::indexing_slicing)] // Input is valid UTF-8, so continuation bytes are always present.
const fn decode_utf8(bytes: &[u8], index: usize) -> (u32, usize) {
    let b0 = bytes[index];
    if b0 < 0x80 {
        (b0 as u32, 1)
    } else if b0 >> 5 == 0b110 {
        let b1 = bytes[index + 1];
        (((b0 as u32 & 0x1F) << 6) | (b1 as u32 & 0x3F), 2)
    } else if b0 >> 4 == 0b1110 {
        let b1 = bytes[index + 1];
        let b2 = bytes[index + 2];
        (((b0 as u32 & 0x0F) << 12) | ((b1 as u32 & 0x3F) << 6) | (b2 as u32 & 0x3F), 3)
    } else {
        let b1 = bytes[index + 1];
        let b2 = bytes[index + 2];
        let b3 = bytes[index + 3];
        (((b0 as u32 & 0x07) << 18) | ((b1 as u32 & 0x3F) << 12) | ((b2 as u32 & 0x3F) << 6) | (b3 as u32 & 0x3F), 4)
    }
}

/// Returns the number of [`char`]s in `s`.
const fn char_count(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut index = 0;
    let mut count = 0;
    while index < bytes.len() {
        let (_, len) = decode_utf8(bytes, index);
        index += len;
        count += 1;
    }
    count
}

/// Error returned when constructing or validating a UEFI string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StringError {
    /// A NUL character was found before the end of the string, at the given index.
    InteriorNul {
        /// Index of the interior NUL, counted in characters (CHAR8) or code units (CHAR16).
        position: usize,
    },
    /// The provided data did not end with a NUL terminator.
    MissingNulTerminator,
    /// A byte buffer holding little-endian CHAR16 code units did not contain a whole number of
    /// `u16` values (its length was not a multiple of two).
    OddByteLength {
        /// The length of the offending byte buffer.
        len: usize,
    },
    /// A character could not be represented in UCS-2.
    ///
    /// This occurs for code points above `U+FFFF` (which would require a UTF-16 surrogate pair) and
    /// for lone surrogate code units in the range `U+D800..=U+DFFF`.
    NotUcs2 {
        /// Index of the offending character, counted in characters or code units.
        position: usize,
        /// The offending Unicode code point or surrogate code unit.
        value: u32,
    },
    /// A character could not be represented in Latin-1 (CHAR8).
    ///
    /// This occurs for code points above `U+00FF`.
    NotLatin1 {
        /// Index of the offending character, counted in characters.
        position: usize,
        /// The offending Unicode code point.
        value: u32,
    },
    /// The string does not fit within the capacity of a fixed size buffer.
    TooLong {
        /// The number of characters or code units the buffer can hold, excluding the NUL terminator.
        capacity: usize,
        /// The number of characters or code units the string requires, including the NUL terminator.
        required: usize,
    },
}

impl core::fmt::Display for StringError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StringError::InteriorNul { position } => {
                write!(f, "interior NUL found at position {position}")
            }
            StringError::MissingNulTerminator => write!(f, "string is not NUL-terminated"),
            StringError::OddByteLength { len } => {
                write!(f, "byte buffer length {len} is not a multiple of two CHAR16 code units")
            }
            StringError::NotUcs2 { position, value } => {
                write!(f, "code point U+{value:04X} at position {position} is not valid UCS-2")
            }
            StringError::NotLatin1 { position, value } => {
                write!(f, "code point U+{value:04X} at position {position} is not valid Latin-1 (CHAR8)")
            }
            StringError::TooLong { capacity, required } => {
                write!(f, "string requires {required} code units but the buffer capacity is {capacity}")
            }
        }
    }
}

impl core::error::Error for StringError {}

impl From<StringError> for EfiError {
    fn from(_: StringError) -> Self {
        EfiError::InvalidParameter
    }
}

/// Builds a `&'static` [`Char16Str`] from a string literal, validated as UCS-2 at compile time.
///
/// The literal must not contain interior NUL characters or code points above `U+FFFF`; violating
/// either condition is a compile-time error.
///
/// # Examples
///
/// ```rust
/// use patina::char16;
///
/// let name = char16!("EFI");
/// assert_eq!(name.len(), 3);
/// assert!(name == "EFI");
/// ```
#[macro_export]
macro_rules! char16 {
    ($s:literal) => {{
        static CHAR16_LITERAL: $crate::string::Char16Array<{ $crate::string::ucs2_capacity($s) }> =
            $crate::string::Char16Array::from_str($s);
        CHAR16_LITERAL.as_char16_str()
    }};
}

/// Builds a `&'static` [`Char8Str`] from a string literal, validated as Latin-1 at compile time.
///
/// The literal must not contain interior NUL characters or code points above `U+00FF`; violating
/// either condition is a compile-time error.
///
/// # Examples
///
/// ```rust
/// use patina::char8;
///
/// let name = char8!("EFI");
/// assert_eq!(name.len(), 3);
/// assert!(name == "EFI");
/// ```
#[macro_export]
macro_rules! char8 {
    ($s:literal) => {{
        static CHAR8_LITERAL: $crate::string::Char8Array<{ $crate::string::latin1_capacity($s) }> =
            $crate::string::Char8Array::from_str($s);
        CHAR8_LITERAL.as_char8_str()
    }};
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use std::string::ToString;

    #[test]
    fn test_string_error_display() {
        assert_eq!(StringError::InteriorNul { position: 3 }.to_string(), "interior NUL found at position 3");
        assert_eq!(StringError::MissingNulTerminator.to_string(), "string is not NUL-terminated");
        assert_eq!(
            StringError::NotUcs2 { position: 1, value: 0x1F600 }.to_string(),
            "code point U+1F600 at position 1 is not valid UCS-2"
        );
        assert_eq!(
            StringError::NotLatin1 { position: 2, value: 0x0100 }.to_string(),
            "code point U+0100 at position 2 is not valid Latin-1 (CHAR8)"
        );
        assert_eq!(
            StringError::TooLong { capacity: 2, required: 4 }.to_string(),
            "string requires 4 code units but the buffer capacity is 2"
        );
    }

    #[test]
    fn test_string_error_into_efi_error() {
        let error: EfiError = StringError::MissingNulTerminator.into();
        assert_eq!(error, EfiError::InvalidParameter);
    }

    #[test]
    fn test_string_error_is_error_trait() {
        fn assert_error<T: core::error::Error>(_: &T) {}
        assert_error(&StringError::MissingNulTerminator);
    }
}
