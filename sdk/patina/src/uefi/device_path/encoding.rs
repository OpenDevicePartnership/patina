//! Scroll encoding contexts for device path fields.
//!
//! Fixed-width integers and byte arrays use scroll's built-in conversions. These
//! contexts cover the remaining field layouts of
//! [UEFI 2.11 Chapter 10](https://uefi.org/specs/UEFI/2.11/10_Protocols_Device_Path_Protocol.html):
//! strings with a specific encoding and terminator, EFI GUIDs, and payloads bounded by
//! the node length. Apply them with `#[scroll(ctx = ...)]` on derived fields.
//!
//! | Context | Rust type | Wire format |
//! |---|---|---|
//! | [`AsciiNul`] | [`String`] | ASCII bytes and a NUL terminator |
//! | [`Char16Nul`] | [`String`] | Little-endian UCS-2 code units and a `0x0000` terminator |
//! | [`Utf8Nul`] | [`String`] | UTF-8 bytes and a NUL terminator |
//! | [`Ascii`] | [`String`] | ASCII bytes filling the rest of the payload, no terminator |
//! | [`IpAddress`] | [`Ipv4Addr`], [`Ipv6Addr`] | `EFI_IPv4_ADDRESS` (4 bytes), `EFI_IPv6_ADDRESS` (16 bytes) |
//! | [`EfiMacAddress`] | [`efi::MacAddress`] | `EFI_MAC_ADDRESS` (32 bytes, zero padded) |
//! | [`Vector`] | `Vec<u8>`, `Vec<u16>`, `Vec<u32>` | Elements filling the rest of the payload; 16- and 32-bit values are little-endian |
//!
//! Reading produces the owned type; writing takes a borrowed `&T`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

// cspell:ignore nvmeoftarget nvmeofdisk

use crate::{BinaryGuid, Char16Str, Char16String, standard::efi};
use alloc::{string::String, vec::Vec};
use core::{
    net::{Ipv4Addr, Ipv6Addr},
    str,
};
use scroll::{
    Pread, Pwrite,
    ctx::{TryFromCtx, TryIntoCtx},
};

/// A scroll context for NUL-terminated ASCII strings.
///
/// Reads an owned [`String`] without the terminator and writes a borrowed
/// `&String` with a trailing NUL. Consumed and written byte counts include the
/// terminator. Bytes following the first NUL are left for subsequent fields.
///
/// # Errors
///
/// Reading rejects a missing terminator or non-ASCII text. Writing rejects
/// non-ASCII text, interior NULs, or insufficient destination space.
///
/// # Examples
///
/// ```rust
/// # extern crate alloc;
/// use alloc::string::String;
/// use patina::uefi::device_path::encoding::AsciiNul;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     id: u16,
///     #[scroll(ctx = AsciiNul)]
///     description: String,
/// }
///
/// let record: Record = b"\x01\x00ABC\0".pread_with(0, scroll::LE)?;
/// assert_eq!(record.id, 1);
/// assert_eq!(record.description, "ABC");
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct AsciiNul;

impl<'a> TryFromCtx<'a, AsciiNul> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: AsciiNul) -> Result<(Self, usize), Self::Error> {
        let len = src
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(scroll::Error::BadInput { size: 0, msg: "missing ASCII terminator" })?;
        let bytes = src.get(..len).ok_or(scroll::Error::BadInput { size: 0, msg: "invalid ASCII string bounds" })?;
        let (value, _) = String::try_from_ctx(bytes, Ascii)?;
        Ok((value, len + 1))
    }
}

