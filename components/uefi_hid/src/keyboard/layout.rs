//! HII Keyboard Layout support.
//!
//! Provides UEFI HII Keyboard Layout structures and serialization.
//! Folded in from the `HiiKeyboardLayout` crate in mu_plus.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::{format, string::String, vec, vec::Vec};
use core::mem;

use num_enum::TryFromPrimitive;

use r_efi::{
    efi,
    hii::{self, PACKAGE_END},
    protocols::hii_database::*,
};
use scroll::{Pread, Pwrite, ctx};

/// GUID for default keyboard layout.
pub const DEFAULT_KEYBOARD_LAYOUT_GUID: efi::Guid =
    efi::Guid::from_fields(0x3a4d7a7c, 0x18a, 0x4b42, 0x81, 0xb3, &[0xdc, 0x10, 0xe3, 0xb5, 0x91, 0xbd]);

/// HII Keyboard Package List
/// Refer to UEFI spec version 2.10 section 33.3.1.2 which defines the generic header structure. This implementation
/// only supports HII Keyboard Packages; other HII package types (or mixes) are not supported.
#[derive(Debug, PartialEq, Eq)]
pub struct HiiKeyboardPkgList {
    /// The GUID associated with this package list.
    pub package_list_guid: efi::Guid,
    /// The HiiKeyboardPkg contained in this package list.
    pub package: HiiKeyboardPkg,
}

/// HII Keyboard Package
/// Refer to UEFI spec version 2.10 section 33.3.9 which defines the keyboard package structure.
#[derive(Debug, PartialEq, Eq)]
pub struct HiiKeyboardPkg {
    /// The list of keyboard layouts in this package.
    pub layouts: Vec<HiiKeyboardLayout>,
}

/// HII Keyboard Layout
/// Refer to UEFI spec version 2.10 section 34.8.10 which defines the keyboard layout structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiiKeyboardLayout {
    /// The unique ID associated with this keyboard layout.
    pub guid: efi::Guid,
    /// A list of key descriptors
    pub keys: Vec<HiiKey>,
    /// A list of descriptions for this keyboard layout.
    pub descriptions: Vec<HiiKeyboardDescription>,
}

/// HII Key descriptor
/// Refer to UEFI spec version 2.10 section 34.10.10 which defines the key descriptor structure.
#[derive(Debug, Pread, Pwrite, PartialEq, Eq, Clone, Copy)]
#[repr(C)]
pub struct HiiKeyDescriptor {
    /// Describes the physical key on the keyboard.
    pub key: EfiKey,
    /// Unicode character for the key (note: UEFI only supports UCS-2 encoding).
    pub unicode: u16,
    /// Unicode character for the key with the shift key being held down.
    pub shifted_unicode: u16,
    /// Unicode character for the key with the Alt-GR being held down.
    pub alt_gr_unicode: u16,
    /// Unicode character for the key with the Alt-GR and shift keys being held down.
    pub shifted_alt_gr_unicode: u16,
    /// Modifier keys are defined to allow for special functionality that is not necessarily accomplished by a printable
    /// character. Many of these modifier keys are flags to toggle certain state bits on and off inside of a keyboard
    /// driver. See [`r_efi::protocols::hii_database`] for modifier definitions.
    pub modifier: u16,
    /// Indicates what modifiers affect this key. See [`r_efi::protocols::hii_database`] for "affected by" definitions.
    pub affected_attribute: u16,
}

/// Non-Spacing HII Key Descriptor variant. Used for "non-spacing" keys.
/// Refer to discussion in UEFI spec version 2.10 section 33.2.4.3 for information on "non-spacing" keys and how they
/// are used.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct HiiNsKeyDescriptor {
    /// The descriptor for the "non-spacing key" itself.
    pub descriptor: HiiKeyDescriptor,
    /// The list of descriptors that are active if the "non-spacing" key has been pressed.
    pub dependent_keys: Vec<HiiKeyDescriptor>,
}

/// HII Key descriptor enumeration.
/// HII spec allows for two types of key descriptors - normal and "non-spacing".
/// Refer to UEFI spec version 2.10 section 33.2.4.3
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiiKey {
    Key(HiiKeyDescriptor),
    NsKey(HiiNsKeyDescriptor),
}

