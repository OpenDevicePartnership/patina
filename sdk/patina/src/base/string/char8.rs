//! CHAR8 (ASCII / ISO-Latin-1) string types
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

use super::char16::Char16Array;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

#[cfg(any(test, feature = "alloc"))]
use super::char16::{Char16Str, Char16String};

/// A borrowed, NUL-terminated CHAR8 (ASCII / ISO-Latin-1) string.
///
/// This is the CHAR8 analogue of [`core::ffi::CStr`]. It is an unsized type that is always accessed
/// behind a reference (`&Char8Str`). The referenced data is guaranteed to:
///
/// - end with a single NUL (`0x00`) terminator, and
/// - contain no interior NUL.
///
/// Every byte in `0x01..=0xFF` is a valid ISO-Latin-1 (ISO-8859-1) character, mapping directly onto
/// the Unicode scalar values `U+0001..=U+00FF`. Because the terminator is guaranteed,
/// [`Char8Str::as_ptr`] can be passed directly to `extern "efiapi"` interfaces expecting a
/// `*const CHAR8`.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char8Str;
///
/// let bytes = *b"EFI\0";
/// let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
/// assert_eq!(s.len(), 3);
/// assert!(s == "EFI");
/// ```
#[repr(transparent)]
pub struct Char8Str([u8]);

impl Char8Str {
    /// An empty, NUL-terminated CHAR8 string.
    // SAFETY: `&[0]` is a single NUL byte that is trivially NUL-terminated with no interior NUL.
    pub const EMPTY: &'static Char8Str = unsafe { Self::from_bytes_with_nul_unchecked(&[0]) };

    /// Creates a `&Char8Str` from a slice of bytes that ends with a NUL terminator.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `bytes` does not end with a NUL, or
    /// [`StringError::InteriorNul`] if a NUL appears before the end.
    pub fn from_bytes_with_nul(bytes: &[u8]) -> Result<&Char8Str, StringError> {
        let body = match bytes.split_last() {
            Some((0, body)) => body,
            _ => return Err(StringError::MissingNulTerminator),
        };

        for (position, &byte) in body.iter().enumerate() {
            if byte == 0 {
                return Err(StringError::InteriorNul { position });
            }
        }

        // SAFETY: The loop above verified that `bytes` is NUL-terminated with no interior NUL, which
        // are exactly the invariants of `Char8Str`. Every remaining byte is valid Latin-1.
        Ok(unsafe { Self::from_bytes_with_nul_unchecked(bytes) })
    }

    /// Creates a `&Char8Str` from a NUL-terminated slice without validating its contents.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `bytes` ends with a single NUL terminator and contains no interior
    /// NUL.
    pub const unsafe fn from_bytes_with_nul_unchecked(bytes: &[u8]) -> &Char8Str {
        // SAFETY: `Char8Str` is `#[repr(transparent)]` over `[u8]`, so `&[u8]` and `&Char8Str` share
        // the same layout. The caller upholds the value invariants.
        unsafe { &*(core::ptr::from_ref::<[u8]>(bytes) as *const Char8Str) }
    }

    /// Creates a `&Char8Str` from the bytes up to and including the first NUL, ignoring anything
    /// after it.
    ///
    /// This mirrors [`core::ffi::CStr::from_bytes_until_nul`]. It is useful for fixed-size or
    /// over-allocated buffers that may carry trailing NUL padding (or other data) after the logical
    /// end of the string, where [`Char8Str::from_bytes_with_nul`] would reject anything after the
    /// first NUL as an interior NUL.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `bytes` contains no NUL at all.
    // `end` is a valid index into `bytes` because it came from `position`, so the slice can't panic.
    #[allow(clippy::indexing_slicing)]
    pub fn from_bytes_until_nul(bytes: &[u8]) -> Result<&Char8Str, StringError> {
        let end = bytes.iter().position(|&byte| byte == 0).ok_or(StringError::MissingNulTerminator)?;
        Self::from_bytes_with_nul(&bytes[..=end])
    }