impl TryIntoCtx<AsciiNul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: AsciiNul) -> Result<usize, Self::Error> {
        Ascii::validate(self)?;
        let required = self.len() + 1;
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        dst.gwrite_with(self.as_bytes(), &mut offset, ())?;
        dst.gwrite_with(0u8, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

/// A scroll context for NUL-terminated UEFI `CHAR16` strings, such as file paths.
///
/// Code units are little-endian UCS-2, following Patina's [`Char16Str`] rules.
/// Reads an owned [`String`] without the terminator and writes a borrowed `&String`
/// with a trailing `0x0000`. Byte counts include the terminator.
///
/// # Errors
///
/// Reading rejects a missing terminator, an incomplete code unit, or surrogate
/// code units. Writing rejects interior NULs, characters outside UCS-2, or
/// insufficient destination space.
///
/// # Examples
///
/// ```rust
/// # extern crate alloc;
/// use alloc::{string::String, vec::Vec};
/// use patina::uefi::device_path::encoding::Char16Nul;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     #[scroll(ctx = Char16Nul)]
///     path: String,
/// }
///
/// // The default boot file of UEFI 2.10 Section 3.5.1.1, as CHAR16 code units with a terminator.
/// let bytes: Vec<u8> = r"\EFI\BOOT\BOOTX64.EFI".encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
/// let record: Record = bytes.pread_with(0, scroll::LE)?;
/// assert_eq!(record.path, r"\EFI\BOOT\BOOTX64.EFI");
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Char16Nul;

impl<'a> TryFromCtx<'a, Char16Nul> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Char16Nul) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let mut char_16_string = Vec::new();
        loop {
            let char_16: u16 = src
                .gread_with(&mut offset, scroll::LE)
                .map_err(|_| scroll::Error::BadInput { size: 0, msg: "missing CHAR16 terminator" })?;
            char_16_string.push(char_16);
            if char_16 == 0 {
                break;
            }
        }
        let value = Char16Str::from_units_with_nul(&char_16_string)
            .map_err(|_| scroll::Error::BadInput { size: 0, msg: "CHAR16 string is not valid UCS-2" })?
            .chars()
            .collect();
        Ok((value, offset))
    }
}

impl TryIntoCtx<Char16Nul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Char16Nul) -> Result<usize, Self::Error> {
        let encoded = Char16String::try_from_str(self)
            .map_err(|_| scroll::Error::BadInput { size: 0, msg: "string is not valid CHAR16" })?;
        let char_16_str = encoded.as_units_with_nul();
        let required = size_of_val(char_16_str);
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        for char_16 in char_16_str {
            dst.gwrite_with(*char_16, &mut offset, scroll::LE)?;
        }
        Ok(offset)
    }
}

/// A scroll context for NUL-terminated UTF-8 strings, such as an NVMe-oF subsystem NQN.
///
/// Reads an owned [`String`] without the terminator and writes a borrowed `&String`
/// with a trailing NUL. Byte counts include the terminator. Field-specific maximum
/// lengths remain the responsibility of the containing node.
///
/// # Errors
///
/// Reading rejects a missing terminator or invalid UTF-8. Writing rejects interior
/// NULs or insufficient destination space.
///
/// # Examples
///
/// ```rust
/// # extern crate alloc;
/// use alloc::string::String;
/// use patina::uefi::device_path::encoding::Utf8Nul;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     #[scroll(ctx = Utf8Nul)]
///     subsystem_nqn: String,
/// }
///
/// // The subsystem NQN of the NVMe-oF example in UEFI 2.10 Section 10.3.4.33.
/// let record: Record = b"nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target\0".pread_with(0, scroll::LE)?;
/// assert_eq!(record.subsystem_nqn, "nqn.1991-05.org.uefi:nvmeoftarget-nvmeofdisk-target");
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Utf8Nul;

impl<'a> TryFromCtx<'a, Utf8Nul> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Utf8Nul) -> Result<(Self, usize), Self::Error> {
        let len = src
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(scroll::Error::BadInput { size: 0, msg: "missing UTF-8 terminator" })?;
        let bytes = src.get(..len).ok_or(scroll::Error::BadInput { size: 0, msg: "invalid UTF-8 string bounds" })?;
        let value =
            str::from_utf8(bytes).map_err(|_| scroll::Error::BadInput { size: 0, msg: "invalid UTF-8 string" })?;
        Ok((String::from(value), len + 1))
    }
}

impl TryIntoCtx<Utf8Nul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Utf8Nul) -> Result<usize, Self::Error> {
        if self.as_bytes().contains(&0) {
            return Err(scroll::Error::BadInput { size: 0, msg: "UTF-8 string contains an embedded NUL" });
        }
        let required = self.len() + 1;
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        dst.gwrite_with(self.as_bytes(), &mut offset, ())?;
        dst.gwrite_with(0u8, &mut offset, scroll::LE)?;
        Ok(offset)
    }
}

