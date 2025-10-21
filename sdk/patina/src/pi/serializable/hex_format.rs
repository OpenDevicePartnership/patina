//! Hex Format Utilities
//!
//! Helpers to convert numbers to/from hex strings in `serde` serialization/deserialization.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::format;
use core::fmt::LowerHex;
use serde::Deserialize;
use serde::{self, Deserializer, Serializer};

/// Serialize a number as a hex string with "0x" prefix.
pub fn serialize<T, S>(num: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: LowerHex,
    S: Serializer,
{
    serializer.serialize_str(&format!("0x{:x}", num))
}

/// Trait to parse a number from a hex string (with or without "0x" prefix).
pub trait FromStrRadix {
    /// Parse from a hex string without "0x" prefix.
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str>
    where
        Self: Sized;
}

impl FromStrRadix for u8 {
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str> {
        u8::from_str_radix(src, 16).map_err(|_| "Invalid hex u8")
    }
}

impl FromStrRadix for u16 {
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str> {
        u16::from_str_radix(src, 16).map_err(|_| "Invalid hex u16")
    }
}

impl FromStrRadix for u32 {
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str> {
        u32::from_str_radix(src, 16).map_err(|_| "Invalid hex u32")
    }
}

impl FromStrRadix for u64 {
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str> {
        u64::from_str_radix(src, 16).map_err(|_| "Invalid hex u64")
    }
}

impl FromStrRadix for usize {
    fn from_str_radix_16(src: &str) -> Result<Self, &'static str> {
        usize::from_str_radix(src, 16).map_err(|_| "Invalid hex usize")
    }
}

/// Deserialize a number from a hex string with "0x" prefix.
pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: FromStrRadix,
    D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    let s = s.strip_prefix("0x").ok_or_else(|| serde::de::Error::custom("Missing '0x' prefix"))?;
    T::from_str_radix_16(s).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::serializable::hex_format;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestStructU32 {
        #[serde(with = "hex_format")]
        value: u32,
    }

    #[test]
    fn test_serialize_u32() {
        let data = TestStructU32 { value: 0x1A2B3C };
        let json = serde_json::to_string(&data).unwrap();
        assert_eq!(json, r#"{"value":"0x1a2b3c"}"#);
    }

    #[test]
    fn test_deserialize_u32() {
        let json = r#"{"value":"0x1a2b3c"}"#;
        let data: TestStructU32 = serde_json::from_str(json).unwrap();
        assert_eq!(data, TestStructU32 { value: 0x1A2B3C });
    }

    #[test]
    fn test_deserialize_missing_prefix() {
        let json = r#"{"value":"1a2b3c"}"#;
        let result: Result<TestStructU32, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject missing 0x prefix");
    }

    #[test]
    fn test_roundtrip() {
        let original = TestStructU32 { value: 0xFF };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: TestStructU32 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_multiple_types() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestAll {
            #[serde(with = "hex_format")]
            u8v: u8,
            #[serde(with = "hex_format")]
            u16v: u16,
            #[serde(with = "hex_format")]
            u32v: u32,
            #[serde(with = "hex_format")]
            u64v: u64,
            #[serde(with = "hex_format")]
            usize_v: usize,
        }

        let data = TestAll { u8v: 0xAB, u16v: 0x1234, u32v: 0xDEADBEEF, u64v: 0xCAFEBABE1234, usize_v: 0x42 };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: TestAll = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, data);
    }
}