    /// Creates a `&Char8Str` by scanning a raw pointer for a NUL terminator.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// - `ptr` is non-null and aligned, even for a zero-length string.
    /// - The memory pointed to by `ptr` contains a sequence of `u8` values that is valid for
    ///   reads up to and including a NUL terminator.
    ///   - The entire memory range of this `Char8Str`, from `ptr` through the terminator, must be
    ///     contained within a single allocation.
    ///   - The entire memory range must remain valid and unmodified for the lifetime `'a`.
    ///   - The NUL terminator must be within `isize::MAX` bytes of `ptr`.
    pub unsafe fn from_ptr<'a>(ptr: *const u8) -> &'a Char8Str {
        let mut len = 0usize;
        // SAFETY: The caller guarantees `ptr` points to a NUL-terminated sequence valid for reads.
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        // SAFETY: `len + 1` bytes up to and including the terminator are valid for reads.
        let bytes = unsafe { core::slice::from_raw_parts(ptr, len + 1) };
        // SAFETY: The scan guarantees a single trailing NUL with no interior NUL.
        unsafe { Self::from_bytes_with_nul_unchecked(bytes) }
    }

    /// Creates a `&Char8Str` by scanning at most `max_bytes` bytes for a NUL terminator.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// - `ptr` is non-null and aligned, even for a zero-length string.
    /// - `max_units * size_of::<u8>()` must not exceed `isize::MAX` bytes.
    /// - The memory pointed to by `ptr` contains a sequence of `u8` values that is valid for
    ///   reads up to and including a NUL terminator.
    ///   - The entire memory range of this `Char8Str`, from `ptr` through the terminator, must be
    ///     contained within a single allocation.
    ///   - The entire memory range must remain valid and unmodified for the lifetime `'a`.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if no NUL is found within `max_bytes` bytes.
    pub unsafe fn from_ptr_bounded<'a>(ptr: *const u8, max_bytes: usize) -> Result<&'a Char8Str, StringError> {
        let mut len = 0usize;
        // SAFETY: The caller guarantees `max_bytes` bytes are valid for reads.
        while len < max_bytes && unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        if len == max_bytes {
            return Err(StringError::MissingNulTerminator);
        }
        // SAFETY: `len + 1 <= max_bytes` bytes up to and including the terminator are valid.
        let bytes = unsafe { core::slice::from_raw_parts(ptr, len + 1) };
        // SAFETY: The scan guarantees a single trailing NUL with no interior NUL.
        Ok(unsafe { Self::from_bytes_with_nul_unchecked(bytes) })
    }

    /// Returns a pointer to the first byte, suitable for passing to `extern "efiapi"` interfaces.
    ///
    /// The pointed-to sequence is always NUL-terminated.
    pub const fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }

    /// Returns the bytes including the trailing NUL terminator.
    pub const fn as_bytes_with_nul(&self) -> &[u8] {
        &self.0
    }

    /// Returns the bytes excluding the trailing NUL terminator.
    pub fn as_bytes(&self) -> &[u8] {
        match self.0.split_last() {
            Some((_, body)) => body,
            None => &[],
        }
    }

    /// Returns the number of characters, excluding the trailing NUL terminator.
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Returns `true` if the string contains no characters (only the NUL terminator).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over the bytes, excluding the terminator.
    ///
    /// This borrows each byte rather than copying it. Call [`Iterator::copied`] on the result if
    /// owned `u8` values are needed.
    pub fn iter(&self) -> core::slice::Iter<'_, u8> {
        self.as_bytes().iter()
    }

    /// Returns an iterator over the characters as [`char`] values.
    ///
    /// Each byte maps directly onto its ISO-Latin-1 Unicode scalar value, so this conversion is always
    /// lossless.
    pub fn chars(&self) -> impl Iterator<Item = char> + '_ {
        self.iter().map(|&byte| byte as char)
    }
}

impl PartialEq for Char8Str {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Char8Str {}

impl PartialOrd for Char8Str {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Char8Str {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl core::hash::Hash for Char8Str {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq<str> for Char8Str {
    fn eq(&self, other: &str) -> bool {
        self.chars().eq(other.chars())
    }
}

impl PartialEq<&str> for Char8Str {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<Char8Str> for str {
    fn eq(&self, other: &Char8Str) -> bool {
        other == self
    }
}

impl core::fmt::Display for Char8Str {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in self.chars() {
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for Char8Str {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("\"")?;
        for c in self.chars() {
            write!(f, "{}", c.escape_debug())?;
        }
        f.write_str("\"")
    }
}

/// An owned, heap-backed, NUL-terminated CHAR8 (ASCII / ISO-Latin-1) string.
///
/// This is the CHAR8 analogue of [`alloc::string::String`]. It dereferences to [`Char8Str`], so all
/// borrowed operations are available on an owned value.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char8String;
///
/// let s = Char8String::try_from_str("Firmware").unwrap();
/// assert_eq!(s.len(), 8);
/// assert!(s == "Firmware");
/// ```
#[cfg(any(test, feature = "alloc"))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Char8String(Vec<u8>);

#[cfg(any(test, feature = "alloc"))]
impl Char8String {
    /// Creates an empty string containing only the NUL terminator.
    pub fn new() -> Self {
        Self(alloc::vec![0])
    }

