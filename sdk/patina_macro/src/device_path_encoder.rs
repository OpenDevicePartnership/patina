//! Checked binary encoding helpers for UEFI device paths.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use std::net::{Ipv4Addr, Ipv6Addr};

use crate::{
    device_path_nodes::{VendorDefinedSchema, encode_node},
    device_path_parser::{DevicePathError, ParsedDevicePath, ParsedNode, parse_device_path},
};

const END_INSTANCE: [u8; 4] = [0x7f, 0x01, 0x04, 0x00];
const END_ENTIRE: [u8; 4] = [0x7f, 0xff, 0x04, 0x00];

/// Parse and encode a complete UEFI text device path.
pub(crate) fn encode_device_path(input: &str) -> Result<Vec<u8>, DevicePathError> {
    encode_device_path_with_vendor_defined(input, &[])
}

/// Parse and encode a complete UEFI text device path with custom vendor nodes.
pub(crate) fn encode_device_path_with_vendor_defined(
    input: &str,
    vendor_defined_schemas: &[VendorDefinedSchema],
) -> Result<Vec<u8>, DevicePathError> {
    let path = parse_device_path(input)?;
    encode_parsed_device_path(&path, vendor_defined_schemas)
}

fn encode_parsed_device_path(
    path: &ParsedDevicePath,
    vendor_defined_schemas: &[VendorDefinedSchema],
) -> Result<Vec<u8>, DevicePathError> {
    let mut bytes = Vec::new();
    for (instance_index, instance) in path.instances.iter().enumerate() {
        for node in instance {
            bytes.extend_from_slice(&encode_node(node, vendor_defined_schemas)?);
        }
        if instance_index + 1 == path.instances.len() {
            bytes.extend_from_slice(&END_ENTIRE);
        } else {
            bytes.extend_from_slice(&END_INSTANCE);
        }
    }
    Ok(bytes)
}

/// A checked writer for one packed UEFI device path node.
pub(crate) struct NodeWriter {
    offset: usize,
    bytes: Vec<u8>,
}

impl NodeWriter {
    pub(crate) fn new(node: &ParsedNode, r#type: u8, subtype: u8) -> Self {
        Self { offset: node.offset, bytes: vec![r#type, subtype, 0, 0] }
    }

    pub(crate) fn byte(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn ascii_nul(&mut self, value: &str, field: &str) -> Result<(), DevicePathError> {
        if !value.is_ascii() {
            return Err(DevicePathError::new(self.offset, format!("{field} must contain only ASCII characters")));
        }
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        Ok(())
    }

    pub(crate) fn utf16(&mut self, value: &str, nul_terminated: bool) {
        for code_unit in value.encode_utf16() {
            self.u16(code_unit);
        }
        if nul_terminated {
            self.u16(0);
        }
    }

    pub(crate) fn finish(mut self) -> Result<Vec<u8>, DevicePathError> {
        let length = u16::try_from(self.bytes.len())
            .map_err(|_| DevicePathError::new(self.offset, "device path node exceeds the 65535-byte length limit"))?;
        self.bytes
            .get_mut(2..4)
            .expect("a node writer always contains a four-byte header")
            .copy_from_slice(&length.to_le_bytes());
        Ok(self.bytes)
    }
}

pub(crate) fn parse_unsigned(value: &str, bits: u32, offset: usize, field: &str) -> Result<u64, DevicePathError> {
    if value.is_empty() {
        return Err(DevicePathError::new(offset, format!("{field} is required")));
    }
    if value.starts_with('+') || value.starts_with('-') {
        return Err(DevicePathError::new(offset, format!("{field} must be an unsigned integer")));
    }

    let (digits, radix) =
        value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).map_or((value, 10), |digits| (digits, 16));
    if digits.is_empty() {
        return Err(DevicePathError::new(offset, format!("{field} is not a valid integer")));
    }

    let parsed = u64::from_str_radix(digits, radix)
        .map_err(|_| DevicePathError::new(offset, format!("{field} is not a valid integer")))?;
    let maximum = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
    if parsed > maximum {
        return Err(DevicePathError::new(offset, format!("{field} exceeds the {bits}-bit range")));
    }
    Ok(parsed)
}

pub(crate) fn parse_u8(value: &str, offset: usize, field: &str) -> Result<u8, DevicePathError> {
    Ok(parse_unsigned(value, 8, offset, field)? as u8)
}

pub(crate) fn parse_u16(value: &str, offset: usize, field: &str) -> Result<u16, DevicePathError> {
    Ok(parse_unsigned(value, 16, offset, field)? as u16)
}

pub(crate) fn parse_u32(value: &str, offset: usize, field: &str) -> Result<u32, DevicePathError> {
    Ok(parse_unsigned(value, 32, offset, field)? as u32)
}

pub(crate) fn parse_u64(value: &str, offset: usize, field: &str) -> Result<u64, DevicePathError> {
    parse_unsigned(value, 64, offset, field)
}

pub(crate) fn parse_hex_bytes(value: &str, offset: usize, field: &str) -> Result<Vec<u8>, DevicePathError> {
    let compact: String = value.chars().filter(|character| *character != '-').collect();
    if !compact.len().is_multiple_of(2) {
        return Err(DevicePathError::new(offset, format!("{field} must contain two hexadecimal digits per byte")));
    }

    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digits = std::str::from_utf8(pair).expect("hex digits are valid UTF-8");
            u8::from_str_radix(digits, 16)
                .map_err(|_| DevicePathError::new(offset, format!("{field} contains a non-hexadecimal byte")))
        })
        .collect()
}