/// A scroll context for an ASCII string that fills the rest of the node payload.
///
/// The wire format has no terminator; the node length bounds the string, which may be
/// empty. Reading consumes every remaining byte in the source, so the field must be last
/// in its structure.
///
/// # Errors
///
/// Reading and writing reject non-ASCII text or NULs. Writing also rejects
/// insufficient destination space.
///
/// # Examples
///
/// ```rust
/// # extern crate alloc;
/// use alloc::string::String;
/// use patina::uefi::device_path::encoding::Ascii;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     id: u16,
///     #[scroll(ctx = Ascii)]
///     text: String,
/// }
///
/// let record: Record = b"\x01\x00ABC".pread_with(0, scroll::LE)?;
/// assert_eq!(record.id, 1);
/// assert_eq!(record.text, "ABC");
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Ascii;

impl Ascii {
    /// Reject text that is not ASCII or that contains a NUL.
    fn validate(value: &str) -> Result<(), scroll::Error> {
        for (i, c) in value.chars().enumerate() {
            if !c.is_ascii() {
                return Err(scroll::Error::BadInput { size: i, msg: "Device Path string must be ASCII" });
            }
            if c == '\0' {
                return Err(scroll::Error::BadInput {
                    size: i,
                    msg: "Device Path string must not contain embedded NUL",
                });
            }
        }
        Ok(())
    }
}

impl<'a> TryFromCtx<'a, Ascii> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Ascii) -> Result<(Self, usize), Self::Error> {
        let value =
            str::from_utf8(src).map_err(|_| scroll::Error::BadInput { size: 0, msg: "string must be ASCII" })?;
        Ascii::validate(value)?;
        Ok((String::from(value), src.len()))
    }
}

impl TryIntoCtx<Ascii> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Ascii) -> Result<usize, Self::Error> {
        Ascii::validate(self)?;
        dst.pwrite_with(self.as_bytes(), 0, ())
    }
}

// Scroll encoding of `BinaryGuid` fields: 16 bytes in the UEFI wire layout, where `Data1`,
// `Data2`, and `Data3` are little-endian and `Data4` is a byte array. The scroll endianness
// of the containing structure does not affect this layout, so derived node encoders can
// use `BinaryGuid` fields without a context attribute.
impl<'a> TryFromCtx<'a, scroll::Endian> for BinaryGuid {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let bytes: [u8; 16] = src.pread_with(0, scroll::LE)?;
        Ok((BinaryGuid::from_bytes(&bytes), bytes.len()))
    }
}

impl TryIntoCtx<scroll::Endian> for &BinaryGuid {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: scroll::Endian) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.as_bytes().as_slice(), 0, ())
    }
}

impl TryIntoCtx<scroll::Endian> for BinaryGuid {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], ctx: scroll::Endian) -> Result<usize, Self::Error> {
        (&self).try_into_ctx(dst, ctx)
    }
}

/// A scroll context for an IP address stored as its octets in network byte order.
///
/// Used for `EFI_IPv4_ADDRESS` (4 bytes) and `EFI_IPv6_ADDRESS` (16 bytes) fields, read
/// into [`Ipv4Addr`] and [`Ipv6Addr`]. The scroll endianness of the containing structure
/// does not affect this layout.
///
/// # Examples
///
/// ```
/// use core::net::Ipv6Addr;
/// use patina::uefi::device_path::encoding::IpAddress;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Endpoint {
///     #[scroll(ctx = IpAddress)]
///     address: Ipv6Addr,
/// }
///
/// let bytes = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
/// let endpoint: Endpoint = bytes.pread_with(0, scroll::LE).unwrap();
/// assert_eq!(endpoint.address, "2001:db8::1".parse::<Ipv6Addr>().unwrap());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct IpAddress;

impl<'a> TryFromCtx<'a, IpAddress> for Ipv4Addr {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: IpAddress) -> Result<(Self, usize), Self::Error> {
        let octets: [u8; 4] = src.pread_with(0, scroll::LE)?;
        Ok((Ipv4Addr::from(octets), octets.len()))
    }
}

