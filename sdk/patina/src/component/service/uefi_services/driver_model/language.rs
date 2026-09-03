//! Language matching for UEFI driver model name tables.
//!
//! [`LanguageTable`] resolves a requested UEFI language code to a driver or controller name. This
//! allows `EFI_COMPONENT_NAME_PROTOCOL` (ISO 639-2) and `EFI_COMPONENT_NAME2_PROTOCOL` (RFC 4646)
//! callers to be served from one table.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::string::{Char8Str, Char16Str};

/// A single name paired with the language codes it is available under.
///
/// Most drivers use the same name for both protocols, so one entry usually covers both.
#[derive(Debug, Clone, Copy)]
pub struct LanguageEntry {
    /// The three letter ISO 639-2 code, matched by [`LanguageTable::lookup_v1`]. For example
    /// `b"eng"`.
    pub iso639: &'static [u8; 3],
    /// The RFC 4646 language tag, matched by [`LanguageTable::lookup_v2`]. For example `"en"`.
    pub rfc4646: &'static str,
    /// The name to return when this entry's language is requested.
    pub name: &'static Char16Str,
}

/// A table of names available in one or more languages.
///
/// Build one of these as a `static` and return it from a
/// [`UefiDriverModelComponentName`](super::component_name::UefiDriverModelComponentName)
/// implementation.
///
/// # Examples
///
/// ```rust
/// use patina::char16;
/// use patina::component::service::uefi_services::driver_model::language::{LanguageEntry, LanguageTable};
///
/// static NAMES: LanguageTable =
///     LanguageTable(&[LanguageEntry { iso639: b"eng", rfc4646: "en", name: char16!("Sample Driver") }]);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LanguageTable(pub &'static [LanguageEntry]);

impl LanguageTable {
    /// Finds the name matching a three letter ISO 639-2 language code.
    ///
    /// `EFI_COMPONENT_NAME_PROTOCOL` compares exactly three characters and does not require
    /// `language` to be NUL terminated, so callers should read it as a fixed three byte buffer
    /// rather than scanning for a terminator.
    pub fn lookup_v1(&self, language: [u8; 3]) -> Option<&'static Char16Str> {
        self.0.iter().find(|entry| *entry.iso639 == language).map(|entry| entry.name)
    }

    /// Finds the name matching an RFC 4646 language tag.
    ///
    /// Tries an exact, case-insensitive match first, then falls back to matching just the primary
    /// subtag (the part before the first `-`), described in RFC 4647 for "basic filtering". This is
    /// a practical subset of what is typically supported. It does not implement wildcard or multiple
    /// candidate language matching.
    pub fn lookup_v2(&self, language: &Char8Str) -> Option<&'static Char16Str> {
        let requested = language.as_bytes();

        if let Some(entry) = self.0.iter().find(|entry| entry.rfc4646.as_bytes().eq_ignore_ascii_case(requested)) {
            return Some(entry.name);
        }

        let requested_primary = primary_subtag(requested);
        self.0
            .iter()
            .find(|entry| primary_subtag(entry.rfc4646.as_bytes()).eq_ignore_ascii_case(requested_primary))
            .map(|entry| entry.name)
    }
}

/// Returns the portion of a language tag before its first `-`, or the whole tag if there is none.
fn primary_subtag(tag: &[u8]) -> &[u8] {
    tag.split(|&byte| byte == b'-').next().unwrap_or(tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::char16;

    // Note: Actual french language is not used since the string details don't matter for the
    // purposes of testing.
    static NAMES: LanguageTable = LanguageTable(&[
        LanguageEntry { iso639: b"eng", rfc4646: "en-US", name: char16!("Sample Driver") },
        LanguageEntry { iso639: b"fra", rfc4646: "fr", name: char16!("Alternate Driver") },
    ]);

    fn char8(s: &str) -> crate::string::Char8String {
        crate::string::Char8String::try_from_str(s).unwrap()
    }

    #[test]
    fn test_lookup_v1_exact_match() {
        assert_eq!(NAMES.lookup_v1(*b"eng"), Some(char16!("Sample Driver")));
        assert_eq!(NAMES.lookup_v1(*b"fra"), Some(char16!("Alternate Driver")));
    }

    #[test]
    fn test_lookup_v1_unsupported() {
        assert_eq!(NAMES.lookup_v1(*b"deu"), None);
    }

    #[test]
    fn test_lookup_v2_exact_match_case_insensitive() {
        assert_eq!(NAMES.lookup_v2(&char8("EN-US")), Some(char16!("Sample Driver")));
        assert_eq!(NAMES.lookup_v2(&char8("fr")), Some(char16!("Alternate Driver")));
    }

    #[test]
    fn test_lookup_v2_primary_subtag_fallback() {
        // "en-GB" has no exact entry, but falls back to the "en-US" entry's primary subtag "en".
        assert_eq!(NAMES.lookup_v2(&char8("en-GB")), Some(char16!("Sample Driver")));
    }

    #[test]
    fn test_lookup_v2_unsupported() {
        assert_eq!(NAMES.lookup_v2(&char8("de-DE")), None);
    }

    #[test]
    fn test_primary_subtag() {
        assert_eq!(primary_subtag(b"en-US"), b"en");
        assert_eq!(primary_subtag(b"en"), b"en");
    }
}