/// Enumeration of physical keys.
/// Refer to UEFI spec version 2.10 section 34.8.10 and section 33.2.4.1.
#[rustfmt::skip]
#[derive(Debug, Clone, Copy, PartialEq, Eq, TryFromPrimitive)]
//Note: UEFI specifies this as an C enum. That means the size is a bit ambiguous; but most compilers
//will make it 32-bit, so that's what this implementation assumes.
#[repr(u32)]
pub enum EfiKey {
  LCtrl = 0, A0 = 1, LAlt = 2, SpaceBar = 3, A2 = 4, A3 = 5, A4 = 6, RCtrl = 7, LeftArrow = 8, DownArrow = 9,
  RightArrow = 10, Zero = 11, Period = 12, Enter = 13, LShift = 14, B0 = 15, B1 = 16, B2 = 17, B3 = 18, B4 = 19,
  B5 = 20, B6 = 21, B7 = 22, B8 = 23, B9 = 24, B10 = 25, RShift = 26, UpArrow = 27, One = 28, Two = 29, Three = 30,
  CapsLock = 31, C1 = 32, C2 = 33, C3 = 34, C4 = 35, C5 = 36, C6 = 37, C7 = 38, C8 = 39, C9 = 40, C10 = 41,
  C11 = 42, C12 = 43, Four = 44, Five = 45, Six = 46, Plus = 47, Tab = 48, D1 = 49, D2 = 50, D3 = 51, D4 = 52,
  D5 = 53, D6 = 54, D7 = 55, D8 = 56, D9 = 57, D10 = 58, D11 = 59, D12 = 60, D13 = 61, Del = 62, End = 63,
  PgDn = 64, Seven = 65, Eight = 66, Nine = 67, E0 = 68, E1 = 69, E2 = 70, E3 = 71, E4 = 72, E5 = 73, E6 = 74,
  E7 = 75, E8 = 76, E9 = 77, E10 = 78, E11 = 79, E12 = 80, BackSpace = 81, Ins = 82, Home = 83, PgUp = 84,
  NLck = 85, Slash = 86, Asterisk = 87, Minus = 88, Esc = 89, F1 = 90, F2 = 91, F3 = 92, F4 = 93, F5 = 94,
  F6 = 95, F7 = 96, F8 = 97, F9 = 98, F10 = 99, F11 = 100, F12 = 101, Print = 102, SLck = 103, Pause = 104,
  Intl0 = 105, Intl1 = 106, Intl2 = 107, Intl3 = 108, Intl4 = 109, Intl5 = 110, Intl6 = 111, Intl7 = 112,
  Intl8 = 113, Intl9 = 114,
}

/// Description for a keyboard layout.
/// Refer to UEFI spec version 2.10 section 34.8.10
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiiKeyboardDescription {
    /// The language code for the description (e.g. "en-US")
    pub language: String,
    /// The description (e.g. "English Keyboard")
    pub description: String,
}

fn gread_guid(src: &[u8], offset: &mut usize) -> Result<efi::Guid, scroll::Error> {
    Ok(efi::Guid::from_fields(
        src.gread(offset)?,
        src.gread(offset)?,
        src.gread(offset)?,
        src.gread(offset)?,
        src.gread(offset)?,
        src.gread_with::<&[u8]>(offset, 6)?
            .try_into()
            .map_err(|_| scroll::Error::BadInput { size: 0, msg: "GUID node6 must be 6 bytes" })?,
    ))
}

impl ctx::TryFromCtx<'_> for HiiKeyboardPkgList {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], _ctx: ()) -> Result<(Self, usize), Self::Error> {
        //Note: This is not a general purpose HII package list reader: it only supports a package list with a single
        //keyboard layout package in it.
        let offset = &mut 0;
        //EFI_HII_PACKAGE_LIST_HEADER::PackageListGuid
        let guid = gread_guid(src, offset)?;
        //EFI_HII_PACKAGE_LIST_HEADER::PackageLength
        let _package_length: u32 = src.gread(offset)?;

        //Read HiiKeyboard Pkg
        let hii_keyboard_pkg: HiiKeyboardPkg = src.gread(offset)?;

        //Read EFI_HHI_PACKAGE_END package
        let _pkg_end_length_type: u32 = src.gread(offset)?;

        Ok((HiiKeyboardPkgList { package_list_guid: guid, package: hii_keyboard_pkg }, *offset))
    }
}