impl TryIntoCtx<IpAddress> for &Ipv4Addr {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: IpAddress) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.octets().as_slice(), 0, ())
    }
}

impl<'a> TryFromCtx<'a, IpAddress> for Ipv6Addr {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: IpAddress) -> Result<(Self, usize), Self::Error> {
        let octets: [u8; 16] = src.pread_with(0, scroll::LE)?;
        Ok((Ipv6Addr::from(octets), octets.len()))
    }
}

impl TryIntoCtx<IpAddress> for &Ipv6Addr {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: IpAddress) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.octets().as_slice(), 0, ())
    }
}

/// A scroll context for an `EFI_MAC_ADDRESS`, read into [`efi::MacAddress`].
///
/// The address is always 32 bytes in wire order. The interface type of the containing
/// node defines how many bytes are significant, such as 6 for Ethernet; producers pad the
/// rest with zeros. The scroll endianness of the containing structure does not affect
/// this layout.
///
/// # Examples
///
/// ```rust
/// use patina::uefi::device_path::encoding::EfiMacAddress;
/// use r_efi::efi;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     #[scroll(ctx = EfiMacAddress)]
///     address: efi::MacAddress,
///     interface_type: u8,
/// }
///
/// // The Ethernet address of the iSCSI example in UEFI 2.10 Section 10.3.4.20.2, padded to 32 bytes.
/// let mut bytes = [0u8; 33];
/// bytes[..6].copy_from_slice(&[0x00, 0x13, 0x20, 0xf5, 0xfa, 0x77]);
/// bytes[32] = 1;
/// let record: Record = bytes.pread_with(0, scroll::LE)?;
/// assert_eq!(record.address.addr[..6], [0x00, 0x13, 0x20, 0xf5, 0xfa, 0x77]);
/// assert_eq!(record.interface_type, 1);
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct EfiMacAddress;

impl<'a> TryFromCtx<'a, EfiMacAddress> for efi::MacAddress {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: EfiMacAddress) -> Result<(Self, usize), Self::Error> {
        let addr: [u8; 32] = src.pread_with(0, scroll::LE)?;
        Ok((efi::MacAddress { addr }, addr.len()))
    }
}

impl TryIntoCtx<EfiMacAddress> for &efi::MacAddress {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: EfiMacAddress) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.addr.as_slice(), 0, ())
    }
}

/// A scroll context for a vector that fills the rest of the node payload.
///
/// The wire format has no element count; the node length bounds the vector. Reading consumes every remaining byte in the source, so the field must be last
/// in its structure. Supports opaque bytes (`Vec<u8>`), such as vendor-defined
/// data, little-endian 16-bit code units (`Vec<u16>`), such as a USB WWID
/// serial number, and little-endian 32-bit values (`Vec<u32>`), such as ACPI
/// `_ADR` values. Semantic limits remain the responsibility of the containing node.
///
/// # Errors
///
/// Reading rejects a byte count that is not a multiple of the element size.
/// Writing rejects insufficient destination space.
///
/// # Examples
///
/// ```rust
/// # extern crate alloc;
/// use alloc::vec::Vec;
/// use patina::uefi::device_path::encoding::Vector;
/// use scroll::{Pread, Pwrite};
///
/// #[derive(Pread, Pwrite)]
/// struct Record {
///     #[scroll(ctx = Vector)]
///     addresses: Vec<u32>,
/// }
///
/// // Two ACPI `_ADR` values, as in an `AcpiAdr(0x80010100,0x80010200)` node.
/// let bytes = [0x00, 0x01, 0x01, 0x80, 0x00, 0x02, 0x01, 0x80];
/// let record: Record = bytes.pread_with(0, scroll::LE)?;
/// assert_eq!(record.addresses, [0x80010100, 0x80010200]);
/// # Ok::<(), scroll::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Vector;

impl<'a> TryFromCtx<'a, Vector> for Vec<u8> {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Vector) -> Result<(Self, usize), Self::Error> {
        Ok((src.to_vec(), src.len()))
    }
}

impl TryIntoCtx<Vector> for &Vec<u8> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Vector) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.as_slice(), 0, ())
    }
}

