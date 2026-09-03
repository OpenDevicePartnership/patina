//! PCD (Platform Configuration Database) services for Patina components.
//!
//! [`PcdServices`] exposes get/set access to Dynamic and `DynamicEx` PCDs, backed by the PI
//! `PCD_PROTOCOL` (see [`crate::pi::protocol::pcd`]). This trait does not itself locate the
//! protocol. It is implemented by a component that does (for example, the `patina_pcd` crate),
//! and consumed by other components through a [`Service<dyn
//! PcdServices>`](crate::component::service::Service) parameter. Only components with a direct
//! dependency on a specific PCD's value should use this service - PCDs are a platform
//! integration detail, not a general-purpose configuration mechanism. Patina does not and will
//! not directly provide a PCD database implementation, it simply offers Service-based access to
//! Dynamic PCDs to provide standardized, type-safe access to the platform's PCD database maintained
//! by an external C driver for components that must read or write PCDs as a pre-existing requirement
//! with other C-based code.
//!
//! ## Safety
//!
//! A PCD token number is an opaque value assigned by the platform's (C/non-Patina) build tooling.
//! The underlying protocol has no way to validate a token ahead of time, so every accessor on
//! [`PcdServices`] is considered `unsafe`. The caller must ensure `token` refers to a PCD that
//! is registered in the platform's PCD database and holds a value of the type the method reads
//! or writes. Supplying an unregistered token number, or a token space GUID that does not match
//! `token`, is undefined behavior in the underlying protocol implementation. [`PcdError::SizeMismatch`]
//! is returned when the PCD's registered size does not match the accessor used, which catches the
//! common mistake of reading a PCD as the wrong scalar type, but does not by itself prove `token`
//! refers to a real, currently-registered PCD.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use crate::BinaryGuid;
use crate::base::error::EfiError;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// Identifies a Dynamic or `DynamicEx` PCD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcdToken {
    /// A Dynamic PCD in the default token space, identified by token number alone.
    Dynamic(usize),
    /// A `DynamicEx` PCD, identified by its token space GUID and token number.
    DynamicEx(BinaryGuid, usize),
}

/// Errors returned by [`PcdServices`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcdError {
    /// The PCD's registered size did not match the size required by the accessor used.
    SizeMismatch {
        /// The size, in bytes, actually registered for this PCD.
        actual: usize,
        /// The size, in bytes, that the accessor used requires.
        expected: usize,
    },
    /// The value being set is larger than the PCD's maximum size.
    BufferTooLarge {
        /// The maximum size, in bytes, supported for this PCD.
        max_size: usize,
    },
    /// The PCD service could not find the requested token.
    NotFound,
    /// The value being set is incompatible with the PCD's existing definition.
    InvalidParameter,
    /// An unexpected internal error occurred.
    Internal,
}

impl core::fmt::Display for PcdError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PcdError::SizeMismatch { actual, expected } => {
                write!(f, "PCD size mismatch: The accessor requires {expected} byte(s), PCD is {actual} byte(s)")
            }
            PcdError::BufferTooLarge { max_size } => {
                write!(f, "value exceeds the PCD's maximum size of {max_size} byte(s)")
            }
            PcdError::NotFound => write!(f, "PCD token not found"),
            PcdError::InvalidParameter => write!(f, "value is incompatible with the PCD's existing definition"),
            PcdError::Internal => write!(f, "an unexpected internal error occurred"),
        }
    }
}

impl core::error::Error for PcdError {}

impl From<EfiError> for PcdError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::NotFound => PcdError::NotFound,
            EfiError::InvalidParameter => PcdError::InvalidParameter,
            _ => PcdError::Internal,
        }
    }
}

