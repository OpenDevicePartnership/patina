//! CHAR16 (UCS-2) string types
//!
//! See the [module documentation](crate::base::string) for an overview of the UEFI string types.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use super::{StringError, char_count, decode_utf8};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::char8::Char8Array;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

#[cfg(any(test, feature = "alloc"))]
use super::char8::{Char8Str, Char8String};

/// The inclusive range of UTF-16 surrogate code units, which are not valid UCS-2.
const SURROGATE_RANGE: core::ops::RangeInclusive<u16> = 0xD800..=0xDFFF;

/// A borrowed, NUL-terminated UCS-2 (UEFI CHAR16) string.
///
/// This is the CHAR16 analogue of [`core::ffi::CStr`]. It is an unsized type that is always accessed
/// behind a reference (`&Char16Str`). The referenced data is guaranteed to:
///
/// - End with a single NUL (`0x0000`) terminator,
/// - Contain no interior NUL, and
/// - Contain only valid UCS-2 code units (no surrogates in `0xD800..=0xDFFF`).
///
/// Because the terminator is guaranteed, [`Char16Str::as_ptr`] can be passed directly to
/// `extern "efiapi"` interfaces expecting a `*const CHAR16`.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char16Str;
///
/// let units = [0x0045u16, 0x0046, 0x0049, 0x0000]; // "EFI\0"
/// let s = Char16Str::from_units_with_nul(&units).unwrap();
/// assert_eq!(s.len(), 3);
/// assert!(s == "EFI");
/// ```
#[repr(transparent)]
pub struct Char16Str([u16]);

impl Char16Str {
    /// An empty, NUL-terminated CHAR16 string.
    // SAFETY: `&[0]` is a single NUL code unit that is trivially NUL-terminated with no interior NUL.
    pub const EMPTY: &'static Char16Str = unsafe { Self::from_units_with_nul_unchecked(&[0]) };

    /// Creates a `&Char16Str` from a slice of code units that ends with a NUL terminator.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `units` does not end with a NUL,
    /// [`StringError::InteriorNul`] if a NUL appears before the end, or [`StringError::NotUcs2`] if a
    /// surrogate code unit is present.
    pub fn from_units_with_nul(units: &[u16]) -> Result<&Char16Str, StringError> {
        let body = match units.split_last() {
            Some((0, body)) => body,
            _ => return Err(StringError::MissingNulTerminator),
        };

        for (position, &unit) in body.iter().enumerate() {
            if unit == 0 {
                return Err(StringError::InteriorNul { position });
            }
            if SURROGATE_RANGE.contains(&unit) {
                return Err(StringError::NotUcs2 { position, value: u32::from(unit) });
            }
        }

        // SAFETY: The loop above verified that `units` is NUL-terminated, has no interior NUL, and
        // contains no surrogate code units, which are exactly the invariants of `Char16Str`.
        Ok(unsafe { Self::from_units_with_nul_unchecked(units) })
    }

    /// Creates a `&Char16Str` from a NUL-terminated slice without validating its contents.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `units` ends with a single NUL terminator, contains no interior
    /// NUL, and contains only valid UCS-2 code units.
    pub const unsafe fn from_units_with_nul_unchecked(units: &[u16]) -> &Char16Str {
        // SAFETY: `Char16Str` is `#[repr(transparent)]` over `[u16]`, so `&[u16]` and `&Char16Str`
        // share the same layout. The caller upholds the value invariants.
        unsafe { &*(core::ptr::from_ref::<[u16]>(units) as *const Char16Str) }
    }

    /// Creates a `&Char16Str` from the code units up to and including the first NUL, ignoring
    /// anything after it.
    ///
    /// This mirrors [`core::ffi::CStr::from_bytes_until_nul`]. It is useful for fixed-size or
    /// over-allocated buffers that may carry trailing NUL padding (or other data) after the logical
    /// end of the string, where [`Char16Str::from_units_with_nul`] would reject anything after the
    /// first NUL as an interior NUL.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `units` contains no NUL at all, or
    /// [`StringError::NotUcs2`] if a surrogate code unit appears before the first NUL.
    // `end` is a valid index into `units` because it came from `position`, so the slice can't panic.
    #[allow(clippy::indexing_slicing)]
    pub fn from_units_until_nul(units: &[u16]) -> Result<&Char16Str, StringError> {
        let end = units.iter().position(|&unit| unit == 0).ok_or(StringError::MissingNulTerminator)?;
        Self::from_units_with_nul(&units[..=end])
    }