impl ctx::TryIntoCtx for &HiiKeyboardPkgList {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: ()) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        //EFI_HII_PACKAGE_LIST_HEADER::PackageListGuid
        dest.gwrite(&self.package_list_guid.as_bytes()[..], offset)?;

        //EFI_HII_PACKAGE_LIST_HEADER::PackageLength will be updated at the end.
        let mut package_length_offset = *offset;
        *offset += 4;

        //Write HiiKeyboardPkg
        dest.gwrite(&self.package, offset)?;

        //EFI_HII_PACKAGE_END
        let length_type: u32 = 4 | ((PACKAGE_END as u32) << 24);
        dest.gwrite(length_type, offset)?;

        //go back and update EFI_HII_PACKAGE_LIST_HEADER::PackageLength
        dest.gwrite(*offset as u32, &mut package_length_offset)?;

        Ok(*offset)
    }
}

impl ctx::TryFromCtx<'_> for HiiKeyboardPkg {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], ctx: ()) -> Result<(Self, usize), Self::Error> {
        let offset = &mut 0;
        //EFI_HII_KEYBOARD_PACKAGE_HDR::Header (bitfield as single u32)
        let length_type: u32 = src.gread(offset)?;
        let pkg_type = (length_type >> 24) as u8;
        if pkg_type != hii::PACKAGE_KEYBOARD_LAYOUT {
            return Err(scroll::Error::BadInput { size: 0, msg: "Unsupported Pkg Type" });
        }
        //EFI_HII_KEYBOARD_PACKAGE_HDR::LayoutCount
        let layout_count: u16 = src.gread(offset)?;

        //EFI_HII_KEYBOARD_PACKAGE_HDR::Layout[] array into vector.
        let mut layouts = Vec::with_capacity(layout_count as usize);
        for _ in 0..layout_count {
            layouts.push(src.gread_with(offset, ctx)?);
        }

        Ok((HiiKeyboardPkg { layouts }, *offset))
    }
}

impl ctx::TryIntoCtx for &HiiKeyboardPkg {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: ()) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        //EFI_HII_KEYBOARD_PKG_HDR::Header::Length will be updated at the end.
        *offset += 4;
        //EFI_HII_KEYBOARD_PKG_HDR::LayoutCount
        dest.gwrite(self.layouts.len() as u16, offset)?;
        //EFI_HII_KEYBOARD_PKG_HDR::Layout[]
        for layout in &self.layouts {
            dest.gwrite(layout, offset)?;
        }
        //update EFI_HII_KEYBOARD_PKG_HEADER at offset zero.
        let length = *offset;
        let length_type: u32 = (hii::PACKAGE_KEYBOARD_LAYOUT as u32) << 24;
        let length_type = length_type | (length & 0xFFFFFF) as u32;
        dest.gwrite(length_type, &mut 0)?;

        Ok(*offset)
    }
}

impl ctx::TryFromCtx<'_> for HiiKeyboardLayout {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], _ctx: ()) -> Result<(Self, usize), Self::Error> {
        let offset = &mut 0;
        //EFI_HII_KEYBOARD_LAYOUT::LayoutLength
        let _layout_length: u16 = src.gread(offset)?;
        //EFI_HII_KEYBOARD_LAYOUT::Guid
        let guid = gread_guid(src, offset)?;
        //EFI_HII_KEYBOARD_LAYOUT::LayoutDescriptorStringOffset
        let layout_descriptor_string_offset: u32 = src.gread(offset)?;
        //EFI_HII_KEYBOARD_LAYOUT::DescriptorCount
        let descriptor_count: u8 = src.gread(offset)?;

        //EFI_HII_KEYBOARD_LAYOUT::Descriptors[] array into vector. Note: descriptor_count is not used for iteration
        //since ns_keys may consume multiple descriptors which are included in the count, resulting in a vector of "real"
        //descriptors that is smaller than the descriptor_count.
        let descriptor_start = *offset;
        let mut descriptors = vec![];
        while *offset < layout_descriptor_string_offset as usize {
            descriptors.push(src.gread(offset)?);
        }
        let parsed_count = (*offset - descriptor_start) / mem::size_of::<HiiKeyDescriptor>();
        if parsed_count != descriptor_count as usize {
            return Err(scroll::Error::BadInput {
                size: 0,
                msg: "parsed descriptor byte count does not match declared descriptor_count",
            });
        }

        //EFI_DESCRIPTION_STRING_BUNDLE::DescriptionCount
        let description_count: u16 = src.gread(offset)?;
        let mut descriptions = Vec::with_capacity(description_count as usize);
        //EFI_DESCRIPTION_STRING_BUNDLE::DescriptionString[]
        for _ in 0..description_count {
            descriptions.push(src.gread(offset)?);
        }

        Ok((HiiKeyboardLayout { guid, keys: descriptors, descriptions }, *offset))
    }
}