/// Get/set access to Dynamic and `DynamicEx` Platform Configuration Database (PCD) values.
///
/// See the [module documentation](self) for the safety contract that applies to every method.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait PcdServices {
    /// Returns the size, in bytes, of the value stored for `token`.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_size(&self, token: PcdToken) -> usize;

    /// Retrieves an 8-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::SizeMismatch`] if `token`'s registered size is not 1 byte.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_u8(&self, token: PcdToken) -> Result<u8, PcdError>;

    /// Retrieves a 16-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::SizeMismatch`] if `token`'s registered size is not 2 bytes.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_u16(&self, token: PcdToken) -> Result<u16, PcdError>;

    /// Retrieves a 32-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::SizeMismatch`] if `token`'s registered size is not 4 bytes.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_u32(&self, token: PcdToken) -> Result<u32, PcdError>;

    /// Retrieves a 64-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::SizeMismatch`] if `token`'s registered size is not 8 bytes.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_u64(&self, token: PcdToken) -> Result<u64, PcdError>;

    /// Retrieves a boolean PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::SizeMismatch`] if `token`'s registered size is not 1 byte.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_bool(&self, token: PcdToken) -> Result<bool, PcdError>;

    /// Retrieves a variable-length PCD value as an owned byte buffer.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn get_bytes(&self, token: PcdToken) -> Result<Vec<u8>, PcdError>;

    /// Sets an 8-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::InvalidParameter`] if `token`'s registered size is not 1 byte, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_u8(&self, token: PcdToken, value: u8) -> Result<(), PcdError>;

    /// Sets a 16-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::InvalidParameter`] if `token`'s registered size is not 2 bytes, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_u16(&self, token: PcdToken, value: u16) -> Result<(), PcdError>;

    /// Sets a 32-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::InvalidParameter`] if `token`'s registered size is not 4 bytes, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_u32(&self, token: PcdToken, value: u32) -> Result<(), PcdError>;

    /// Sets a 64-bit PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::InvalidParameter`] if `token`'s registered size is not 8 bytes, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_u64(&self, token: PcdToken, value: u64) -> Result<(), PcdError>;

    /// Sets a boolean PCD value.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::InvalidParameter`] if `token`'s registered size is not 1 byte, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_bool(&self, token: PcdToken, value: bool) -> Result<(), PcdError>;

    /// Sets a variable-length PCD value from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns [`PcdError::BufferTooLarge`] if `value` is larger than `token`'s maximum size, and
    /// [`PcdError::NotFound`] if `token` is not a registered PCD.
    ///
    /// # Safety
    ///
    /// `token` must be valid; see the [module documentation](self).
    unsafe fn set_bytes(&self, token: PcdToken, value: &[u8]) -> Result<(), PcdError>;
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn test_pcd_token_dynamic_and_dynamic_ex_are_distinct() {
        let guid = BinaryGuid::ZERO;
        assert_ne!(PcdToken::Dynamic(1), PcdToken::DynamicEx(guid, 1));
        assert_eq!(PcdToken::DynamicEx(guid, 1), PcdToken::DynamicEx(guid, 1));
    }

    #[test]
    fn test_pcd_error_display() {
        assert_eq!(
            format!("{}", PcdError::SizeMismatch { actual: 4, expected: 1 }),
            "PCD size mismatch: The accessor requires 1 byte(s), PCD is 4 byte(s)"
        );
        assert_eq!(
            format!("{}", PcdError::BufferTooLarge { max_size: 4 }),
            "value exceeds the PCD's maximum size of 4 byte(s)"
        );
        assert_eq!(format!("{}", PcdError::NotFound), "PCD token not found");
        assert_eq!(
            format!("{}", PcdError::InvalidParameter),
            "value is incompatible with the PCD's existing definition"
        );
        assert_eq!(format!("{}", PcdError::Internal), "an unexpected internal error occurred");
    }

    #[test]
    fn test_pcd_error_from_efi_error() {
        assert_eq!(PcdError::from(EfiError::NotFound), PcdError::NotFound);
        assert_eq!(PcdError::from(EfiError::InvalidParameter), PcdError::InvalidParameter);
        assert_eq!(PcdError::from(EfiError::Unsupported), PcdError::Internal);
    }

    #[test]
    fn test_pcd_error_is_error_trait() {
        fn assert_error<E: core::error::Error>() {}
        assert_error::<PcdError>();
    }
}