impl<'a> TryFromCtx<'a, Vector> for Vec<u16> {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Vector) -> Result<(Self, usize), Self::Error> {
        if !src.len().is_multiple_of(2) {
            return Err(scroll::Error::BadInput {
                size: 0,
                msg: "trailing payload has an incomplete 16-bit code unit",
            });
        }
        let mut offset = 0;
        let mut units = Vec::with_capacity(src.len() / 2);
        while offset < src.len() {
            units.push(src.gread_with::<u16>(&mut offset, scroll::LE)?);
        }
        Ok((units, offset))
    }
}

impl TryIntoCtx<Vector> for &Vec<u16> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Vector) -> Result<usize, Self::Error> {
        let required = size_of_val(self.as_slice());
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        for unit in self {
            dst.gwrite_with(*unit, &mut offset, scroll::LE)?;
        }
        Ok(offset)
    }
}

impl<'a> TryFromCtx<'a, Vector> for Vec<u32> {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Vector) -> Result<(Self, usize), Self::Error> {
        if !src.len().is_multiple_of(4) {
            return Err(scroll::Error::BadInput { size: 0, msg: "trailing payload has an incomplete 32-bit value" });
        }
        let mut offset = 0;
        let mut values = Vec::with_capacity(src.len() / 4);
        while offset < src.len() {
            values.push(src.gread_with::<u32>(&mut offset, scroll::LE)?);
        }
        Ok((values, offset))
    }
}