impl ctx::TryIntoCtx for &HiiKeyboardLayout {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: ()) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        //EFI_HII_KEYBOARD_LAYOUT::LayoutLength will be updated at the end.
        *offset += 2;
        //EFI_HII_KEYBOARD_LAYOUT::Guid
        dest.gwrite(&self.guid.as_bytes()[..], offset)?;
        //EFI_HII_KEYBOARD_LAYOUT::LayoutDescriptorStringOffset will be updated after writing out the descriptors.
        let mut descriptor_string_offset = *offset;
        *offset += 4;

        //EFI_HII_KEYBOARD_LAYOUT::DescriptorCount will be updated after writing out the descriptors.
        let mut descriptor_count_offset = *offset;
        *offset += 1;

        let descriptor_start = *offset;
        //EFI_HII_KEYBOARD_LAYOUT::Descriptors[]
        for descriptor in &self.keys {
            //Note: may expand to more than one descriptor due to non-spacing keys.
            dest.gwrite(descriptor, offset)?;
        }

        //Go back and update EFI_HII_KEYBOARD_LAYOUT::DescriptorCount
        let descriptor_count = (*offset - descriptor_start) / mem::size_of::<HiiKeyDescriptor>();
        dest.gwrite(descriptor_count as u8, &mut descriptor_count_offset)?;

        //Go back and update EFI_HII_KEYBOARD_LAYOUT::LayoutDescriptorStringOffset.
        dest.gwrite(*offset as u32, &mut descriptor_string_offset)?;

        //EFI_DESCRIPTION_STRING_BUNDLE::DescriptionCount
        dest.gwrite(self.descriptions.len() as u16, offset)?;

        //EFI_DESCRIPTION_STRING_BUNDLE::DescriptionString[]
        for description in &self.descriptions {
            dest.gwrite(description, offset)?;
        }

        //Go back and update EFI_HII_KEYBOARD_LAYOUT::LayoutLength
        dest.gwrite(*offset as u16, &mut 0)?;

        Ok(*offset)
    }
}

impl ctx::TryFromCtx<'_> for HiiKey {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], _ctx: ()) -> Result<(Self, usize), Self::Error> {
        let offset = &mut 0;
        let descriptor: HiiKeyDescriptor = src.gread(offset)?;
        if descriptor.modifier == NS_KEY_MODIFIER {
            //For Non-Spacing keys, consume descriptors until we find one without EFI_NS_KEY_DEPENDENCY_MODIFIER or run out.
            //Refer to UEFI spec 2.10 section 33.2.4.3 for details.
            let mut dependent_keys = vec![];
            while let Ok(dependent_key) = src.pread::<HiiKeyDescriptor>(*offset) {
                if dependent_key.modifier == NS_KEY_DEPENDENCY_MODIFIER {
                    //found a dependent descriptor. Re-read it with gread to update offset.
                    dependent_keys.push(src.gread(offset)?);
                } else {
                    //found a descriptor without EFI_NS_KEY_DEPENDENCY_MODIFIER
                    break;
                }
            }
            Ok((HiiKey::NsKey(HiiNsKeyDescriptor { descriptor, dependent_keys }), *offset))
        } else {
            Ok((HiiKey::Key(descriptor), *offset))
        }
    }
}

impl ctx::TryIntoCtx for &HiiKey {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: ()) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        match self {
            HiiKey::Key(descriptor) => {
                dest.gwrite(descriptor, offset)?;
            }
            HiiKey::NsKey(ns_descriptor) => {
                dest.gwrite(&ns_descriptor.descriptor, offset)?;
                for descriptor in &ns_descriptor.dependent_keys {
                    dest.gwrite(descriptor, offset)?;
                }
            }
        }
        Ok(*offset)
    }
}

