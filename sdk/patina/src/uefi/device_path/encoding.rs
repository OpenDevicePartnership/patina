//! Scroll encoding contexts for device path fields.
//!
//! Fixed-width integers and byte arrays use scroll's built-in conversions. These
//! contexts cover the remaining UEFI 2.11 Chapter 10 field layouts: strings with a
//! specific encoding and terminator, EFI GUIDs, and payloads bounded by the node
//! length. Apply them with `#[scroll(ctx = ...)]` on derived fields.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use crate::{Char16Str, Char16String, standard::efi};
use alloc::{string::String, vec::Vec};
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
        let mut offset = 0;
        let value = read_ascii(src, &mut offset)?;
        Ok((value, offset))
    }
}

impl TryIntoCtx<AsciiNul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: AsciiNul) -> Result<usize, Self::Error> {
        ascii(self)?;
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
#[derive(Debug, Clone, Copy)]
pub struct Char16Nul;

impl<'a> TryFromCtx<'a, Char16Nul> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Char16Nul) -> Result<(Self, usize), Self::Error> {
        let mut offset = 0;
        let mut units = Vec::new();
        loop {
            let unit: u16 =
                src.gread_with(&mut offset, scroll::LE).map_err(|_| invalid("missing CHAR16 terminator"))?;
            units.push(unit);
            if unit == 0 {
                break;
            }
        }
        let value = Char16Str::from_units_with_nul(&units)
            .map_err(|_| invalid("CHAR16 string is not valid UCS-2"))?
            .chars()
            .collect();
        Ok((value, offset))
    }
}

impl TryIntoCtx<Char16Nul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Char16Nul) -> Result<usize, Self::Error> {
        let encoded = Char16String::try_from_str(self).map_err(|_| invalid("string is not valid CHAR16"))?;
        let units = encoded.as_units_with_nul();
        let required = units.len() * 2;
        if dst.len() < required {
            return Err(scroll::Error::TooBig { size: required, len: dst.len() });
        }
        let mut offset = 0;
        for unit in units {
            dst.gwrite_with(*unit, &mut offset, scroll::LE)?;
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
#[derive(Debug, Clone, Copy)]
pub struct Utf8Nul;

impl<'a> TryFromCtx<'a, Utf8Nul> for String {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Utf8Nul) -> Result<(Self, usize), Self::Error> {
        let len = src.iter().position(|byte| *byte == 0).ok_or_else(|| invalid("missing UTF-8 terminator"))?;
        let bytes = src.get(..len).ok_or_else(|| invalid("invalid UTF-8 string bounds"))?;
        let value = core::str::from_utf8(bytes).map_err(|_| invalid("invalid UTF-8 string"))?;
        Ok((String::from(value), len + 1))
    }
}

impl TryIntoCtx<Utf8Nul> for &String {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Utf8Nul) -> Result<usize, Self::Error> {
        if self.as_bytes().contains(&0) {
            return Err(invalid("UTF-8 string contains an embedded NUL"));
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

/// A scroll context for an `EFI_GUID` stored in its 16-byte UEFI wire layout.
///
/// `Data1`, `Data2`, and `Data3` are little-endian; `Data4` is a byte array. The
/// scroll endianness of the containing structure does not affect this layout.
#[derive(Debug, Clone, Copy)]
pub struct EfiGuid;

impl<'a> TryFromCtx<'a, EfiGuid> for efi::Guid {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: EfiGuid) -> Result<(Self, usize), Self::Error> {
        let bytes: [u8; 16] = src.pread_with(0, scroll::LE)?;
        Ok((efi::Guid::from_bytes(&bytes), bytes.len()))
    }
}

impl TryIntoCtx<EfiGuid> for &efi::Guid {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: EfiGuid) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.as_bytes().as_slice(), 0, ())
    }
}

/// A scroll context for a trailing payload bounded by the node length.
///
/// Reading consumes every remaining byte in the source, so the field must be last
/// in its structure. Supports opaque bytes (`Vec<u8>`), such as vendor-defined
/// data, and little-endian 16-bit code units (`Vec<u16>`), such as a USB WWID
/// serial number. Semantic limits remain the responsibility of the containing node.
///
/// # Errors
///
/// Reading `Vec<u16>` rejects an odd number of bytes. Writing rejects
/// insufficient destination space.
#[derive(Debug, Clone, Copy)]
pub struct Remaining;

impl<'a> TryFromCtx<'a, Remaining> for Vec<u8> {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Remaining) -> Result<(Self, usize), Self::Error> {
        Ok((src.to_vec(), src.len()))
    }
}

impl TryIntoCtx<Remaining> for &Vec<u8> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Remaining) -> Result<usize, Self::Error> {
        dst.pwrite_with(self.as_slice(), 0, ())
    }
}

impl<'a> TryFromCtx<'a, Remaining> for Vec<u16> {
    type Error = scroll::Error;