    /// Creates a `&Char16Str` by scanning a raw pointer for a NUL terminator.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// - `ptr` is non-null and aligned, even for a zero-length string.
    /// - The memory pointed to by `ptr` contains a sequence of `u16` values that is valid for
    ///   reads up to and including a NUL terminator.
    ///   - The entire memory range of this `Char16Str`, from `ptr` through the terminator, must be
    ///     contained within a single allocation.
    ///   - The entire memory range must remain valid and unmodified for the lifetime `'a`.
    ///   - The NUL terminator must be within `isize::MAX` bytes of `ptr`.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::NotUcs2`] if the scanned data contains a surrogate code unit.
    pub unsafe fn from_ptr<'a>(ptr: *const u16) -> Result<&'a Char16Str, StringError> {
        let mut len = 0usize;
        // SAFETY: The caller guarantees `ptr` points to a NUL-terminated sequence valid for reads.
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len + 1` code units up to and including the terminator are valid for reads.
        let units = unsafe { core::slice::from_raw_parts(ptr, len + 1) };
        Self::from_units_with_nul(units)
    }

    /// Creates a `&Char16Str` by scanning at most `max_units` code units for a NUL terminator.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// - `ptr` is non-null and aligned, even for a zero-length string.
    /// - `max_units * size_of::<u16>()` must not exceed `isize::MAX` bytes.
    /// - The memory pointed to by `ptr` contains a sequence of `u16` values that is valid for
    ///   reads up to and including a NUL terminator.
    ///   - The entire memory range of this `Char16Str`, from `ptr` through the terminator, must be
    ///     contained within a single allocation.
    ///   - The entire memory range must remain valid and unmodified for the lifetime `'a`.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if no NUL is found within `max_units` code units,
    /// or [`StringError::NotUcs2`] if a surrogate code unit is present.
    pub unsafe fn from_ptr_bounded<'a>(ptr: *const u16, max_units: usize) -> Result<&'a Char16Str, StringError> {
        let mut len = 0usize;
        // SAFETY: The caller guarantees `max_units` code units are valid for reads.
        while len < max_units && unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        if len == max_units {
            return Err(StringError::MissingNulTerminator);
        }
        // SAFETY: `len + 1 <= max_units` code units up to and including the terminator are valid.
        let units = unsafe { core::slice::from_raw_parts(ptr, len + 1) };
        Self::from_units_with_nul(units)
    }

    /// Returns a pointer to the first code unit, suitable for passing to `extern "efiapi"` interfaces.
    ///
    /// The pointed-to sequence is always NUL-terminated.
    pub const fn as_ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }

    /// Returns the code units including the trailing NUL terminator.
    pub const fn as_units_with_nul(&self) -> &[u16] {
        &self.0
    }

    /// Returns the code units excluding the trailing NUL terminator.
    pub fn as_units(&self) -> &[u16] {
        match self.0.split_last() {
            Some((_, body)) => body,
            None => &[],
        }
    }

    /// Returns the number of code units, excluding the trailing NUL terminator.
    pub fn len(&self) -> usize {
        self.as_units().len()
    }

    /// Returns `true` if the string contains no characters (only the NUL terminator).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over the code units, excluding the terminator.
    ///
    /// This borrows each code unit rather than copying it. Call [`Iterator::copied`] on the result if
    /// owned `u16` values are needed.
    pub fn iter(&self) -> core::slice::Iter<'_, u16> {
        self.as_units().iter()
    }

    /// Returns an iterator over the characters as [`char`] values.
    ///
    /// Every valid UCS-2 code unit is a Unicode scalar value in the Basic Multilingual Plane, so this
    /// conversion is lossless for any string built through a checked constructor. If the string was
    /// built through an unchecked constructor and contains a surrogate code unit, that unit is mapped
    /// to [`char::REPLACEMENT_CHARACTER`] rather than panicking.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.iter().map(|&unit| char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER))
    }
}

impl PartialEq for Char16Str {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Char16Str {}

impl PartialOrd for Char16Str {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Char16Str {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl core::hash::Hash for Char16Str {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq<str> for Char16Str {
    fn eq(&self, other: &str) -> bool {
        self.chars().eq(other.chars())
    }
}

impl PartialEq<&str> for Char16Str {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<Char16Str> for str {
    fn eq(&self, other: &Char16Str) -> bool {
        other == self
    }
}

impl core::fmt::Display for Char16Str {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.chars() {
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for Char16Str {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("\"")?;
        for c in self.chars() {
            write!(f, "{}", c.escape_debug())?;
        }
        f.write_str("\"")
    }
}

/// Decodes a little-endian CHAR16 byte buffer into `u16` code units.
///
/// # Errors
///
/// Returns [`StringError::OddByteLength`] if `bytes` is not a whole number of `u16` code units.
#[cfg(any(test, feature = "alloc"))]
fn decode_le_units(bytes: &[u8]) -> Result<Vec<u16>, StringError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(StringError::OddByteLength { len: bytes.len() });
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes(pair.try_into().expect("chunks_exact(2) yields 2-byte slices")))
        .collect())
}

/// An owned, heap-backed, NUL-terminated UCS-2 (UEFI CHAR16) string.
///
/// This is the CHAR16 analogue of [`alloc::string::String`]. It dereferences to [`Char16Str`], so all
/// borrowed operations are available on an owned value.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char16String;
///
/// let s = Char16String::try_from_str("Boot0001").unwrap();
/// assert_eq!(s.len(), 8);
/// assert!(s == "Boot0001");
/// ```
#[cfg(any(test, feature = "alloc"))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Char16String(Vec<u16>);

#[cfg(any(test, feature = "alloc"))]
impl Char16String {
    /// Creates an empty string containing only the NUL terminator.
    pub fn new() -> Self {
        Self(alloc::vec![0])
    }