pub(crate) fn parse_fixed_hex<const N: usize>(
    value: &str,
    offset: usize,
    field: &str,
) -> Result<[u8; N], DevicePathError> {
    let bytes = parse_hex_bytes(value, offset, field)?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        DevicePathError::new(offset, format!("{field} must be exactly {N} bytes, not {}", bytes.len()))
    })
}

pub(crate) fn efi_guid_bytes(value: &str) -> Result<[u8; 16], uuid::Error> {
    let guid = uuid::Uuid::parse_str(value)?;
    let (data1, data2, data3, data4) = guid.as_fields();
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&data1.to_le_bytes());
    bytes[4..6].copy_from_slice(&data2.to_le_bytes());
    bytes[6..8].copy_from_slice(&data3.to_le_bytes());
    bytes[8..16].copy_from_slice(data4);
    Ok(bytes)
}

pub(crate) fn parse_efi_guid(value: &str, offset: usize, field: &str) -> Result<[u8; 16], DevicePathError> {
    efi_guid_bytes(value).map_err(|_| DevicePathError::new(offset, format!("{field} is not a valid GUID")))
}

pub(crate) fn parse_uuid_bytes(value: &str, offset: usize, field: &str) -> Result<[u8; 16], DevicePathError> {
    let guid = uuid::Uuid::parse_str(value)
        .map_err(|_| DevicePathError::new(offset, format!("{field} is not a valid UUID")))?;
    Ok(*guid.as_bytes())
}

pub(crate) fn parse_eisa_id(value: &str, offset: usize, field: &str) -> Result<u32, DevicePathError> {
    if value == "0" {
        return Ok(0);
    }
    let bytes = value.as_bytes();
    let [first, second, third, product_0, product_1, product_2, product_3] = bytes else {
        return Err(DevicePathError::new(offset, format!("{field} is not a valid seven-character EISA ID")));
    };
    if ![first, second, third].iter().all(|byte| byte.is_ascii_alphabetic())
        || ![product_0, product_1, product_2, product_3].iter().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DevicePathError::new(offset, format!("{field} is not a valid seven-character EISA ID")));
    }

    let first = u32::from(first.to_ascii_uppercase() - b'A' + 1);
    let second = u32::from(second.to_ascii_uppercase() - b'A' + 1);
    let third = u32::from(third.to_ascii_uppercase() - b'A' + 1);
    let product = u16::from_str_radix(value.get(3..).expect("validated EISA ID has an ASCII product suffix"), 16)
        .map_err(|_| DevicePathError::new(offset, format!("{field} has an invalid product identifier")))?;
    Ok((u32::from(product) << 16) | (first << 10) | (second << 5) | third)
}