impl ctx::TryFromCtx<'_> for HiiKeyboardDescription {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], _ctx: ()) -> Result<(Self, usize), Self::Error> {
        let offset = &mut 0;
        //consume u16 characters until NULL.
        let mut desc_chars = vec![];
        loop {
            let desc_char: u16 = src.gread(offset)?;
            if desc_char == 0 {
                break;
            }
            desc_chars.push(desc_char);
        }
        //convert to string. Note: UEFI spec uses UCS-2 encoding, so all valid inputs should translate to UTF-16 without
        //error.
        let desc_string = String::from_utf16(&desc_chars)
            .map_err(|_| scroll::Error::BadInput { size: 0, msg: "Invalid string in keyboard description." })?;

        //split the resulting string on the first space - this gives us language and description.
        if let Some((lang, desc)) = desc_string.split_once(' ') {
            Ok((HiiKeyboardDescription { language: String::from(lang), description: String::from(desc) }, *offset))
        } else {
            Err(scroll::Error::BadInput { size: 0, msg: "No space in keyboard description." })
        }
    }
}

impl ctx::TryIntoCtx for &HiiKeyboardDescription {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: ()) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        //Format as EFI_DESCRIPTION_STRING per UEFI spec 2.10 section 34.8.10.
        let desc_string = format!("{} {}", self.language, self.description);
        let mut characters: Vec<u16> = desc_string.encode_utf16().collect();
        characters.push(0);
        for character in characters {
            dest.gwrite(character, offset)?;
        }
        Ok(*offset)
    }
}

impl ctx::TryFromCtx<'_, scroll::Endian> for EfiKey {
    type Error = scroll::Error;
    fn try_from_ctx(src: &'_ [u8], _ctx: scroll::Endian) -> Result<(Self, usize), Self::Error> {
        let offset = &mut 0;
        let efi_key = EfiKey::try_from(src.gread::<u32>(offset)?)
            .map_err(|_| scroll::Error::BadInput { size: 0, msg: "Invalid EfiKey enum value" })?;
        Ok((efi_key, *offset))
    }
}

impl ctx::TryIntoCtx<scroll::Endian> for &EfiKey {
    type Error = scroll::Error;
    fn try_into_ctx(self, dest: &mut [u8], _ctx: scroll::Endian) -> Result<usize, Self::Error> {
        let offset = &mut 0;
        dest.gwrite(*self as u32, offset)?;
        Ok(*offset)
    }
}

// Convenience macro for defining HiiKey::Key structures.
macro_rules! key {
    ($key:expr, $unicode:literal, $shifted:literal, $alt_gr:literal, $shifted_alt_gr:literal, $modifier:expr, $affected:expr ) => {
        HiiKey::Key(key_descriptor!($key, $unicode, $shifted, $alt_gr, $shifted_alt_gr, $modifier, $affected))
    };
}

// convenience macro for defining HiiKeyDescriptor structures.
// note: for unicode characters, these are encoded as u16 for compliance with UEFI spec. UEFI only supports UCS-2
// encoding - so unicode characters that require more than two bytes under UTF-16 are not supported (and will panic).
macro_rules! key_descriptor {
    ($key:expr, $unicode:literal, $shifted:literal, $alt_gr:literal, $shifted_alt_gr:literal, $modifier:expr, $affected:expr ) => {
        HiiKeyDescriptor {
            key: $key,
            unicode: $unicode.encode_utf16(&mut [0u16; 1])[0],
            shifted_unicode: $shifted.encode_utf16(&mut [0u16; 1])[0],
            alt_gr_unicode: $alt_gr.encode_utf16(&mut [0u16; 1])[0],
            shifted_alt_gr_unicode: $shifted_alt_gr.encode_utf16(&mut [0u16; 1])[0],
            modifier: $modifier,
            affected_attribute: $affected,
        }
    };
}

