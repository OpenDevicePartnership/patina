//! Toolchain-stable rendering of [`core::any::type_name`] output.
//!
//! The exact string produced by [`core::any::type_name`] is documented as a
//! best-effort description that may change between compiler versions. In
//! particular, the formatter's handling of anonymous lifetime parameters
//! (e.g. `SomeType<'_, T>` vs. `SomeType<T>`) has varied across rustc releases. The
//! helpers in this module normalize the rendering so component diagnostics
//! (and the tests that assert against them) compare equal regardless of which
//! toolchain produced them.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use alloc::string::String;

/// Returns the [`core::any::type_name`] of `T` with anonymous lifetime
/// parameters elided so the result is stable across rustc versions.
pub(crate) fn normalized<T: ?Sized>() -> String {
    normalize(core::any::type_name::<T>())
}

/// Removes anonymous lifetime tokens (`'_`) from a type-name string produced
/// by [`core::any::type_name`].
///
/// Handles the two forms the standard library has been observed to emit:
///   - `SomeType<'_, T>` -> `SomeType<T>`
///   - `SomeType<'_>`    -> `SomeType`
pub(crate) fn normalize(name: &str) -> String {
    name.replace("<'_, ", "<").replace("<'_>", "")
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_strips_leading_anonymous_lifetime() {
        assert_eq!(normalize("SomeType<'_, u32>"), "SomeType<u32>");
    }

    #[test]
    fn test_normalize_strips_sole_anonymous_lifetime() {
        assert_eq!(normalize("SomeType<'_>"), "SomeType");
    }

    #[test]
    fn test_normalize_is_noop_when_no_anonymous_lifetime_present() {
        assert_eq!(normalize("SomeType<u32>"), "SomeType<u32>");
        assert_eq!(normalize("path::to::AType"), "path::to::AType");
    }

    #[test]
    fn test_normalize_handles_nested_anonymous_lifetimes() {
        assert_eq!(normalize("Outer<Inner<'_, u8>, '_>"), "Outer<Inner<u8>, '_>");
        assert_eq!(normalize("Wrap<SomeType<'_>>"), "Wrap<SomeType>");
    }
}