    /// Creates a `Char16String` by encoding a Rust [`str`] as UCS-2.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::InteriorNul`] if the input contains a NUL character, or
    /// [`StringError::NotUcs2`] if it contains a code point above `U+FFFF` (which cannot be
    /// represented in UCS-2 without a surrogate pair).
    pub fn try_from_str(s: &str) -> Result<Self, StringError> {
        let mut units = Vec::with_capacity(s.len() + 1);
        for (position, c) in s.chars().enumerate() {
            let value = c as u32;
            if value == 0 {
                return Err(StringError::InteriorNul { position });
            }
            if value > 0xFFFF {
                return Err(StringError::NotUcs2 { position, value });
            }
            units.push(value as u16);
        }
        units.push(0);
        Ok(Self(units))
    }

    /// Creates a `Char16String` from a `Vec<u16>` that ends with a NUL terminator.
    ///
    /// This validates `units` the same way [`Char16Str::from_units_with_nul`] does, then takes
    /// ownership of the buffer directly instead of cloning it.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `units` does not end with a NUL,
    /// [`StringError::InteriorNul`] if a NUL appears before the end, or [`StringError::NotUcs2`] if a
    /// surrogate code unit is present.
    pub fn from_units_with_nul(units: Vec<u16>) -> Result<Self, StringError> {
        Char16Str::from_units_with_nul(&units)?;
        Ok(Self(units))
    }

    /// Creates a `Char16String` by decoding a little-endian CHAR16 byte buffer that ends with a NUL
    /// terminator.
    ///
    /// This is the byte-oriented companion to [`Char16Str::from_units_with_nul`]. UEFI stores CHAR16
    /// strings as little-endian byte sequences, so this pairs each two bytes into a `u16` before
    /// applying the same NUL and UCS-2 validation.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::OddByteLength`] if `bytes` is not a whole number of `u16` code units,
    /// [`StringError::MissingNulTerminator`] if `bytes` does not end with a NUL,
    /// [`StringError::InteriorNul`] if a NUL appears before the end, or [`StringError::NotUcs2`] if a
    /// surrogate code unit is present.
    pub fn from_le_bytes_with_nul(bytes: &[u8]) -> Result<Self, StringError> {
        Self::from_units_with_nul(decode_le_units(bytes)?)
    }

    /// Creates a `Char16String` by decoding a little-endian CHAR16 byte buffer up to and including
    /// the first NUL, ignoring anything after it.
    ///
    /// This is the byte-oriented companion to [`Char16Str::from_units_until_nul`]. It is useful for
    /// fixed-size or over-allocated buffers that may carry trailing padding after the logical end of
    /// the string, such as the `UserInterface` section of a firmware file.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::OddByteLength`] if `bytes` is not a whole number of `u16` code units,
    /// [`StringError::MissingNulTerminator`] if no NUL code unit is present, or
    /// [`StringError::NotUcs2`] if a surrogate code unit appears before the first NUL.
    pub fn from_le_bytes_until_nul(bytes: &[u8]) -> Result<Self, StringError> {
        use alloc::borrow::ToOwned;
        let units = decode_le_units(bytes)?;
        Ok(Char16Str::from_units_until_nul(&units)?.to_owned())
    }

    /// Creates a `Char16String` from a NUL-terminated `Vec<u16>` without validating its contents.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `units` ends with a single NUL terminator, contains no interior
    /// NUL, and contains only valid UCS-2 code units.
    pub unsafe fn from_units_with_nul_unchecked(units: Vec<u16>) -> Self {
        Self(units)
    }

    /// Borrows this owned string as a [`Char16Str`].
    pub fn as_char16_str(&self) -> &Char16Str {
        // SAFETY: The type invariant guarantees `self.0` is a valid NUL-terminated UCS-2 sequence.
        unsafe { Char16Str::from_units_with_nul_unchecked(&self.0) }
    }

    /// Consumes the string and returns the backing code units, including the trailing NUL.
    pub fn into_units_with_nul(self) -> Vec<u16> {
        self.0
    }
}

#[cfg(any(test, feature = "alloc"))]
impl Default for Char16String {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::ops::Deref for Char16String {
    type Target = Char16Str;