    fn try_from_ctx(src: &'a [u8], _: Remaining) -> Result<(Self, usize), Self::Error> {
        if !src.len().is_multiple_of(2) {
            return Err(invalid("trailing payload has an incomplete 16-bit code unit"));
        }
        let mut offset = 0;
        let mut units = Vec::with_capacity(src.len() / 2);
        while offset < src.len() {
            units.push(src.gread_with::<u16>(&mut offset, scroll::LE)?);
        }
        Ok((units, offset))
    }
}

impl TryIntoCtx<Remaining> for &Vec<u16> {
    type Error = scroll::Error;

    fn try_into_ctx(self, dst: &mut [u8], _: Remaining) -> Result<usize, Self::Error> {
        let required = self.len() * 2;
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

pub(super) fn invalid(msg: &'static str) -> scroll::Error {
    scroll::Error::BadInput { size: 0, msg }
}

pub(super) fn ascii(value: &str) -> Result<(), scroll::Error> {
    if !value.is_ascii() || value.as_bytes().contains(&0) {
        return Err(invalid("device path string must be ASCII without embedded NUL"));
    }
    Ok(())
}

pub(super) fn read_ascii(data: &[u8], offset: &mut usize) -> Result<String, scroll::Error> {
    let remaining = data.get(*offset..).ok_or_else(|| invalid("missing ASCII string"))?;
    let len = remaining.iter().position(|byte| *byte == 0).ok_or_else(|| invalid("missing ASCII terminator"))?;
    let bytes = remaining.get(..len).ok_or_else(|| invalid("invalid ASCII string bounds"))?;
    let value = core::str::from_utf8(bytes).map_err(|_| invalid("invalid ASCII string"))?;
    ascii(value)?;
    *offset += len + 1;
    Ok(String::from(value))
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
    fn test_device_path_encoding_efi_guid_layout() {
        let guid =
            efi::Guid::from_fields(0x0011_2233, 0x4455, 0x6677, 0x88, 0x99, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        let wire = [0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        let (parsed, consumed) = read::<efi::Guid, _>(&wire, EfiGuid).unwrap();
        assert_eq!(parsed, guid);
        assert_eq!(consumed, 16);
        let mut bytes = [0xa5; 17];
        assert_eq!(bytes.pwrite_with(&guid, 0, EfiGuid).unwrap(), 16);
        assert_eq!(&bytes[..16], &wire);
        assert_eq!(bytes[16], 0xa5);
        assert!(read::<efi::Guid, _>(&wire[..15], EfiGuid).is_err());
        let mut short = [0xa5; 15];
        assert!(write(&guid, &mut short, EfiGuid).is_err());
        assert_eq!(short, [0xa5; 15]);
    }

    #[test]
    fn test_device_path_encoding_remaining_bytes() {
        for payload in [vec![], vec![1, 2, 3]] {
            let (parsed, consumed) = read::<Vec<u8>, _>(&payload, Remaining).unwrap();
            assert_eq!(parsed, payload);
            assert_eq!(consumed, payload.len());
            let mut bytes = vec![0xa5; payload.len() + 1];
            assert_eq!(bytes.pwrite_with(&payload, 0, Remaining).unwrap(), payload.len());
            assert_eq!(&bytes[..payload.len()], payload.as_slice());
        }
        let mut short = [0xa5; 2];
        assert!(write(&vec![1u8, 2, 3], &mut short, Remaining).is_err());
        assert_eq!(short, [0xa5; 2]);
    }

    #[test]
    fn test_device_path_encoding_remaining_code_units() {
        let (units, consumed) = read::<Vec<u16>, _>(&[0x41, 0, 0x34, 0x12], Remaining).unwrap();
        assert_eq!(units, [0x41, 0x1234]);
        assert_eq!(consumed, 4);
        let mut bytes = [0xa5; 5];
        assert_eq!(bytes.pwrite_with(&units, 0, Remaining).unwrap(), 4);
        assert_eq!(bytes, [0x41, 0, 0x34, 0x12, 0xa5]);
        assert!(read::<Vec<u16>, _>(&[1, 2, 3], Remaining).is_err());
        let mut short = [0xa5; 3];
        assert!(write(&units, &mut short, Remaining).is_err());
        assert_eq!(short, [0xa5; 3]);
    }

    #[derive(Debug, Clone, Eq, PartialEq, Pread, Pwrite)]
    struct VendorRecord {
        kind: u16,
        #[scroll(ctx = EfiGuid)]
        guid: efi::Guid,
        #[scroll(ctx = Char16Nul)]
        path: String,
        #[scroll(ctx = Remaining)]
        data: Vec<u8>,
    }

    #[test]
    fn test_device_path_encoding_contexts_derive_together() {
        let record = VendorRecord {
            kind: 0x0102,
            guid: efi::Guid::from_bytes(&[7; 16]),
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