    /// Creates a `Char8String` by encoding a Rust [`str`] as Latin-1.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::InteriorNul`] if the input contains a NUL character, or
    /// [`StringError::NotLatin1`] if it contains a code point above `U+00FF`.
    pub fn try_from_str(s: &str) -> Result<Self, StringError> {
        let mut bytes = Vec::with_capacity(s.len() + 1);
        for (position, c) in s.chars().enumerate() {
            let value = c as u32;
            if value == 0 {
                return Err(StringError::InteriorNul { position });
            }
            if value > 0xFF {
                return Err(StringError::NotLatin1 { position, value });
            }
            bytes.push(value as u8);
        }
        bytes.push(0);
        Ok(Self(bytes))
    }

    /// Creates a `Char8String` from a `Vec<u8>` that ends with a NUL terminator.
    ///
    /// This validates `bytes` the same way [`Char8Str::from_bytes_with_nul`] does, then takes
    /// ownership of the buffer directly instead of cloning it.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::MissingNulTerminator`] if `bytes` does not end with a NUL, or
    /// [`StringError::InteriorNul`] if a NUL appears before the end.
    pub fn from_bytes_with_nul(bytes: Vec<u8>) -> Result<Self, StringError> {
        Char8Str::from_bytes_with_nul(&bytes)?;
        Ok(Self(bytes))
    }

    /// Creates a `Char8String` from a NUL-terminated `Vec<u8>` without validating its contents.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `bytes` ends with a single NUL terminator and contains no interior
    /// NUL.
    pub unsafe fn from_bytes_with_nul_unchecked(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Borrows this owned string as a [`Char8Str`].
    pub fn as_char8_str(&self) -> &Char8Str {
        // SAFETY: The type invariant guarantees `self.0` is a valid NUL-terminated Latin-1 sequence.
        unsafe { Char8Str::from_bytes_with_nul_unchecked(&self.0) }
    }

    /// Consumes the string and returns the backing bytes, including the trailing NUL.
    pub fn into_bytes_with_nul(self) -> Vec<u8> {
        self.0
    }
}

#[cfg(any(test, feature = "alloc"))]
impl Default for Char8String {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::ops::Deref for Char8String {
    type Target = Char8Str;