    fn deref(&self) -> &Char16Str {
        self.as_char16_str()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::borrow::Borrow<Char16Str> for Char16String {
    fn borrow(&self) -> &Char16Str {
        self.as_char16_str()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl alloc::borrow::ToOwned for Char16Str {
    type Owned = Char16String;

    fn to_owned(&self) -> Char16String {
        Char16String(self.0.to_vec())
    }
}

#[cfg(any(test, feature = "alloc"))]
impl From<&Char16Str> for Char16String {
    fn from(s: &Char16Str) -> Self {
        use alloc::borrow::ToOwned;
        s.to_owned()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<'a> TryFrom<&'a str> for Char16String {
    type Error = StringError;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Self::try_from_str(s)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl From<&Char8Str> for Char16String {
    /// Converts Latin-1 to UCS-2.
    ///
    /// Always succeeds since every Latin-1 byte maps directly onto an identically-valued, non-surrogate
    /// UCS-2 code unit.
    fn from(value: &Char8Str) -> Self {
        let mut units = Vec::with_capacity(value.len() + 1);
        for &byte in value.iter() {
            units.push(u16::from(byte));
        }
        units.push(0);
        Self(units)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl From<&Char8String> for Char16String {
    fn from(value: &Char8String) -> Self {
        Self::from(value.as_char8_str())
    }
}

#[cfg(any(test, feature = "alloc"))]
impl From<Char8String> for Char16String {
    fn from(value: Char8String) -> Self {
        Self::from(&value)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::fmt::Display for Char16String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(self.as_char16_str(), f)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::fmt::Debug for Char16String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_char16_str(), f)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<str> for Char16String {
    fn eq(&self, other: &str) -> bool {
        self.as_char16_str() == other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<&str> for Char16String {
    fn eq(&self, other: &&str) -> bool {
        self.as_char16_str() == *other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char16String> for str {
    fn eq(&self, other: &Char16String) -> bool {
        other.as_char16_str() == self
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char16Str> for Char16String {
    fn eq(&self, other: &Char16Str) -> bool {
        self.as_char16_str() == other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<&Char16Str> for Char16String {
    fn eq(&self, other: &&Char16Str) -> bool {
        self.as_char16_str() == *other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char16String> for Char16Str {
    fn eq(&self, other: &Char16String) -> bool {
        self == other.as_char16_str()
    }
}

/// Returns the [`Char16Array`] capacity needed to hold `s` plus a NUL terminator.
///
/// This is an implementation detail of the [`char16!`](crate::char16) macro.
#[doc(hidden)]
pub const fn ucs2_capacity(s: &str) -> usize {
    // Every accepted UCS-2 character occupies a single code unit, plus one for the terminator.
    char_count(s) + 1
}

/// A fixed capacity, NUL-terminated UCS-2 (UEFI CHAR16) string of `N` code units.
///
/// The capacity `N` includes the trailing NUL terminator, so the array can hold up to `N - 1`
/// characters. Unused trailing slots are filled with NUL. This type is `#[repr(transparent)]` over
/// `[u16; N]`, making it suitable for `#[repr(C)]` structure fields.
///
/// Values can be built in `const` contexts using [`Char16Array::from_str`] and
/// [`Char16Array::try_from_str`], which is how the [`char16!`](crate::char16) macro validates literals
/// at compile time.
///
/// This type also implements [`zerocopy::FromBytes`] and [`zerocopy::IntoBytes`], so it can be used
/// directly as a field of a `#[repr(C)]`/`#[repr(C, packed)]` structure that derives those traits for
/// zero-copy parsing and serialization. Because `FromBytes` allows constructing a value from arbitrary
/// code units, a value built that way is not guaranteed to contain a NUL terminator. In that case,
/// [`Char16Array::as_char16_str`] (and [`Deref`](core::ops::Deref)) return an empty string rather than
/// panicking.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char16Array;
///
/// const NAME: Char16Array<4> = Char16Array::from_str("EFI");
/// assert_eq!(NAME.as_char16_str().len(), 3);
/// assert!(NAME.as_char16_str() == "EFI");
/// ```
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Char16Array<const N: usize>([u16; N]);

impl<const N: usize> Char16Array<N> {
    /// Creates a `Char16Array` by encoding a Rust [`str`] as UCS-2.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::InteriorNul`] if the input contains a NUL character,
    /// [`StringError::NotUcs2`] if it contains a code point above `U+FFFF`, or
    /// [`StringError::TooLong`] if the encoded string plus its NUL terminator does not fit in `N` code
    /// units.
    // `out[out_index]` is guarded by the `out_index + 1 >= N` check, so `out_index < N`.
    #[allow(clippy::indexing_slicing)]
    pub const fn try_from_str(s: &str) -> Result<Self, StringError> {
        let bytes = s.as_bytes();
        let mut out = [0u16; N];
        let mut index = 0;
        let mut out_index = 0;
        let mut position = 0;

        while index < bytes.len() {
            let (value, len) = decode_utf8(bytes, index);
            if value == 0 {
                return Err(StringError::InteriorNul { position });
            }
            if value > 0xFFFF {
                return Err(StringError::NotUcs2 { position, value });
            }
            // Require room for this code unit and a trailing NUL terminator.
            if out_index + 1 >= N {
                return Err(StringError::TooLong { capacity: N.saturating_sub(1), required: out_index + 2 });
            }
            out[out_index] = value as u16;
            out_index += 1;
            index += len;
            position += 1;
        }

        // Guard against `N == 0`, where there is no room for even a terminator.
        if out_index >= N {
            return Err(StringError::TooLong { capacity: 0, required: 1 });
        }

        Ok(Self(out))
    }

    /// Creates a `Char16Array`, panicking on invalid input.
    ///
    /// This is intended for compile-time string literals: because it is a `const fn`, an invalid
    /// literal in a `const`/`static` context (including through the [`char16!`](crate::char16) macro)
    /// is reported as a compile-time error, not a runtime panic. Runtime callers that cannot
    /// guarantee valid input should use [`Char16Array::try_from_str`] instead, which returns a
    /// [`Result`] rather than panicking.
    ///
    /// # Panics
    ///
    /// Panics if the input is not valid UCS-2 or does not fit in `N` code units. In a `const` context
    /// this manifests as a compile-time error.
    pub const fn from_str(s: &str) -> Self {
        match Self::try_from_str(s) {
            Ok(array) => array,
            Err(_) => panic!("invalid UCS-2 string literal"),
        }
    }

    /// Borrows the populated portion of the array as a [`Char16Str`].
    ///
    /// Every value produced by a safe constructor (`try_from_str`, `from_str`, `Default`) contains a
    /// NUL terminator by construction. A value materialized from arbitrary code units via `FromBytes`
    /// is not guaranteed to, so this returns [`Char16Str::EMPTY`] in that case rather than panicking.
    #[allow(clippy::indexing_slicing)]
    pub fn as_char16_str(&self) -> &Char16Str {
        let mut len = 0;
        while len < N && self.0[len] != 0 {
            len += 1;
        }
        if len == N {
            return Char16Str::EMPTY;
        }
        // SAFETY: `self.0[..=len]` ends at the first NUL (found above) and contains only validated
        // UCS-2 code units.
        unsafe { Char16Str::from_units_with_nul_unchecked(&self.0[..=len]) }
    }

    /// Returns the full backing array, including trailing NUL padding.
    pub const fn as_array(&self) -> &[u16; N] {
        &self.0
    }
}

impl<const N: usize> core::ops::Deref for Char16Array<N> {
    type Target = Char16Str;

    fn deref(&self) -> &Char16Str {
        self.as_char16_str()
    }
}

impl<const N: usize> core::fmt::Display for Char16Array<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(self.as_char16_str(), f)
    }
}

impl<const N: usize> Default for Char16Array<N> {
    /// Returns an empty string backed by an all-zero array.
    ///
    /// `N` must be at least `1` to hold the mandatory NUL terminator; as with
    /// [`Char16Array::try_from_str`], a capacity of `0` cannot represent a valid string.
    fn default() -> Self {
        Self([0; N])
    }
}

impl<'a, const N: usize> TryFrom<&'a str> for Char16Array<N> {
    type Error = StringError;

    /// Equivalent to [`Char16Array::try_from_str`], provided for generic code written against the
    /// standard [`TryFrom`] trait.
    fn try_from(s: &'a str) -> Result<Self, StringError> {
        Self::try_from_str(s)
    }
}

impl<const N: usize> From<Char8Array<N>> for Char16Array<N> {
    /// Converts Latin-1 to UCS-2.
    ///
    /// Always succeeds since every Latin-1 byte maps directly onto an identically-valued, non-surrogate
    /// UCS-2 code unit, and a `Char8Array<N>`'s logical content (at most `N - 1` characters) always
    /// fits within `Char16Array<N>`'s equal capacity.
    // Note: `out[position]` is safe because `position < src.len() < N`, except when `N == 0`, where `src`
    // is empty and the loop never runs.
    #[allow(clippy::indexing_slicing)]
    fn from(value: Char8Array<N>) -> Self {
        let src = value.as_char8_str();
        // Note: The terminator and any trailing padding are already present due to zero-initialization.
        let mut out = [0u16; N];
        for (position, &byte) in src.iter().enumerate() {
            out[position] = u16::from(byte);
        }
        Self(out)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<const N: usize> From<Char16Array<N>> for Char16String {
    /// Converts a fixed-capacity array to a heap-owned string, dropping the const capacity `N`.
    fn from(value: Char16Array<N>) -> Self {
        use alloc::borrow::ToOwned;
        value.as_char16_str().to_owned()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<const N: usize> From<Char16Array<N>> for String {
    /// Converts to a native Rust string.
    ///
    /// Always succeeds. Lossless for arrays built through a safe constructor; a surrogate code unit
    /// reachable only via an unchecked constructor is replaced with [`char::REPLACEMENT_CHARACTER`]
    /// rather than causing a panic.
    fn from(value: Char16Array<N>) -> Self {
        value.as_char16_str().chars().collect()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::char16;
    use std::string::ToString;

    #[test]
    fn test_char16_from_units_with_nul_valid() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert_eq!(s.as_units(), &[0x0045, 0x0046, 0x0049]);
        assert_eq!(s.as_units_with_nul(), &units);
    }

    #[test]
    fn test_char16_empty_string() {
        let units = [0x0000u16];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn test_char16_missing_nul_terminator() {
        let units = [0x0045u16, 0x0046];
        assert_eq!(Char16Str::from_units_with_nul(&units), Err(StringError::MissingNulTerminator));
        assert_eq!(Char16Str::from_units_with_nul(&[]), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char16_interior_nul() {
        let units = [0x0045u16, 0x0000, 0x0049, 0x0000];
        assert_eq!(Char16Str::from_units_with_nul(&units), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char16_surrogate_rejected() {
        let units = [0x0045u16, 0xD800, 0x0000];
        assert_eq!(Char16Str::from_units_with_nul(&units), Err(StringError::NotUcs2 { position: 1, value: 0xD800 }));
    }

    #[test]
    fn test_char16_chars_and_iter() {
        let units = [0x0041u16, 0x00E9, 0x20AC, 0x0000]; // "Aé€"
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        let chars: std::vec::Vec<char> = s.chars().collect();
        assert_eq!(chars, ['A', 'é', '€']);
        let code_units: std::vec::Vec<u16> = s.iter().copied().collect();
        assert_eq!(code_units, [0x0041, 0x00E9, 0x20AC]);
    }

    #[test]
    fn test_char16_chars_lossy_on_unchecked_surrogate() {
        // A surrogate can only reach `chars()` via the unchecked constructor. Rather than panicking,
        // it is mapped to the Unicode replacement character.
        let units = [0x0041u16, 0xD800, 0x0000];
        // SAFETY: This deliberately bypasses validation to exercise the lossy fallback path.
        let s = unsafe { Char16Str::from_units_with_nul_unchecked(&units) };
        let chars: std::vec::Vec<char> = s.chars().collect();
        assert_eq!(chars, ['A', char::REPLACEMENT_CHARACTER]);
    }

    #[test]
    fn test_char16_from_ptr() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        // SAFETY: `units` is a valid NUL-terminated buffer that outlives the borrow.
        let s = unsafe { Char16Str::from_ptr(units.as_ptr()) }.unwrap();
        assert!(s == "EFI");
    }

    #[test]
    fn test_char16_from_ptr_bounded_finds_nul() {
        let units = [0x0045u16, 0x0046, 0x0000, 0x0049];
        // SAFETY: `units` has at least 4 readable code units.
        let s = unsafe { Char16Str::from_ptr_bounded(units.as_ptr(), 4) }.unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn test_char16_from_ptr_bounded_no_nul() {
        let units = [0x0045u16, 0x0046, 0x0049];
        // SAFETY: `units` has at least 3 readable code units.
        let result = unsafe { Char16Str::from_ptr_bounded(units.as_ptr(), 3) };
        assert_eq!(result, Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char16_as_ptr_points_to_first_unit() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        assert_eq!(s.as_ptr(), units.as_ptr());
    }

    #[test]
    fn test_char16_display_and_debug() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        assert_eq!(s.to_string(), "EFI");
        assert_eq!(std::format!("{s:?}"), "\"EFI\"");
    }

    #[test]
    fn test_char16_equality_and_ordering() {
        let a = [0x0041u16, 0x0000];
        let b = [0x0042u16, 0x0000];
        let a = Char16Str::from_units_with_nul(&a).unwrap();
        let b = Char16Str::from_units_with_nul(&b).unwrap();
        assert!(a < b);
        assert_ne!(a, b);
        assert!(a == "A");
        assert!(*"A" == *a);
    }

    #[test]
    fn test_char16string_try_from_str() {
        let s = Char16String::try_from_str("Boot0001").unwrap();
        assert_eq!(s.len(), 8);
        assert!(*s == *"Boot0001");
        assert_eq!(s.to_string(), "Boot0001");
    }

    #[test]
    fn test_char16string_partial_eq_str() {
        let s = Char16String::try_from_str("EFI").unwrap();
        assert!(s == "EFI");
        assert!(s == *"EFI");
        assert!(*"EFI" == s);
        assert!(s != "NOPE");
        assert!(*"NOPE" != s);
    }

    #[test]
    fn test_char16string_partial_eq_char16str() {
        let s = Char16String::try_from_str("EFI").unwrap();
        let same = Char16String::try_from_str("EFI").unwrap();
        let different = Char16String::try_from_str("NOPE").unwrap();
        let same_str: &Char16Str = same.as_char16_str();
        let different_str: &Char16Str = different.as_char16_str();
        assert!(s == *same_str);
        assert!(s == same_str);
        assert!(*same_str == s);
        assert!(s != *different_str);
        assert!(*different_str != s);
    }

    #[test]
    fn test_char16string_from_le_bytes_with_nul() {
        // "EFI\0" as little-endian CHAR16 bytes.
        let bytes = [0x45, 0x00, 0x46, 0x00, 0x49, 0x00, 0x00, 0x00];
        let s = Char16String::from_le_bytes_with_nul(&bytes).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s.to_string(), "EFI");
    }

    #[test]
    fn test_char16string_from_le_bytes_with_nul_rejects_trailing_padding() {
        // A trailing code unit after the terminator is an interior NUL for the with_nul constructor.
        let bytes = [0x45, 0x00, 0x00, 0x00, 0x49, 0x00, 0x00, 0x00];
        assert_eq!(Char16String::from_le_bytes_with_nul(&bytes), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char16string_from_le_bytes_until_nul_ignores_padding() {
        // "EFI\0" followed by trailing padding that must be ignored.
        let bytes = [0x45, 0x00, 0x46, 0x00, 0x49, 0x00, 0x00, 0x00, 0xFF, 0xFF];
        let s = Char16String::from_le_bytes_until_nul(&bytes).unwrap();
        assert_eq!(s.to_string(), "EFI");
        // The owned buffer is trimmed to exactly "EFI\0".
        assert_eq!(s.into_units_with_nul(), std::vec![0x0045, 0x0046, 0x0049, 0x0000]);
    }

    #[test]
    fn test_char16string_from_le_bytes_odd_length() {
        let bytes = [0x45, 0x00, 0x46];
        assert_eq!(Char16String::from_le_bytes_with_nul(&bytes), Err(StringError::OddByteLength { len: 3 }));
        assert_eq!(Char16String::from_le_bytes_until_nul(&bytes), Err(StringError::OddByteLength { len: 3 }));
    }

    #[test]
    fn test_char16string_from_le_bytes_until_nul_missing_terminator() {
        let bytes = [0x45, 0x00, 0x46, 0x00];
        assert_eq!(Char16String::from_le_bytes_until_nul(&bytes), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char16string_from_le_bytes_surrogate_rejected() {
        // 0xD800 is a lone surrogate and is not valid UCS-2.
        let bytes = [0x45, 0x00, 0x00, 0xD8, 0x00, 0x00];
        assert_eq!(
            Char16String::from_le_bytes_until_nul(&bytes),
            Err(StringError::NotUcs2 { position: 1, value: 0xD800 })
        );
    }

    #[test]
    fn test_char16string_rejects_non_bmp() {
        // U+1F600 requires a surrogate pair in UTF-16 and is not valid UCS-2.
        let result = Char16String::try_from_str("A\u{1F600}");
        assert_eq!(result, Err(StringError::NotUcs2 { position: 1, value: 0x1F600 }));
    }

    #[test]
    fn test_char16string_rejects_interior_nul() {
        let result = Char16String::try_from_str("A\0B");
        assert_eq!(result, Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char16string_new_and_default() {
        let a = Char16String::new();
        let b = Char16String::default();
        assert!(a.is_empty());
        assert_eq!(a, b);
        assert_eq!(a.clone().into_units_with_nul(), std::vec![0]);
    }

    #[test]
    fn test_char16string_to_owned_roundtrip() {
        use std::borrow::ToOwned;
        let owned = Char16String::try_from_str("Test").unwrap();
        let borrowed: &Char16Str = &owned;
        let reowned = borrowed.to_owned();
        assert_eq!(owned, reowned);
    }

    #[test]
    fn test_char16string_from_ref_char16str() {
        let owned = Char16String::try_from_str("Test").unwrap();
        let borrowed: &Char16Str = &owned;
        let converted = Char16String::from(borrowed);
        assert_eq!(owned, converted);
        let via_into: Char16String = borrowed.into();
        assert_eq!(owned, via_into);
    }

    #[test]
    fn test_char16_from_ptr_surrogate_error() {
        let units = [0x0041u16, 0xD800, 0x0000];
        // SAFETY: `units` is a NUL-terminated buffer that outlives the borrow.
        let result = unsafe { Char16Str::from_ptr(units.as_ptr()) };
        assert_eq!(result, Err(StringError::NotUcs2 { position: 1, value: 0xD800 }));
    }

    #[test]
    fn test_char16str_partial_eq_ref_str() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        let other: &str = "EFI";
        assert!(s == other);
        assert!(s != "NOPE");
    }

    #[test]
    fn test_char16str_hash() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let units = [0x0045u16, 0x0000];
        let s = Char16Str::from_units_with_nul(&units).unwrap();
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        let _ = hasher.finish();
    }

    #[test]
    fn test_char16string_debug_borrow_and_display() {
        use core::borrow::Borrow;
        let s = Char16String::try_from_str("EFI").unwrap();
        assert_eq!(std::format!("{s:?}"), "\"EFI\"");
        assert_eq!(s.to_string(), "EFI");
        let borrowed: &Char16Str = s.borrow();
        assert_eq!(borrowed.len(), 3);
    }

    #[test]
    fn test_char16_from_units_until_nul_exact() {
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16Str::from_units_until_nul(&units).unwrap();
        assert!(s == "EFI");
    }

    #[test]
    fn test_char16_from_units_until_nul_trailing_padding() {
        // Extra trailing NUL padding after the terminator is ignored rather than rejected.
        let units = [0x0045u16, 0x0046, 0x0049, 0x0000, 0x0000, 0x0000];
        let s = Char16Str::from_units_until_nul(&units).unwrap();
        assert_eq!(s.len(), 3);
        assert!(s == "EFI");
    }

    #[test]
    fn test_char16_from_units_until_nul_trailing_garbage() {
        // Anything after the first NUL is ignored, matching core::ffi::CStr::from_bytes_until_nul.
        let units = [0x0045u16, 0x0000, 0x1234];
        let s = Char16Str::from_units_until_nul(&units).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s == "E");
    }

    #[test]
    fn test_char16_from_units_until_nul_missing_nul() {
        let units = [0x0045u16, 0x0046];
        assert_eq!(Char16Str::from_units_until_nul(&units), Err(StringError::MissingNulTerminator));
        assert_eq!(Char16Str::from_units_until_nul(&[]), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char16_from_units_until_nul_surrogate_rejected() {
        let units = [0x0045u16, 0xD800, 0x0000];
        assert_eq!(Char16Str::from_units_until_nul(&units), Err(StringError::NotUcs2 { position: 1, value: 0xD800 }));
    }

    #[test]
    fn test_char16string_from_units_with_nul() {
        let units = std::vec![0x0045u16, 0x0046, 0x0049, 0x0000];
        let s = Char16String::from_units_with_nul(units).unwrap();
        assert_eq!(s.len(), 3);
        assert!(*s == *"EFI");
    }

    #[test]
    fn test_char16string_from_units_with_nul_missing_terminator() {
        let units = std::vec![0x0045u16, 0x0046];
        assert_eq!(Char16String::from_units_with_nul(units), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char16string_from_units_with_nul_interior_nul() {
        let units = std::vec![0x0045u16, 0x0000, 0x0049, 0x0000];
        assert_eq!(Char16String::from_units_with_nul(units), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char16string_from_units_with_nul_surrogate_rejected() {
        let units = std::vec![0x0045u16, 0xD800, 0x0000];
        assert_eq!(Char16String::from_units_with_nul(units), Err(StringError::NotUcs2 { position: 1, value: 0xD800 }));
    }

    #[test]
    fn test_char16string_from_units_with_nul_unchecked() {
        let units = std::vec![0x0045u16, 0x0046, 0x0049, 0x0000];
        // SAFETY: `units` is a valid NUL-terminated UCS-2 sequence.
        let s = unsafe { Char16String::from_units_with_nul_unchecked(units) };
        assert!(*s == *"EFI");
    }

    #[test]
    fn test_char16_array_from_str_exact_fit() {
        const NAME: Char16Array<4> = Char16Array::from_str("EFI");
        assert_eq!(NAME.as_char16_str().len(), 3);
        assert!(NAME.as_char16_str() == "EFI");
        assert_eq!(NAME.to_string(), "EFI");
    }

    #[test]
    fn test_char16_array_with_padding() {
        let array = Char16Array::<8>::from_str("EFI");
        assert_eq!(array.as_char16_str().len(), 3);
        // The backing array is NUL-padded to its full capacity.
        assert_eq!(array.as_array(), &[0x45, 0x46, 0x49, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_char16_array_too_long() {
        assert_eq!(Char16Array::<3>::try_from_str("EFI"), Err(StringError::TooLong { capacity: 2, required: 4 }));
    }

    #[test]
    fn test_char16_array_rejects_non_bmp() {
        assert_eq!(
            Char16Array::<8>::try_from_str("A\u{1F600}"),
            Err(StringError::NotUcs2 { position: 1, value: 0x1F600 })
        );
    }

    #[test]
    fn test_char16_array_zero_capacity() {
        assert_eq!(Char16Array::<0>::try_from_str(""), Err(StringError::TooLong { capacity: 0, required: 1 }));
    }

    #[test]
    fn test_char16_macro() {
        let name = char16!("Firmware");
        assert_eq!(name.len(), 8);
        assert!(name == "Firmware");
    }

    #[test]
    fn test_char16_macro_multibyte_utf8() {
        // "café" is 4 characters but 5 UTF-8 bytes; capacity must be counted in characters.
        let name = char16!("café");
        assert_eq!(name.len(), 4);
        assert!(name == "café");
    }

    #[test]
    fn test_char16_array_binary_layout() {
        assert_eq!(core::mem::size_of::<Char16Array<8>>(), 16);
    }

    #[test]
    fn test_char16_array_runtime_multibyte_decode() {
        // Exercise the multi-byte UTF-8 decode branches at runtime (not just at const time).
        let two_byte = Char16Array::<8>::try_from_str("café").unwrap(); // 'é' is 2 UTF-8 bytes
        assert!(two_byte.as_char16_str() == "café");
        let three_byte = Char16Array::<8>::try_from_str("€").unwrap(); // '€' is 3 UTF-8 bytes
        assert_eq!(three_byte.as_char16_str().len(), 1);
        // A 4-byte sequence decodes to a non-BMP code point, which is rejected as non-UCS-2.
        assert!(matches!(Char16Array::<8>::try_from_str("\u{1F600}"), Err(StringError::NotUcs2 { .. })));
    }

    #[test]
    fn test_char16_capacity_helper_runtime() {
        assert_eq!(ucs2_capacity("EFI"), 4);
        assert_eq!(ucs2_capacity(""), 1);
    }

    #[test]
    fn test_char16_array_try_from_str_trait() {
        let array = Char16Array::<9>::try_from("Firmware").unwrap();
        assert!(array.as_char16_str() == "Firmware");
    }

    #[test]
    fn test_char16_array_from_char8_array() {
        let narrow = Char8Array::<9>::from_str("Firmware");
        let wide: Char16Array<9> = narrow.into();
        assert!(wide.as_char16_str() == "Firmware");
    }

    #[test]
    fn test_char16_array_from_char8_array_high_latin1() {
        let narrow = Char8Array::<2>::from_str("\u{00E9}"); // 'é'
        let wide: Char16Array<2> = narrow.into();
        assert!(wide.as_char16_str() == "\u{00E9}");
    }

    #[test]
    fn test_char16_array_from_char8_array_empty_capacity() {
        let narrow = Char8Array::<0>::default();
        let wide: Char16Array<0> = narrow.into();
        assert!(wide.as_char16_str().is_empty());
    }

    #[test]
    fn test_char16_array_to_char16string() {
        let array = Char16Array::<9>::from_str("Firmware");
        let owned: Char16String = array.into();
        assert!(owned == "Firmware");
    }

    #[test]
    fn test_char16_array_to_string() {
        let array = Char16Array::<9>::from_str("Firmware");
        let owned: std::string::String = array.into();
        assert_eq!(owned, "Firmware");
    }

    #[test]
    fn test_char16string_from_char8str() {
        let narrow = Char8Array::<4>::from_str("EFI");
        let wide = Char16String::from(narrow.as_char8_str());
        assert!(wide == "EFI");
    }

    #[test]
    fn test_char16string_from_char8string() {
        let narrow = Char8String::try_from_str("EFI").unwrap();
        let wide = Char16String::from(&narrow);
        assert!(wide == "EFI");
        let wide_owned = Char16String::from(narrow);
        assert!(wide_owned == "EFI");
    }
}