impl TryIntoCtx<Vector> for &Vec<u32> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Vector) -> Result<usize, Self::Error> {
        let required = size_of_val(self.as_slice());
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        for value in self {
            dst.gwrite_with(*value, &mut offset, scroll::LE)?;
        }
        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn read<'a, T, C: Copy>(src: &'a [u8], ctx: C) -> Result<(T, usize), scroll::Error>
    where
        T: TryFromCtx<'a, C, Error = scroll::Error>,
    {
        T::try_from_ctx(src, ctx)
    }

    fn write<T: TryIntoCtx<C, Error = scroll::Error>, C: Copy>(
        value: T,
        dst: &mut [u8],
        ctx: C,
    ) -> Result<usize, scroll::Error> {
        value.try_into_ctx(dst, ctx)
    }

    #[derive(Debug, Clone, Eq, PartialEq, Pread, Pwrite)]
    struct Record {
        prefix: u16,
        #[scroll(ctx = AsciiNul)]
        text: String,
        suffix: u16,
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_read() {
        let (value, consumed) = read::<String, _>(b"ABC\0tail", AsciiNul).unwrap();
        assert_eq!(value, "ABC");
        assert_eq!(consumed, 4);
        let (value, consumed) = read::<String, _>(b"\0tail", AsciiNul).unwrap();
        assert!(value.is_empty());
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_rejects_missing_terminator() {
        for input in [b"".as_slice(), b"ABC".as_slice()] {
            assert!(read::<String, _>(input, AsciiNul).is_err());
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_rejects_non_ascii_input() {
        for input in [&[0xff, 0][..], &[0xc3, 0xa9, 0][..]] {
            assert!(read::<String, _>(input, AsciiNul).is_err());
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_write() {
        for text in ["", "ABC"] {
            let value = String::from(text);
            let required = value.len() + 1;
            for length in [required, required + 4] {
                let mut bytes = vec![0xa5; length];
                assert_eq!(bytes.pwrite_with(&value, 0, AsciiNul).unwrap(), required);
                assert_eq!(&bytes[..value.len()], value.as_bytes());
                assert_eq!(bytes[value.len()], 0);
                assert!(bytes[required..].iter().all(|byte| *byte == 0xa5));
            }
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_rejects_invalid_output() {
        for text in ["A\0B", "\u{e9}"] {
            let value = String::from(text);
            let mut bytes = [0xa5; 16];
            assert!(write(&value, &mut bytes, AsciiNul).is_err());
            assert_eq!(bytes, [0xa5; 16]);
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_short_output_is_unchanged() {
        for text in ["", "ABC"] {
            let value = String::from(text);
            for length in 0..=value.len() {
                let mut bytes = vec![0xa5; length];
                assert!(write(&value, &mut bytes, AsciiNul).is_err());
                assert!(bytes.iter().all(|byte| *byte == 0xa5));
            }
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_derives() {
        let record = Record { prefix: 0x1234, text: String::from("ABC"), suffix: 0x5678 };
        for (endian, expected) in [
            (scroll::LE, [0x34, 0x12, b'A', b'B', b'C', 0, 0x78, 0x56]),
            (scroll::BE, [0x12, 0x34, b'A', b'B', b'C', 0, 0x56, 0x78]),
        ] {
            let mut bytes = [0; 8];
            assert_eq!(bytes.pwrite_with(&record, 0, endian).unwrap(), expected.len());
            assert_eq!(bytes, expected);
            let (parsed, consumed) = Record::try_from_ctx(&bytes, endian).unwrap();
            assert_eq!(parsed, record);
            assert_eq!(consumed, expected.len());
        }
    }

    #[test]
    fn test_device_path_encoding_ascii_nul_derived_reader_rejects_invalid_text() {
        assert!(Record::try_from_ctx(&[0, 0, b'A', b'B'], scroll::LE).is_err());
        assert!(Record::try_from_ctx(&[0, 0, 0xff, 0, 0, 0], scroll::LE).is_err());
    }

    fn write_all<C: Copy>(value: &String, ctx: C, expected: &[u8])
    where
        for<'a> &'a String: TryIntoCtx<C, Error = scroll::Error>,
    {
        let mut bytes = vec![0xa5; expected.len() + 2];
        assert_eq!(bytes.pwrite_with(value, 0, ctx).unwrap(), expected.len());
        assert_eq!(&bytes[..expected.len()], expected);
        assert_eq!(&bytes[expected.len()..], &[0xa5, 0xa5]);
        for length in 0..expected.len() {
            let mut short = vec![0xa5; length];
            assert!(write(value, &mut short, ctx).is_err());
            assert!(short.iter().all(|byte| *byte == 0xa5));
        }
    }

    #[test]
    fn test_device_path_encoding_char16_nul() {
        let wire = [b'\\', 0, b'A', 0, 0xe9, 0, 0, 0, 0xff, 0xff];
        let (value, consumed) = read::<String, _>(&wire, Char16Nul).unwrap();
        assert_eq!(value, "\\A\u{e9}");
        assert_eq!(consumed, 8);
        write_all(&value, Char16Nul, &wire[..8]);
        let (empty, consumed) = read::<String, _>(&[0, 0], Char16Nul).unwrap();
        assert!(empty.is_empty());
        assert_eq!(consumed, 2);
        write_all(&empty, Char16Nul, &[0, 0]);
    }

    #[test]
    fn test_device_path_encoding_char16_nul_rejects_invalid_values() {
        for input in [&[][..], &[b'A', 0][..], &[b'A', 0, 0][..], &[0x00, 0xd8, 0, 0][..]] {
            assert!(read::<String, _>(input, Char16Nul).is_err());
        }
        for text in ["A\0B", "\u{1f600}"] {
            let mut bytes = [0xa5; 16];
            assert!(write(&String::from(text), &mut bytes, Char16Nul).is_err());
            assert_eq!(bytes, [0xa5; 16]);
        }
    }

    #[test]
    fn test_device_path_encoding_utf8_nul() {
        let wire = [b'n', 0xc3, 0xa9, 0, b'x'];
        let (value, consumed) = read::<String, _>(&wire, Utf8Nul).unwrap();
        assert_eq!(value, "n\u{e9}");
        assert_eq!(consumed, 4);
        write_all(&value, Utf8Nul, &wire[..4]);
        write_all(&String::new(), Utf8Nul, &[0]);
    }

    #[test]
    fn test_device_path_encoding_utf8_nul_rejects_invalid_values() {
        for input in [&[][..], &b"nqn"[..], &[0xff, 0][..]] {
            assert!(read::<String, _>(input, Utf8Nul).is_err());
        }
        let mut bytes = [0xa5; 8];
        assert!(write(&String::from("a\0b"), &mut bytes, Utf8Nul).is_err());
        assert_eq!(bytes, [0xa5; 8]);
    }

    #[test]
    fn test_device_path_encoding_guid_layout() {
        let guid =
            BinaryGuid::from_fields(0x0011_2233, 0x4455, 0x6677, 0x88, 0x99, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let wire = [0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        for ctx in [scroll::LE, scroll::BE] {
            let (parsed, consumed) = read::<BinaryGuid, _>(&wire, ctx).unwrap();
            assert_eq!(parsed, guid);
            assert_eq!(consumed, 16);
            let mut bytes = [0xa5; 17];
            assert_eq!(bytes.pwrite_with(&guid, 0, ctx).unwrap(), 16);
            assert_eq!(&bytes[..16], &wire);
            assert_eq!(bytes[16], 0xa5);
        }
        assert!(read::<BinaryGuid, _>(&wire[..15], scroll::LE).is_err());
        let mut short = [0xa5; 15];
        assert!(write(guid, &mut short, scroll::LE).is_err());
        assert_eq!(short, [0xa5; 15]);
    }

    #[test]
    fn test_device_path_encoding_ip_address_layout() {
        let v4 = Ipv4Addr::new(192, 168, 1, 2);
        let v6: Ipv6Addr = "2001:4898:a:1005:215:17ff:fe21:5593".parse().unwrap();
        let v6_wire = [0x20, 0x01, 0x48, 0x98, 0x00, 0x0a, 0x10, 0x05, 0x02, 0x15, 0x17, 0xff, 0xfe, 0x21, 0x55, 0x93];
        assert_eq!(read::<Ipv4Addr, _>(&[192, 168, 1, 2], IpAddress).unwrap(), (v4, 4));
        assert_eq!(read::<Ipv6Addr, _>(&v6_wire, IpAddress).unwrap(), (v6, 16));
        let mut bytes = [0xa5; 17];
        assert_eq!(bytes.pwrite_with(&v6, 0, IpAddress).unwrap(), 16);
        assert_eq!(&bytes[..16], &v6_wire);
        assert_eq!(bytes[16], 0xa5);
        assert_eq!(bytes.pwrite_with(&v4, 0, IpAddress).unwrap(), 4);
        assert_eq!(&bytes[..4], &[192, 168, 1, 2]);
        assert!(read::<Ipv4Addr, _>(&[1, 2, 3], IpAddress).is_err());
        assert!(read::<Ipv6Addr, _>(&v6_wire[..15], IpAddress).is_err());
        let mut short = [0xa5; 15];
        assert!(write(&v6, &mut short, IpAddress).is_err());
        assert_eq!(short, [0xa5; 15]);
    }

    #[test]
    fn test_device_path_encoding_mac_address_layout() {
        let mut wire = [0; 32];
        wire[..6].copy_from_slice(&[0x00, 0x15, 0x5d, 0x01, 0x02, 0x03]);
        let mac = efi::MacAddress { addr: wire };
        assert_eq!(read::<efi::MacAddress, _>(&wire, EfiMacAddress).unwrap(), (mac, 32));
        let mut bytes = [0xa5; 33];
        assert_eq!(bytes.pwrite_with(&mac, 0, EfiMacAddress).unwrap(), 32);
        assert_eq!(&bytes[..32], &wire);
        assert_eq!(bytes[32], 0xa5);
        assert!(read::<efi::MacAddress, _>(&wire[..31], EfiMacAddress).is_err());
        let mut short = [0xa5; 31];
        assert!(write(&mac, &mut short, EfiMacAddress).is_err());
        assert_eq!(short, [0xa5; 31]);
    }

    #[test]
    fn test_device_path_encoding_ascii_fills_payload() {
        for text in ["", "https://a"] {
            let (parsed, consumed) = read::<String, _>(text.as_bytes(), Ascii).unwrap();
            assert_eq!(parsed, text);
            assert_eq!(consumed, text.len());
            let mut bytes = vec![0xa5; text.len() + 1];
            assert_eq!(bytes.pwrite_with(&String::from(text), 0, Ascii).unwrap(), text.len());
            assert_eq!(&bytes[..text.len()], text.as_bytes());
        }
        for input in [&b"a\0b"[..], "\u{e9}".as_bytes(), &[0xff]] {
            assert!(read::<String, _>(input, Ascii).is_err());
        }
        for value in ["a\0b", "\u{e9}"] {
            let mut bytes = [0xa5; 4];
            assert!(write(&String::from(value), &mut bytes, Ascii).is_err());
            assert_eq!(bytes, [0xa5; 4]);
        }
        let mut short = [0xa5; 2];
        assert!(write(&String::from("abc"), &mut short, Ascii).is_err());
        assert_eq!(short, [0xa5; 2]);
    }

    #[test]
    fn test_device_path_encoding_remaining_bytes() {
        for payload in [vec![], vec![1, 2, 3]] {
            let (parsed, consumed) = read::<Vec<u8>, _>(&payload, Vector).unwrap();
            assert_eq!(parsed, payload);
            assert_eq!(consumed, payload.len());
            let mut bytes = vec![0xa5; payload.len() + 1];
            assert_eq!(bytes.pwrite_with(&payload, 0, Vector).unwrap(), payload.len());
            assert_eq!(&bytes[..payload.len()], payload.as_slice());
        }
        let mut short = [0xa5; 2];
        assert!(write(&vec![1u8, 2, 3], &mut short, Vector).is_err());
        assert_eq!(short, [0xa5; 2]);
    }

    #[test]
    fn test_device_path_encoding_remaining_code_units() {
        let (units, consumed) = read::<Vec<u16>, _>(&[0x41, 0, 0x34, 0x12], Vector).unwrap();
        assert_eq!(units, [0x41, 0x1234]);
        assert_eq!(consumed, 4);
        let mut bytes = [0xa5; 5];
        assert_eq!(bytes.pwrite_with(&units, 0, Vector).unwrap(), 4);
        assert_eq!(bytes, [0x41, 0, 0x34, 0x12, 0xa5]);
        assert!(read::<Vec<u16>, _>(&[1, 2, 3], Vector).is_err());
        let mut short = [0xa5; 3];
        assert!(write(&units, &mut short, Vector).is_err());
        assert_eq!(short, [0xa5; 3]);
    }

    #[test]
    fn test_device_path_encoding_vector_u32() {
        let (values, consumed) = read::<Vec<u32>, _>(&[1, 0, 0, 0, 0x78, 0x56, 0x34, 0x12], Vector).unwrap();
        assert_eq!(values, [1, 0x1234_5678]);
        assert_eq!(consumed, 8);
        let mut bytes = [0xa5; 9];
        assert_eq!(bytes.pwrite_with(&values, 0, Vector).unwrap(), 8);
        assert_eq!(bytes, [1, 0, 0, 0, 0x78, 0x56, 0x34, 0x12, 0xa5]);
        assert!(read::<Vec<u32>, _>(&[1, 2, 3], Vector).is_err());
        let mut short = [0xa5; 7];
        assert!(write(&values, &mut short, Vector).is_err());
        assert_eq!(short, [0xa5; 7]);
    }

    #[derive(Debug, Clone, Eq, PartialEq, Pread, Pwrite)]
    struct VendorRecord {
        kind: u16,
        guid: BinaryGuid,
        #[scroll(ctx = Char16Nul)]
        path: String,
        #[scroll(ctx = Vector)]
        data: Vec<u8>,
    }

    #[test]
    fn test_device_path_encoding_contexts_derive_together() {
        let record = VendorRecord {
            kind: 0x0102,
            guid: BinaryGuid::from_bytes(&[7; 16]),
            path: String::from("A"),
            data: vec![0xaa, 0xbb],
        };
        let mut expected = vec![0x02, 0x01];
        expected.extend_from_slice(&[7; 16]);
        expected.extend_from_slice(&[b'A', 0, 0, 0, 0xaa, 0xbb]);
        let mut bytes = vec![0; expected.len()];
        assert_eq!(bytes.pwrite_with(&record, 0, scroll::LE).unwrap(), expected.len());
        assert_eq!(bytes, expected);
        let (parsed, consumed) = VendorRecord::try_from_ctx(&bytes, scroll::LE).unwrap();
        assert_eq!(parsed, record);
        assert_eq!(consumed, expected.len());
    }
}