    fn deref(&self) -> &Char8Str {
        self.as_char8_str()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::borrow::Borrow<Char8Str> for Char8String {
    fn borrow(&self) -> &Char8Str {
        self.as_char8_str()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl alloc::borrow::ToOwned for Char8Str {
    type Owned = Char8String;

    fn to_owned(&self) -> Char8String {
        Char8String(self.0.to_vec())
    }
}

#[cfg(any(test, feature = "alloc"))]
impl From<&Char8Str> for Char8String {
    fn from(s: &Char8Str) -> Self {
        use alloc::borrow::ToOwned;
        s.to_owned()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<'a> TryFrom<&'a str> for Char8String {
    type Error = StringError;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Self::try_from_str(s)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl TryFrom<&Char16Str> for Char8String {
    type Error = StringError;

    /// Converts UCS-2 to Latin-1.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::NotLatin1`] if any code unit is above `0xFF`.
    fn try_from(value: &Char16Str) -> Result<Self, StringError> {
        let mut bytes = Vec::with_capacity(value.len() + 1);
        for (position, &unit) in value.iter().enumerate() {
            if unit > 0xFF {
                return Err(StringError::NotLatin1 { position, value: u32::from(unit) });
            }
            bytes.push(unit as u8);
        }
        bytes.push(0);
        Ok(Self(bytes))
    }
}

#[cfg(any(test, feature = "alloc"))]
impl TryFrom<&Char16String> for Char8String {
    type Error = StringError;

    fn try_from(value: &Char16String) -> Result<Self, StringError> {
        Self::try_from(value.as_char16_str())
    }
}

#[cfg(any(test, feature = "alloc"))]
impl TryFrom<Char16String> for Char8String {
    type Error = StringError;

    fn try_from(value: Char16String) -> Result<Self, StringError> {
        Self::try_from(&value)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::fmt::Display for Char8String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(self.as_char8_str(), f)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl core::fmt::Debug for Char8String {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_char8_str(), f)
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<str> for Char8String {
    fn eq(&self, other: &str) -> bool {
        self.as_char8_str() == other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<&str> for Char8String {
    fn eq(&self, other: &&str) -> bool {
        self.as_char8_str() == *other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char8String> for str {
    fn eq(&self, other: &Char8String) -> bool {
        other.as_char8_str() == self
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char8Str> for Char8String {
    fn eq(&self, other: &Char8Str) -> bool {
        self.as_char8_str() == other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<&Char8Str> for Char8String {
    fn eq(&self, other: &&Char8Str) -> bool {
        self.as_char8_str() == *other
    }
}

#[cfg(any(test, feature = "alloc"))]
impl PartialEq<Char8String> for Char8Str {
    fn eq(&self, other: &Char8String) -> bool {
        self == other.as_char8_str()
    }
}

/// Returns the [`Char8Array`] capacity needed to hold `s` plus a NUL terminator.
///
/// This is an implementation detail of the [`char8!`](crate::char8) macro.
#[doc(hidden)]
pub const fn latin1_capacity(s: &str) -> usize {
    // Every accepted Latin-1 character occupies a single byte, plus one for the terminator.
    char_count(s) + 1
}

/// A fixed capacity, NUL-terminated CHAR8 (ASCII / ISO-Latin-1) string of `N` bytes.
///
/// The capacity `N` includes the trailing NUL terminator, so the array can hold up to `N - 1`
/// characters. Unused trailing slots are filled with NUL. This type is `#[repr(transparent)]` over
/// `[u8; N]`, making it suitable for `#[repr(C)]` structure fields.
///
/// Values can be built in `const` contexts via [`Char8Array::from_str`] and
/// [`Char8Array::try_from_str`], which is how the [`char8!`](crate::char8) macro validates literals at
/// compile time.
///
/// This type also implements [`zerocopy::FromBytes`] and [`zerocopy::IntoBytes`], so it can be used
/// directly as a field of a `#[repr(C)]`/`#[repr(C, packed)]` structure that derives those traits for
/// zero-copy parsing and serialization. Because `FromBytes` allows constructing a value from arbitrary
/// bytes, a value built that way is not guaranteed to contain a NUL terminator. In that case,
/// [`Char8Array::as_char8_str`] (and [`Deref`](core::ops::Deref)) return an empty string rather than
/// panicking.
///
/// # Examples
///
/// ```rust
/// use patina::string::Char8Array;
///
/// const NAME: Char8Array<4> = Char8Array::from_str("EFI");
/// assert_eq!(NAME.as_char8_str().len(), 3);
/// assert!(NAME.as_char8_str() == "EFI");
/// ```
#[repr(transparent)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Char8Array<const N: usize>([u8; N]);

impl<const N: usize> Char8Array<N> {
    /// Creates a `Char8Array` by encoding a Rust [`str`] as Latin-1.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::InteriorNul`] if the input contains a NUL character,
    /// [`StringError::NotLatin1`] if it contains a code point above `U+00FF`, or
    /// [`StringError::TooLong`] if the encoded string plus its NUL terminator does not fit in `N`
    /// bytes.
    // `out[out_index]` is guarded by the `out_index + 1 >= N` check, so `out_index < N`.
    #[allow(clippy::indexing_slicing)]
    pub const fn try_from_str(s: &str) -> Result<Self, StringError> {
        let bytes = s.as_bytes();
        let mut out = [0u8; N];
        let mut index = 0;
        let mut out_index = 0;
        let mut position = 0;

        while index < bytes.len() {
            let (value, len) = decode_utf8(bytes, index);
            if value == 0 {
                return Err(StringError::InteriorNul { position });
            }
            if value > 0xFF {
                return Err(StringError::NotLatin1 { position, value });
            }
            // Require room for this byte and a trailing NUL terminator.
            if out_index + 1 >= N {
                return Err(StringError::TooLong { capacity: N.saturating_sub(1), required: out_index + 2 });
            }
            out[out_index] = value as u8;
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

    /// Creates a `Char8Array`, panicking on invalid input.
    ///
    /// This is intended for compile-time string literals: because it is a `const fn`, an invalid
    /// literal in a `const`/`static` context (including through the [`char8!`](crate::char8) macro) is
    /// reported as a compile-time error, never a runtime panic. Runtime callers that cannot guarantee
    /// valid input should use [`Char8Array::try_from_str`] instead, which returns a [`Result`] rather
    /// than panicking.
    ///
    /// # Panics
    ///
    /// Panics if the input is not valid Latin-1 or does not fit in `N` bytes. In a `const` context
    /// this manifests as a compile-time error.
    pub const fn from_str(s: &str) -> Self {
        match Self::try_from_str(s) {
            Ok(array) => array,
            Err(_) => panic!("invalid Latin-1 string literal"),
        }
    }

    /// Borrows the populated portion of the array as a [`Char8Str`].
    ///
    /// Every value produced by a safe constructor (`try_from_str`, `from_str`, `Default`) contains a
    /// NUL terminator by construction. A value materialized from arbitrary bytes using `FromBytes` is
    /// not guaranteed to, so this returns [`Char8Str::EMPTY`] in that case rather than panicking.
    #[allow(clippy::indexing_slicing)]
    pub fn as_char8_str(&self) -> &Char8Str {
        let mut len = 0;
        while len < N && self.0[len] != 0 {
            len += 1;
        }
        if len == N {
            return Char8Str::EMPTY;
        }
        // SAFETY: `self.0[..=len]` ends at the first NUL (found above) and contains only valid Latin-1
        // bytes.
        unsafe { Char8Str::from_bytes_with_nul_unchecked(&self.0[..=len]) }
    }

    /// Returns the full backing array, including trailing NUL padding.
    pub const fn as_array(&self) -> &[u8; N] {
        &self.0
    }
}

impl<const N: usize> core::ops::Deref for Char8Array<N> {
    type Target = Char8Str;

    fn deref(&self) -> &Char8Str {
        self.as_char8_str()
    }
}

impl<const N: usize> core::fmt::Display for Char8Array<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(self.as_char8_str(), f)
    }
}

impl<const N: usize> Default for Char8Array<N> {
    /// Returns an empty string backed by an all-zero array.
    ///
    /// `N` must be at least `1` to hold the mandatory NUL terminator; as with
    /// [`Char8Array::try_from_str`], a capacity of `0` cannot represent a valid string.
    fn default() -> Self {
        Self([0; N])
    }
}

impl<'a, const N: usize> TryFrom<&'a str> for Char8Array<N> {
    type Error = StringError;

    /// Equivalent to [`Char8Array::try_from_str`], provided for generic code written against the
    /// standard [`TryFrom`] trait.
    fn try_from(s: &'a str) -> Result<Self, StringError> {
        Self::try_from_str(s)
    }
}

impl<const N: usize> TryFrom<Char16Array<N>> for Char8Array<N> {
    type Error = StringError;

    /// Converts UCS-2 to Latin-1.
    ///
    /// # Errors
    ///
    /// Returns [`StringError::NotLatin1`] if any code unit is above `0xFF`. This can never fail with
    /// [`StringError::TooLong`] since both arrays share the same capacity `N`, and a `Char16Array<N>`'s
    /// logical content is always at most `N - 1` code units.
    // Note: `out[position]` is safe because `position < src.len() < N`, except when `N == 0`, where `src`
    // is empty and the loop never runs.
    #[allow(clippy::indexing_slicing)]
    fn try_from(value: Char16Array<N>) -> Result<Self, StringError> {
        let src = value.as_char16_str();
        // Note: The terminator and any trailing padding are already present due to zero-initialization.
        let mut out = [0u8; N];
        for (position, &unit) in src.iter().enumerate() {
            if unit > 0xFF {
                return Err(StringError::NotLatin1 { position, value: u32::from(unit) });
            }
            out[position] = unit as u8;
        }
        Ok(Self(out))
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<const N: usize> From<Char8Array<N>> for Char8String {
    /// Converts a fixed-capacity array to a heap-owned string, dropping the const capacity `N`.
    fn from(value: Char8Array<N>) -> Self {
        use alloc::borrow::ToOwned;
        value.as_char8_str().to_owned()
    }
}

#[cfg(any(test, feature = "alloc"))]
impl<const N: usize> From<Char8Array<N>> for String {
    /// Converts to a native Rust string.
    ///
    /// This will always succeed since every Latin-1 character is a valid Unicode scalar value.
    fn from(value: Char8Array<N>) -> Self {
        value.as_char8_str().chars().collect()
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::char8;
    use alloc::string::ToString;

    #[test]
    fn test_char8_from_bytes_with_nul_valid() {
        let bytes = *b"EFI\0";
        let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert_eq!(s.as_bytes(), b"EFI");
        assert_eq!(s.as_bytes_with_nul(), b"EFI\0");
    }

    #[test]
    fn test_char8_empty_string() {
        let bytes = [0u8];
        let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
        assert_eq!(s.len(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn test_char8_missing_nul_terminator() {
        assert_eq!(Char8Str::from_bytes_with_nul(b"EFI"), Err(StringError::MissingNulTerminator));
        assert_eq!(Char8Str::from_bytes_with_nul(b""), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char8_interior_nul() {
        let bytes = *b"E\0I\0";
        assert_eq!(Char8Str::from_bytes_with_nul(&bytes), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char8_high_latin1_bytes_valid() {
        let bytes = [0xE9u8, 0xFF, 0x00]; // "éÿ"
        let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
        let chars: std::vec::Vec<char> = s.chars().collect();
        assert_eq!(chars, ['\u{00E9}', '\u{00FF}']);
    }

    #[test]
    fn test_char8_iter() {
        let bytes = *b"EFI\0";
        let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
        let collected: std::vec::Vec<u8> = s.iter().copied().collect();
        assert_eq!(collected, b"EFI");
    }

    #[test]
    fn test_char8_from_ptr() {
        let bytes = *b"EFI\0";
        // SAFETY: `bytes` is a valid NUL-terminated buffer that outlives the borrow.
        let s = unsafe { Char8Str::from_ptr(bytes.as_ptr()) };
        assert!(s == "EFI");
    }

    #[test]
    fn test_char8_from_ptr_bounded_finds_nul() {
        let bytes = *b"EF\0I";
        // SAFETY: `bytes` has at least 4 readable bytes.
        let s = unsafe { Char8Str::from_ptr_bounded(bytes.as_ptr(), 4) }.unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn test_char8_from_ptr_bounded_no_nul() {
        let bytes = *b"EFI";
        // SAFETY: `bytes` has at least 3 readable bytes.
        let result = unsafe { Char8Str::from_ptr_bounded(bytes.as_ptr(), 3) };
        assert_eq!(result, Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char8_display_and_debug() {
        let bytes = *b"EFI\0";
        let s = Char8Str::from_bytes_with_nul(&bytes).unwrap();
        assert_eq!(s.to_string(), "EFI");
        assert_eq!(std::format!("{s:?}"), "\"EFI\"");
    }

    #[test]
    fn test_char8_equality_and_ordering() {
        let a = Char8Str::from_bytes_with_nul(b"A\0").unwrap();
        let b = Char8Str::from_bytes_with_nul(b"B\0").unwrap();
        assert!(a < b);
        assert_ne!(a, b);
        assert!(a == "A");
        assert!(*"A" == *a);
    }

    #[test]
    fn test_char8string_try_from_str() {
        let s = Char8String::try_from_str("Firmware").unwrap();
        assert_eq!(s.len(), 8);
        assert!(*s == *"Firmware");
        assert_eq!(s.to_string(), "Firmware");
    }

    #[test]
    fn test_char8string_partial_eq_str() {
        let s = Char8String::try_from_str("EFI").unwrap();
        assert!(s == "EFI");
        assert!(s == *"EFI");
        assert!(*"EFI" == s);
        assert!(s != "NOPE");
        assert!(*"NOPE" != s);
    }

    #[test]
    fn test_char8string_partial_eq_char8str() {
        let s = Char8String::try_from_str("EFI").unwrap();
        let same = Char8String::try_from_str("EFI").unwrap();
        let different = Char8String::try_from_str("NOPE").unwrap();
        let same_str: &Char8Str = same.as_char8_str();
        let different_str: &Char8Str = different.as_char8_str();
        assert!(s == *same_str);
        assert!(s == same_str);
        assert!(*same_str == s);
        assert!(s != *different_str);
        assert!(*different_str != s);
    }

    #[test]
    fn test_char8string_latin1_boundary() {
        // U+00FF is the last valid Latin-1 code point; U+0100 is the first invalid one.
        assert!(Char8String::try_from_str("\u{00FF}").is_ok());
        assert_eq!(Char8String::try_from_str("A\u{0100}"), Err(StringError::NotLatin1 { position: 1, value: 0x0100 }));
    }

    #[test]
    fn test_char8string_rejects_interior_nul() {
        assert_eq!(Char8String::try_from_str("A\0B"), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char8string_new_and_default() {
        let a = Char8String::new();
        let b = Char8String::default();
        assert!(a.is_empty());
        assert_eq!(a, b);
        assert_eq!(a.clone().into_bytes_with_nul(), std::vec![0]);
    }

    #[test]
    fn test_char8string_to_owned_roundtrip() {
        use std::borrow::ToOwned;
        let owned = Char8String::try_from_str("Test").unwrap();
        let borrowed: &Char8Str = &owned;
        let reowned = borrowed.to_owned();
        assert_eq!(owned, reowned);
    }

    #[test]
    fn test_char8string_from_ref_char8str() {
        let owned = Char8String::try_from_str("Test").unwrap();
        let borrowed: &Char8Str = &owned;
        let converted = Char8String::from(borrowed);
        assert_eq!(owned, converted);
        let via_into: Char8String = borrowed.into();
        assert_eq!(owned, via_into);
    }

    #[test]
    fn test_char8str_partial_eq_ref_str() {
        let s = Char8Str::from_bytes_with_nul(b"EFI\0").unwrap();
        let other: &str = "EFI";
        assert!(s == other);
        assert!(s != "NOPE");
    }

    #[test]
    fn test_char8str_hash() {
        use core::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let s = Char8Str::from_bytes_with_nul(b"E\0").unwrap();
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        let _ = hasher.finish();
    }

    #[test]
    fn test_char8string_debug_borrow_and_display() {
        use core::borrow::Borrow;
        let s = Char8String::try_from_str("EFI").unwrap();
        assert_eq!(std::format!("{s:?}"), "\"EFI\"");
        assert_eq!(s.to_string(), "EFI");
        let borrowed: &Char8Str = s.borrow();
        assert_eq!(borrowed.len(), 3);
    }

    #[test]
    fn test_char8_from_bytes_until_nul_exact() {
        let bytes = *b"EFI\0";
        let s = Char8Str::from_bytes_until_nul(&bytes).unwrap();
        assert!(s == "EFI");
    }

    #[test]
    fn test_char8_from_bytes_until_nul_trailing_padding() {
        // Extra trailing NUL padding after the terminator is ignored rather than rejected.
        let bytes = *b"EFI\0\0\0";
        let s = Char8Str::from_bytes_until_nul(&bytes).unwrap();
        assert_eq!(s.len(), 3);
        assert!(s == "EFI");
    }

    #[test]
    fn test_char8_from_bytes_until_nul_trailing_garbage() {
        // Anything after the first NUL is ignored, matching core::ffi::CStr::from_bytes_until_nul.
        let bytes = [b'E', 0, 0xFF];
        let s = Char8Str::from_bytes_until_nul(&bytes).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s == "E");
    }

    #[test]
    fn test_char8_from_bytes_until_nul_missing_nul() {
        assert_eq!(Char8Str::from_bytes_until_nul(b"EFI"), Err(StringError::MissingNulTerminator));
        assert_eq!(Char8Str::from_bytes_until_nul(b""), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char8string_from_bytes_with_nul() {
        let bytes = std::vec![b'E', b'F', b'I', 0];
        let s = Char8String::from_bytes_with_nul(bytes).unwrap();
        assert_eq!(s.len(), 3);
        assert!(*s == *"EFI");
    }

    #[test]
    fn test_char8string_from_bytes_with_nul_missing_terminator() {
        let bytes = std::vec![b'E', b'F', b'I'];
        assert_eq!(Char8String::from_bytes_with_nul(bytes), Err(StringError::MissingNulTerminator));
    }

    #[test]
    fn test_char8string_from_bytes_with_nul_interior_nul() {
        let bytes = std::vec![b'E', 0, b'I', 0];
        assert_eq!(Char8String::from_bytes_with_nul(bytes), Err(StringError::InteriorNul { position: 1 }));
    }

    #[test]
    fn test_char8string_from_bytes_with_nul_unchecked() {
        let bytes = std::vec![b'E', b'F', b'I', 0];
        // SAFETY: `bytes` is a valid NUL-terminated Latin-1 sequence.
        let s = unsafe { Char8String::from_bytes_with_nul_unchecked(bytes) };
        assert!(*s == *"EFI");
    }

    #[test]
    fn test_char8_array_from_str_exact_fit() {
        const NAME: Char8Array<4> = Char8Array::from_str("EFI");
        assert_eq!(NAME.as_char8_str().len(), 3);
        assert!(NAME.as_char8_str() == "EFI");
        assert_eq!(NAME.to_string(), "EFI");
    }

    #[test]
    fn test_char8_array_with_padding() {
        let array = Char8Array::<8>::from_str("EFI");
        assert_eq!(array.as_char8_str().len(), 3);
        assert_eq!(array.as_array(), b"EFI\0\0\0\0\0");
    }

    #[test]
    fn test_char8_array_too_long() {
        assert_eq!(Char8Array::<3>::try_from_str("EFI"), Err(StringError::TooLong { capacity: 2, required: 4 }));
    }

    #[test]
    fn test_char8_array_rejects_non_latin1() {
        assert_eq!(
            Char8Array::<8>::try_from_str("A\u{0100}"),
            Err(StringError::NotLatin1 { position: 1, value: 0x0100 })
        );
    }

    #[test]
    fn test_char8_macro() {
        let name = char8!("Firmware");
        assert_eq!(name.len(), 8);
        assert!(name == "Firmware");
    }

    #[test]
    fn test_char8_array_binary_layout() {
        assert_eq!(core::mem::size_of::<Char8Array<8>>(), 8);
    }

    #[test]
    fn test_char8_array_runtime_multibyte_decode() {
        let latin1 = Char8Array::<8>::try_from_str("café").unwrap();
        assert!(latin1.as_char8_str() == "café");
        // '€' (U+20AC) exceeds Latin-1 and is rejected.
        assert!(matches!(Char8Array::<8>::try_from_str("€"), Err(StringError::NotLatin1 { .. })));
    }

    #[test]
    fn test_char8_capacity_helper_runtime() {
        assert_eq!(latin1_capacity("café"), 5);
    }

    #[test]
    fn test_char8_array_try_from_str_trait() {
        let array = Char8Array::<9>::try_from("Firmware").unwrap();
        assert!(array.as_char8_str() == "Firmware");
    }

    #[test]
    fn test_char8_array_from_char16_array() {
        let wide = Char16Array::<9>::from_str("Firmware");
        let narrow = Char8Array::<9>::try_from(wide).unwrap();
        assert!(narrow.as_char8_str() == "Firmware");
    }

    #[test]
    fn test_char8_array_from_char16_array_high_latin1() {
        let wide = Char16Array::<2>::from_str("\u{00E9}"); // 'é'
        let narrow = Char8Array::<2>::try_from(wide).unwrap();
        assert!(narrow.as_char8_str() == "\u{00E9}");
    }

    #[test]
    fn test_char8_array_from_char16_array_not_latin1() {
        let wide = Char16Array::<4>::from_str("A\u{20AC}"); // '€' (U+20AC) exceeds Latin-1
        assert_eq!(Char8Array::<4>::try_from(wide), Err(StringError::NotLatin1 { position: 1, value: 0x20AC }));
    }

    #[test]
    fn test_char8_array_from_char16_array_empty_capacity() {
        let wide = Char16Array::<0>::default();
        let narrow = Char8Array::<0>::try_from(wide).unwrap();
        assert!(narrow.as_char8_str().is_empty());
    }

    #[test]
    fn test_char8_array_to_char8string() {
        let array = Char8Array::<9>::from_str("Firmware");
        let owned: Char8String = array.into();
        assert!(owned == "Firmware");
    }

    #[test]
    fn test_char8_array_to_string() {
        let array = Char8Array::<9>::from_str("Firmware");
        let owned: std::string::String = array.into();
        assert_eq!(owned, "Firmware");
    }

    #[test]
    fn test_char8string_try_from_char16str() {
        let wide = Char16Array::<4>::from_str("EFI");
        let narrow = Char8String::try_from(wide.as_char16_str()).unwrap();
        assert!(narrow == "EFI");
    }

    #[test]
    fn test_char8string_try_from_char16string() {
        let wide = Char16String::try_from_str("EFI").unwrap();
        let narrow = Char8String::try_from(&wide).unwrap();
        assert!(narrow == "EFI");
        let narrow_owned = Char8String::try_from(wide).unwrap();
        assert!(narrow_owned == "EFI");
    }

    #[test]
    fn test_char8string_try_from_char16string_not_latin1() {
        let wide = Char16String::try_from_str("A\u{20AC}").unwrap();
        assert_eq!(Char8String::try_from(&wide), Err(StringError::NotLatin1 { position: 1, value: 0x20AC }));
    }
}