pub(crate) fn parse_ipv4(value: &str, offset: usize, field: &str) -> Result<[u8; 4], DevicePathError> {
    let address = value
        .parse::<Ipv4Addr>()
        .map_err(|_| DevicePathError::new(offset, format!("{field} is not a valid IPv4 address")))?;
    Ok(address.octets())
}

pub(crate) fn parse_ipv6(value: &str, offset: usize, field: &str) -> Result<[u8; 16], DevicePathError> {
    let address = value
        .parse::<Ipv6Addr>()
        .map_err(|_| DevicePathError::new(offset, format!("{field} is not a valid IPv6 address")))?;
    Ok(address.octets())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_requested_example() {
        assert_eq!(
            encode_device_path("PciRoot(0)/Pci(0x11,0)").expect("path should encode"),
            [
                0x02, 0x01, 0x0c, 0x00, 0xd0, 0x41, 0x03, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x06, 0x00, 0x00,
                0x11, 0x7f, 0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_parse_efi_guid_uses_uefi_byte_order() {
        assert_eq!(
            parse_efi_guid("00112233-4455-6677-8899-aabbccddeeff", 0, "Guid").expect("GUID should parse"),
            [0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]
        );
    }

    #[test]
    fn test_encode_multiple_instances_inserts_both_end_node_types() {
        assert_eq!(
            encode_device_path("Pci(1,0),USB(2,1)").expect("path should encode"),
            [
                0x01, 0x01, 0x06, 0x00, 0x00, 0x01, 0x7f, 0x01, 0x04, 0x00, 0x03, 0x05, 0x06, 0x00, 0x02, 0x01, 0x7f,
                0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_encode_uefi_ipv4_example() {
        assert_eq!(
            encode_device_path("IPv4(192.168.0.100,TCP,Static,192.168.0.1)").expect("path should encode"),
            [
                0x03, 0x0c, 0x1b, 0x00, 0xc0, 0xa8, 0x00, 0x01, 0xc0, 0xa8, 0x00, 0x64, 0x00, 0x00, 0x00, 0x00, 0x06,
                0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7f, 0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_encode_uefi_hard_drive_example() {
        assert_eq!(
            encode_device_path("HD(1,GPT,15E39A00-1DD2-1000-8D7F-00A0C92408FC,0x22,0x2710000)")
                .expect("path should encode"),
            [
                0x04, 0x01, 0x2a, 0x00, 0x01, 0x00, 0x00, 0x00, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x71, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9a, 0xe3, 0x15, 0xd2, 0x1d, 0x00, 0x10, 0x8d, 0x7f,
                0x00, 0xa0, 0xc9, 0x24, 0x08, 0xfc, 0x02, 0x02, 0x7f, 0xff, 0x04, 0x00,
            ]
        );
    }

    #[test]
    fn test_encode_file_path_uses_utf16_surrogate_pairs() {
        assert_eq!(
            encode_device_path("\u{1f4c1}").expect("path should encode"),
            [0x04, 0x04, 0x0a, 0x00, 0x3d, 0xd8, 0xc1, 0xdc, 0x00, 0x00, 0x7f, 0xff, 0x04, 0x00]
        );
    }

    #[test]
    fn test_encode_rejects_oversized_node() {
        let path = "a".repeat(32_765);
        let error = encode_device_path(&path).expect_err("oversized file path should fail");

        assert_eq!(error.message, "device path node exceeds the 65535-byte length limit");
    }
}