/// Returns a default HiiKeyboardLayout (which is a standard US-104 layout)
#[rustfmt::skip]
pub fn get_default_keyboard_layout() -> HiiKeyboardLayout {
  HiiKeyboardLayout {
    guid: DEFAULT_KEYBOARD_LAYOUT_GUID,
    keys: vec![
      key!(EfiKey::C1,          'a',    'A',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B5,          'b',    'B',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B3,          'c',    'C',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C3,          'd',    'D',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D3,          'e',    'E',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C4,          'f',    'F',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C5,          'g',    'G',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C6,          'h',    'H',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D8,          'i',    'I',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C7,          'j',    'J',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C8,          'k',    'K',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C9,          'l',    'L',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B7,          'm',    'M',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B6,          'n',    'N',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D9,          'o',    'O',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D10,         'p',    'P',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D1,          'q',    'Q',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D4,          'r',    'R',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::C2,          's',    'S',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D5,          't',    'T',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D7,          'u',    'U',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B4,          'v',    'V',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D2,          'w',    'W',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B2,          'x',    'X',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::D6,          'y',    'Y',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::B1,          'z',    'Z',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_CAPS_LOCK),
      key!(EfiKey::E1,          '1',    '!',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E2,          '2',    '@',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E3,          '3',    '#',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E4,          '4',    '$',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E5,          '5',    '%',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E6,          '6',    '^',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E7,          '7',    '&',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E8,          '8',    '*',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E9,          '9',    '(',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E10,         '0',    ')',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::Enter,       '\x0d', '\x0d', '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Esc,         '\x1b', '\x1b', '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::BackSpace,   '\x08', '\x08', '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Tab,         '\x09', '\x09', '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::SpaceBar,    ' ',    ' ',    '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::E11,         '-',    '_',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E12,         '=',    '+',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::D11,         '[',    '{',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::D12,         ']',    '}',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::D13,         '\\',   '|',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::C12,         '\\',   '|',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::C10,         ';',    ':',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::C11,         '\'',   '"',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::E0,          '`',    '~',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::B8,          ',',    '<',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::B9,          '.',    '>',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::B10,         '/',    '?',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT                        ),
      key!(EfiKey::CapsLock,    '\0',   '\0',   '\0',   '\0',   CAPS_LOCK_MODIFIER,           0                                                 ),
      key!(EfiKey::F1,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_ONE_MODIFIER,    0                                                 ),
      key!(EfiKey::F2,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_TWO_MODIFIER,    0                                                 ),
      key!(EfiKey::F3,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_THREE_MODIFIER,  0                                                 ),
      key!(EfiKey::F4,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_FOUR_MODIFIER,   0                                                 ),
      key!(EfiKey::F5,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_FIVE_MODIFIER,   0                                                 ),
      key!(EfiKey::F6,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_SIX_MODIFIER,    0                                                 ),
      key!(EfiKey::F7,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_SEVEN_MODIFIER,  0                                                 ),
      key!(EfiKey::F8,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_EIGHT_MODIFIER,  0                                                 ),
      key!(EfiKey::F9,          '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_NINE_MODIFIER,   0                                                 ),
      key!(EfiKey::F10,         '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_TEN_MODIFIER,    0                                                 ),
      key!(EfiKey::F11,         '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_ELEVEN_MODIFIER, 0                                                 ),
      key!(EfiKey::F12,         '\0',   '\0',   '\0',   '\0',   FUNCTION_KEY_TWELVE_MODIFIER, 0                                                 ),
      key!(EfiKey::Print,       '\0',   '\0',   '\0',   '\0',   PRINT_MODIFIER,               0                                                 ),
      key!(EfiKey::SLck,        '\0',   '\0',   '\0',   '\0',   SCROLL_LOCK_MODIFIER,         0                                                 ),
      key!(EfiKey::Pause,       '\0',   '\0',   '\0',   '\0',   PAUSE_MODIFIER,               0                                                 ),
      key!(EfiKey::Ins,         '\0',   '\0',   '\0',   '\0',   INSERT_MODIFIER,              0                                                 ),
      key!(EfiKey::Home,        '\0',   '\0',   '\0',   '\0',   HOME_MODIFIER,                0                                                 ),
      key!(EfiKey::PgUp,        '\0',   '\0',   '\0',   '\0',   PAGE_UP_MODIFIER,             0                                                 ),
      key!(EfiKey::Del,         '\0',   '\0',   '\0',   '\0',   DELETE_MODIFIER,              0                                                 ),
      key!(EfiKey::End,         '\0',   '\0',   '\0',   '\0',   END_MODIFIER,                 0                                                 ),
      key!(EfiKey::PgDn,        '\0',   '\0',   '\0',   '\0',   PAGE_DOWN_MODIFIER,           0                                                 ),
      key!(EfiKey::RightArrow,  '\0',   '\0',   '\0',   '\0',   RIGHT_ARROW_MODIFIER,         0                                                 ),
      key!(EfiKey::LeftArrow,   '\0',   '\0',   '\0',   '\0',   LEFT_ARROW_MODIFIER,          0                                                 ),
      key!(EfiKey::DownArrow,   '\0',   '\0',   '\0',   '\0',   DOWN_ARROW_MODIFIER,          0                                                 ),
      key!(EfiKey::UpArrow,     '\0',   '\0',   '\0',   '\0',   UP_ARROW_MODIFIER,            0                                                 ),
      key!(EfiKey::NLck,        '\0',   '\0',   '\0',   '\0',   NUM_LOCK_MODIFIER,            0                                                 ),
      key!(EfiKey::Slash,       '/',    '/',    '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Asterisk,    '*',    '*',    '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Minus,       '-',    '-',    '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Plus,        '+',    '+',    '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::Enter,       '\x0d', '\x0d', '\0',   '\0',   NULL_MODIFIER,                0                                                 ),
      key!(EfiKey::One,         '1',    '1',    '\0',   '\0',   END_MODIFIER,                 AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Two,         '2',    '2',    '\0',   '\0',   DOWN_ARROW_MODIFIER,          AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Three,       '3',    '3',    '\0',   '\0',   PAGE_DOWN_MODIFIER,           AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Four,        '4',    '4',    '\0',   '\0',   LEFT_ARROW_MODIFIER,          AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Five,        '5',    '5',    '\0',   '\0',   NULL_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Six,         '6',    '6',    '\0',   '\0',   RIGHT_ARROW_MODIFIER,         AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Seven,       '7',    '7',    '\0',   '\0',   HOME_MODIFIER,                AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Eight,       '8',    '8',    '\0',   '\0',   UP_ARROW_MODIFIER,            AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Nine,        '9',    '9',    '\0',   '\0',   PAGE_UP_MODIFIER,             AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Zero,        '0',    '0',    '\0',   '\0',   INSERT_MODIFIER,              AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::Period,      '.',    '.',    '\0',   '\0',   DELETE_MODIFIER,              AFFECTED_BY_STANDARD_SHIFT | AFFECTED_BY_NUM_LOCK ),
      key!(EfiKey::A4,          '\0',   '\0',   '\0',   '\0',   MENU_MODIFIER,                0                                                 ),
      key!(EfiKey::LCtrl,       '\0',   '\0',   '\0',   '\0',   LEFT_CONTROL_MODIFIER,        0                                                 ),
      key!(EfiKey::LShift,      '\0',   '\0',   '\0',   '\0',   LEFT_SHIFT_MODIFIER,          0                                                 ),
      key!(EfiKey::LAlt,        '\0',   '\0',   '\0',   '\0',   LEFT_ALT_MODIFIER,            0                                                 ),
      key!(EfiKey::A0,          '\0',   '\0',   '\0',   '\0',   LEFT_LOGO_MODIFIER,           0                                                 ),
      key!(EfiKey::RCtrl,       '\0',   '\0',   '\0',   '\0',   RIGHT_CONTROL_MODIFIER,       0                                                 ),
      key!(EfiKey::RShift,      '\0',   '\0',   '\0',   '\0',   RIGHT_SHIFT_MODIFIER,         0                                                 ),
      key!(EfiKey::A2,          '\0',   '\0',   '\0',   '\0',   RIGHT_ALT_MODIFIER,           0                                                 ),
      key!(EfiKey::A3,          '\0',   '\0',   '\0',   '\0',   RIGHT_LOGO_MODIFIER,          0                                                 ),
    ],
    descriptions: vec![
      HiiKeyboardDescription {
        language: String::from("en-US"),
        description: String::from("English Keyboard")
      }
    ]
  }
}

/// Returns a default keyboard layout package.
pub fn get_default_keyboard_pkg() -> HiiKeyboardPkg {
    HiiKeyboardPkg { layouts: vec![get_default_keyboard_layout()] }
}

/// Returns a default keyboard layout package list.
pub fn get_default_keyboard_pkg_list() -> HiiKeyboardPkgList {
    HiiKeyboardPkgList {
        package_list_guid: efi::Guid::from_fields(
            0xc0f3b43,
            0x44de,
            0x4907,
            0xb4,
            0x78,
            &[0x22, 0x5f, 0x6f, 0x62, 0x89, 0xdc],
        ),
        package: get_default_keyboard_pkg(),
    }
}

/// Returns a default keyboard layout package list as a byte vector.
///
/// This is suitable for use with [`r_efi::protocols::hii_database::ProtocolNewPackageList`].
///
/// # Panics
/// Panics if the built-in default keyboard layout fails to serialize. This should never occur
/// as the layout is a compile-time constant.
pub fn get_default_keyboard_pkg_list_buffer() -> Vec<u8> {
    // Upper bound: ~86 keys × 16 bytes + headers + descriptions. Buffer is resized to actual size after writing.
    let mut buffer = vec![0u8; 4096];

    let result = buffer.pwrite(&get_default_keyboard_pkg_list(), 0);
    if let Ok(buffer_size) = result {
        buffer.resize(buffer_size, 0);
        buffer
    } else {
        panic!("Unexpected error serializing HII Keyboard Package List: {:?}", result);
    }
}

/// Defines errors that can occur while parsing keyboard layout.
#[derive(Debug)]
pub enum LayoutError {
    /// Malformed key buffer
    ParseError(scroll::Error),
}

/// Returns a HiiKeyboardLayout structure parsed from the given buffer.
pub fn keyboard_layout_from_buffer(buffer: &[u8]) -> Result<HiiKeyboardLayout, LayoutError> {
    let layout = buffer.pread::<HiiKeyboardLayout>(0).map_err(LayoutError::ParseError)?;
    log::trace!(
        "keyboard_layout_from_buffer: parsed layout with {:?} keys, {:?} descriptions",
        layout.keys.len(),
        layout.descriptions.len(),
    );
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use scroll::{Pread, Pwrite};

    use super::*;

    #[test]
    fn hii_keyboard_package_serialize_deserialize_should_produce_consistent_results() {
        let mut buffer = [0u8; 4096];

        let package = get_default_keyboard_pkg();
        buffer.pwrite(&package, 0).unwrap();

        let package2: HiiKeyboardPkg = buffer.pread(0).unwrap();
        assert_eq!(package, package2);
    }

    #[test]
    fn efi_key_from_u32_valid_values() {
        assert_eq!(EfiKey::try_from(0u32), Ok(EfiKey::LCtrl));
        assert_eq!(EfiKey::try_from(32u32), Ok(EfiKey::C1));
        assert_eq!(EfiKey::try_from(114u32), Ok(EfiKey::Intl9));
    }

    #[test]
    fn efi_key_from_u32_invalid_value() {
        assert!(EfiKey::try_from(115u32).is_err());
        assert!(EfiKey::try_from(u32::MAX).is_err());
    }

    #[test]
    fn efi_key_roundtrip_through_u32() {
        for val in 0..=114u32 {
            let key = EfiKey::try_from(val).unwrap();
            assert_eq!(key as u32, val);
        }
    }

    #[test]
    fn hii_keyboard_pkg_list_roundtrip() {
        let pkg_list = get_default_keyboard_pkg_list();
        let buffer = get_default_keyboard_pkg_list_buffer();
        let parsed: HiiKeyboardPkgList = buffer.pread(0).unwrap();
        assert_eq!(pkg_list, parsed);
    }

    #[test]
    fn keyboard_layout_from_buffer_valid() {
        let layout = get_default_keyboard_layout();
        let mut buffer = [0u8; 4096];
        let size = buffer.pwrite(&layout, 0).unwrap();
        let parsed = keyboard_layout_from_buffer(&buffer[..size]).unwrap();
        assert_eq!(layout, parsed);
    }

    #[test]
    fn keyboard_layout_from_buffer_empty_fails() {
        let result = keyboard_layout_from_buffer(&[]);
        assert!(matches!(result, Err(LayoutError::ParseError(_))));
    }

    #[test]
    fn keyboard_layout_from_buffer_truncated_fails() {
        let result = keyboard_layout_from_buffer(&[0u8; 4]);
        assert!(matches!(result, Err(LayoutError::ParseError(_))));
    }

    #[test]
    fn default_layout_has_expected_key_count() {
        let layout = get_default_keyboard_layout();
        // US-104 default layout: 105 key entries (includes numpad duplicates and modifier keys)
        assert_eq!(layout.keys.len(), 105);
    }

    #[test]
    fn default_layout_has_description() {
        let layout = get_default_keyboard_layout();
        assert_eq!(layout.descriptions.len(), 1);
        assert_eq!(layout.descriptions[0].language, "en-US");
    }

    #[test]
    fn hii_keyboard_pkg_invalid_type_fails() {
        let mut buffer = [0u8; 64];
        // Write a package header with wrong type (type = 0x01 instead of PACKAGE_KEYBOARD_LAYOUT)
        let bad_length_type: u32 = 0x01_000006; // type=1, length=6
        buffer.pwrite(bad_length_type, 0).unwrap();
        let result = buffer.pread::<HiiKeyboardPkg>(0);
        assert!(result.is_err());
    }
}
