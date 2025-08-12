//! UEFI PE/COFF Support Library
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
extern crate alloc;

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use scroll::{LE, Pread, Pwrite};

pub mod error;
pub mod relocation;
mod resource_directory;

#[allow(unused_imports)]
pub use goblin::pe::section_table::IMAGE_SCN_CNT_CODE;

use relocation::{RelocationBlock, parse_relocation_blocks};
use resource_directory::{DataEntry, Directory, DirectoryEntry, DirectoryString};

// Magic value for TE header.
const TE_MAGIC: u16 = 0x5A56;
// Magic value for PE32 header.
const PE32_MAGIC: u16 = 0x5A4D;
// The size of the PE32 signature.
const SIZEOF_PE32_SIGNATURE: usize = 4;
// The size of the COFF header.
const SIZEOF_COFF_HEADER: usize = 20;
// The offset from the start the TE header, that the image base is located at.
const TE_IMAGE_BASE_HEADER_FIELD_OFFSET: usize = 16;
// The size of the standard fields in the PE32Plus header.
const SIZEOF_STANDARD_FIELDS_64: usize = 24;

// Relocation type that does not require any action.
const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
// Relocation type that requires the adjustment be applied to the entire
// 32-bit value.
const IMAGE_REL_BASED_HIGHLOW: u16 = 3;
// Relocation type that requires the adjustment be applied to the entire
// 64-bit value.
const IMAGE_REL_BASED_DIR64: u16 = 10;

/// Enum representing the type of header in a PE32 image.
#[derive(Debug, Default, Clone, PartialEq)]
pub enum HeaderType {
    Te(usize),
    #[default]
    Pe,
}

/// Type containing information about a PE32 image.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UefiPeInfo {
    /// Type of header (PE32 or TE)
    pub header_type: HeaderType,
    /// Offset into an image header where the image_base address is located.
    /// NOT the actual image base address.
    pub image_base_header_field_offset: usize,
    /// RVA offset of the entry point.
    pub entry_point_offset: usize,
    /// The subsystem type (IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER \[0xB\], etc.).
    pub image_type: u16,
    /// The total length of the image.
    pub size_of_image: u32,
    /// The size of an individual section in a power of 2 (4K \[0x1000\], etc.).
    pub section_alignment: u32,
    /// The total length of the image header.
    pub size_of_headers: usize,
    /// Structs representing the section table inside the image header.
    pub sections: Vec<goblin::pe::section_table::SectionTable>,
    /// The filename, if present, from debug_data
    pub filename: Option<String>,
    /// The relocation directory, if present.
    pub reloc_dir: Option<goblin::pe::data_directories::DataDirectory>,
    /// Whether the NX_COMPAT DLL Characteristic flag is set
    pub nx_compat: bool,
}

impl UefiPeInfo {
    pub fn parse(bytes: &[u8]) -> error::Result<Self> {
        match scroll::Pread::gread_with::<u16>(bytes, &mut 0, scroll::LE)? {
            PE32_MAGIC => UefiPeInfo::from_pe(bytes),
            TE_MAGIC => UefiPeInfo::from_te(bytes),
            sig => Err(error::Error::BadSignature(sig)),
        }
    }

    /// Parses a PE with a TE header, gathering the necessary data for operating on the image in a UEFI environment.
    fn from_te(bytes: &[u8]) -> error::Result<Self> {
        let mut pe = UefiPeInfo::default();
        let parsed_te = goblin::pe::TE::parse(bytes)?;

        // Set the simple fields.
        pe.image_base_header_field_offset = TE_IMAGE_BASE_HEADER_FIELD_OFFSET;
        pe.header_type = HeaderType::Te(parsed_te.rva_offset);
        pe.entry_point_offset = parsed_te.header.entry_point as usize;
        pe.image_type = parsed_te.header.subsystem as u16;
        pe.section_alignment = 0;
        pe.size_of_headers = parsed_te.header.base_of_code as usize;
        pe.sections = parsed_te.sections;
        // TE doesn't have the optional header with DLL Characteristics, so we have to assume the image is NX_COMPAT
        pe.nx_compat = true;

        // TE headers always have a reloc dir, even if it's empty
        // unlike PE32 headers.
        if parsed_te.header.reloc_dir.size != 0 {
            pe.reloc_dir = Some(parsed_te.header.reloc_dir);
        }

        // TE headers don't have a size of image filed like PE32 headers
        // so it needs to be calculated.
        if let Some(last_section) = pe.sections.last() {
            pe.size_of_image = last_section.virtual_address + last_section.virtual_size;

            // Parse the filename from the debug data if it exists.
            if let Some(codeview_data) = &parsed_te.debug_data.codeview_pdb70_debug_info {
                pe.filename = UefiPeInfo::read_filename(codeview_data.filename)?;
            };

            Ok(pe)
        } else {
            Err(error::Error::Goblin(goblin::error::Error::Malformed("No sections found in PE.".to_string())))
        }
    }

    /// Parses a PE image with a PE32 header, gathering the necessary data for operating on the image in a UEFI environment.
    fn from_pe(bytes: &[u8]) -> error::Result<Self> {
        let mut pe = UefiPeInfo::default();

        // Parse the PE header and verify the optional header exists
        let parsed_pe = goblin::pe::PE::parse(bytes)?;
        let optional_header = parsed_pe.header.optional_header.ok_or(error::Error::NoOptionalHeader)?;

        // Set the simple fields
        pe.header_type = HeaderType::Pe;
        pe.entry_point_offset = optional_header.standard_fields.address_of_entry_point as usize;
        pe.image_type = optional_header.windows_fields.subsystem;
        pe.section_alignment = optional_header.windows_fields.section_alignment;
        pe.size_of_image = optional_header.windows_fields.size_of_image;
        pe.sections = parsed_pe.sections.into_iter().collect();
        pe.size_of_headers = optional_header.windows_fields.size_of_headers as usize;
        pe.nx_compat = optional_header.windows_fields.dll_characteristics
            & goblin::pe::dll_characteristic::IMAGE_DLLCHARACTERISTICS_NX_COMPAT
            != 0;

        // Set the relocation diretory if it exists
        if let Some(reloc_section) = optional_header.data_directories.get_base_relocation_table() {
            pe.reloc_dir = Some(*reloc_section);
        }

        // Calculate the image base offset by finding the offset of the windows fields
        // image_base is the first entry in the windows_fields
        let mut windows_fields_offset = parsed_pe.header.dos_header.pe_pointer;
        windows_fields_offset += SIZEOF_COFF_HEADER as u32;
        windows_fields_offset += SIZEOF_PE32_SIGNATURE as u32;
        windows_fields_offset += SIZEOF_STANDARD_FIELDS_64 as u32;
        pe.image_base_header_field_offset = windows_fields_offset as usize;

        // Get the filename if the data exists
        if let Some(debug_data) = parsed_pe.debug_data {
            if let Some(codeview_data) = debug_data.codeview_pdb70_debug_info {
                pe.filename = UefiPeInfo::read_filename(codeview_data.filename)?;
            } else if let Some(codeview_data) = debug_data.codeview_pdb20_debug_info {
                pe.filename = UefiPeInfo::read_filename(codeview_data.filename)?;
            }
        }
        Ok(pe)
    }

    /// Parses a bytes buffer containing the filename.
    fn read_filename(bytes: &[u8]) -> error::Result<Option<String>> {
        let filename_end = bytes.iter().position(|&c| c == b'\0').unwrap_or(bytes.len());
        let mut filename = String::from_utf8_lossy(&bytes[0..filename_end]).into_owned();

        if filename.ends_with(".pdb") || filename.ends_with(".dll") {
            filename.truncate(filename.len() - 4);
        }

        if let Some(index) = filename.rfind(|ref c| ['/', '\\'].contains(c)) {
            filename.drain(..index + 1);
        }

        Ok(Some(format!("{}.efi", filename)))
    }
}

/// Attempts to load the image into the specified bytes buffer.
///
/// Copies the provided image, section by section, into the zero'd out buffer after copying the
/// headers, returning an error if it failed.
///
/// ## Errors
///
/// Returns [`Parse`](error::Error::Parse) error if parsing a image containing a TE header
/// failed.
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing a image containing a PE32 header
/// failed. Contains the exact parsing [`Error`](goblin::error::Error).
///
/// Returns [`BufferTooShort`](error::Error::BufferTooShort) error if either of the buffers provided are
/// not large enough to contain the image as specified by the image header.
///
/// ## Panics
///
/// Panics if the loaded_image buffer is not the same length as the image.
pub fn load_image(pe_info: &UefiPeInfo, image: &[u8], loaded_image: &mut [u8]) -> error::Result<()> {
    loaded_image.fill(0);

    let size_of_headers = pe_info.size_of_headers;
    let dst =
        loaded_image.get_mut(..size_of_headers).ok_or(error::Error::BufferTooShort(size_of_headers, "loaded_image"))?;
    let src = image.get(..size_of_headers).ok_or(error::Error::BufferTooShort(size_of_headers, "image"))?;
    dst.copy_from_slice(src);

    for section in &pe_info.sections {
        let mut size = section.virtual_size;
        if size == 0 || size > section.size_of_raw_data {
            size = section.size_of_raw_data;
        }

        let dst = loaded_image
            .get_mut((section.virtual_address as usize)..(section.virtual_address as usize + size as usize))
            .ok_or(error::Error::BufferTooShort(size as usize, "loaded_image"))?;
        let src = image
            .get((section.pointer_to_raw_data as usize)..(section.pointer_to_raw_data as usize + size as usize))
            .ok_or(error::Error::BufferTooShort(size as usize, "image"))?;
        dst.copy_from_slice(src)
    }
    Ok(())
}

/// Attempts to relocate the image to the specified destination.
///
/// Relocates the already loaded image to the destination address, applying
/// all relocation fixups, returning an error if it failed.
///
/// ## Errors
///
/// Returns [`Parse`](error::Error::Parse) error if parsing a image containing a TE header
/// failed.
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing a image containing a PE32 header
/// failed. Contains the exact parsing [`Error`](goblin::error::Error).
///
/// Returns [`BufferTooShort`](error::Error::BufferTooShort) error if either of the buffers provided are
/// not large enough to contain the image as specified by the image header.
pub fn relocate_image(
    pe_info: &UefiPeInfo,
    destination: usize,
    image: &mut [u8],
    prev_reloc_blocks: &[relocation::RelocationBlock],
) -> error::Result<Vec<RelocationBlock>> {
    let rva_offset = match pe_info.header_type {
        HeaderType::Te(rva_offset) => rva_offset,
        HeaderType::Pe => 0,
    };

    // Read original image base for future relocations, then update it.
    let base = image.pread_with::<u64>(pe_info.image_base_header_field_offset, LE)?;
    image.pwrite_with::<u64>(destination as u64 - rva_offset as u64, pe_info.image_base_header_field_offset, LE)?;

    let adjustment = (destination as u64).wrapping_sub(base + rva_offset as u64);

    if adjustment == 0 || pe_info.reloc_dir.is_none() {
        return Ok(Vec::new());
    }

    let dir = pe_info.reloc_dir.expect("Reloc Dir was not None above.");
    let relocation_data = image
        .get((dir.virtual_address as usize)..(dir.virtual_address as usize + dir.size as usize))
        .ok_or(error::Error::BufferTooShort(dir.size as usize, "image"))?;

    let mut relocation_block = parse_relocation_blocks(relocation_data)?;
    assert!(prev_reloc_blocks.is_empty() || relocation_block.len() == prev_reloc_blocks.len());
    for (block_idx, reloc_block) in relocation_block.iter_mut().enumerate() {
        for (reloc_idx, reloc) in reloc_block.relocations.iter_mut().enumerate() {
            let fixup_type = reloc.type_and_offset >> 12;
            let fixup =
                reloc_block.block_header.page_rva as usize + (reloc.type_and_offset & 0xFFF) as usize - rva_offset;

            match fixup_type {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_HIGHLOW => {
                    let value = image.pread_with::<u32>(fixup, LE)?;
                    image.pwrite_with(value.wrapping_add(adjustment as u32), fixup, LE)?;
                }
                IMAGE_REL_BASED_DIR64 => {
                    let mut value = image.pread_with::<u64>(fixup, LE)?;
                    image.pwrite_with(value.wrapping_add(adjustment), fixup, LE)?;

                    if !prev_reloc_blocks.is_empty()
                        && prev_reloc_blocks[block_idx].relocations[reloc_idx].value != value
                    {
                        continue;
                    }

                    value = value.wrapping_add(adjustment);
                    reloc.value = value;

                    let subslice = image.get_mut(fixup..fixup + 8).ok_or(error::Error::BufferTooShort(8, "image"))?;
                    subslice.copy_from_slice(&value.to_le_bytes()[..]);
                }
                _ => todo!(), // Other fixups not implemented at this time
            }
        }
    }
    Ok(relocation_block)
}

/// Attempts to load the HII resource section data for a given PE32 image.
///
/// Extracts the HII resource section data from the provided image, returning None
/// if the image does not contain the HII resource section.
///
/// ## Errors
///
/// Returns [`Parse`](crate::error::Error::Parse) error if parsing a image containing a TE header
/// failed.
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing a image containing a PE32 header
/// failed. Contains the exact parsing [`Error`](goblin::error::Error).
pub fn load_resource_section(pe_info: &UefiPeInfo, image: &[u8]) -> error::Result<Option<(usize, usize)>> {
    for section in &pe_info.sections {
        if String::from_utf8_lossy(&section.name).trim_end_matches('\0') == ".rsrc" {
            let mut size = section.virtual_size;
            if size == 0 || size > section.size_of_raw_data {
                size = section.size_of_raw_data;
            }

            let start = section.pointer_to_raw_data as usize;
            let end = match section.pointer_to_raw_data.checked_add(size) {
                Some(offset) => offset as usize,
                None => {
                    return Err(error::Error::Goblin(goblin::error::Error::Malformed(String::from(
                        "HII resource section size is invalid",
                    ))));
                }
            };
            let resource_section = image
                .get(start..end)
                .ok_or(error::Error::Goblin(goblin::error::Error::BufferTooShort(end - start, "bytes")))?;
            let mut directory: Directory = resource_section.pread(0)?;

            let mut offset = directory.size_in_bytes();

            if offset > size as usize {
                return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(offset, "bytes")));
            }

            let mut directory_entry: DirectoryEntry = resource_section.pread(core::mem::size_of::<Directory>())?;

            for _ in 0..directory.number_of_named_entries {
                if directory_entry.name_is_string() {
                    if directory_entry.name_offset() >= size {
                        return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                            directory_entry.name_offset() as usize,
                            "bytes",
                        )));
                    }

                    let resource_directory_string =
                        resource_section.pread::<DirectoryString>(directory_entry.name_offset() as usize)?;

                    let name_start_offset = (directory_entry.name_offset() + 1) as usize;
                    let name_end_offset = name_start_offset + (resource_directory_string.length * 2) as usize;
                    let string_val = resource_section
                        .get(name_start_offset..name_end_offset)
                        .ok_or(error::Error::Goblin(goblin::error::Error::BufferTooShort(name_end_offset, "bytes")))?;

                    // L"HII" = [0x0, 0x48, 0x0, 0x49, 0x0, 0x49]
                    if resource_directory_string.length == 3 && string_val == [0x0, 0x48, 0x0, 0x49, 0x0, 0x49] {
                        if directory_entry.data_is_directory() {
                            if directory_entry.offset_to_directory() > size {
                                return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                                    directory_entry.offset_to_directory() as usize,
                                    "bytes",
                                )));
                            }

                            directory = resource_section.pread(directory_entry.offset_to_directory() as usize)?;
                            offset = (directory_entry.offset_to_directory() as usize) + directory.size_in_bytes();

                            if offset > size as usize {
                                return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                                    offset, "bytes",
                                )));
                            }

                            directory_entry = resource_section.pread(
                                (directory_entry.offset_to_directory() as usize) + core::mem::size_of::<Directory>(),
                            )?;

                            if directory_entry.data_is_directory() {
                                if directory_entry.offset_to_directory() > size {
                                    return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                                        directory_entry.offset_to_directory() as usize,
                                        "bytes",
                                    )));
                                }

                                directory = resource_section.pread(directory_entry.offset_to_directory() as usize)?;

                                offset = (directory_entry.offset_to_directory() as usize) + directory.size_in_bytes();

                                if offset > size as usize {
                                    return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                                        offset, "bytes",
                                    )));
                                }

                                directory_entry = resource_section.pread(
                                    (directory_entry.offset_to_directory() as usize)
                                        + core::mem::size_of::<Directory>(),
                                )?;
                            }
                        }

                        if !directory_entry.data_is_directory() {
                            if directory_entry.data >= size {
                                return Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(
                                    directory_entry.data as usize,
                                    "bytes",
                                )));
                            }

                            let resource_data_entry: DataEntry =
                                resource_section.pread(directory_entry.data as usize)?;
                            return Ok(Some((
                                resource_data_entry.offset_to_data as usize,
                                resource_data_entry.size as usize,
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    extern crate std;

    use std::vec;

    #[test]
    fn test_image_bad_signature() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let mut image = *image;
        image.as_mut()[0] = 00;
        let result = UefiPeInfo::parse(&image);
        assert!(result.is_err());
        assert_eq!(format!("{:?}", result.unwrap_err()), "BadSignature(23040)");
    }

    #[test]
    fn te_image_info_should_be_correct() {
        let image = include_bytes!("../resources/test/te/test_image.te");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert_eq!(image_info.image_type, 11);
        assert_eq!(image_info.section_alignment, 0x0);
        assert_eq!(image_info.filename, Some(String::from("RustTerseImageTestDxe.efi")));
        assert_eq!(image_info.size_of_image, 0x5ef8);
        assert_eq!(image_info.entry_point_offset, 0x10a8);
    }

    #[test]
    fn pe_image_info_should_be_correct() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert_eq!(image_info.image_type, 0x0B);
        assert_eq!(image_info.section_alignment, 0x1000);
        assert_eq!(image_info.filename, Some(String::from("RustFfiTestDxe.efi")));
        assert_eq!(image_info.size_of_image, 0x14000);
        assert_eq!(image_info.entry_point_offset, 0x11B8);
    }

    #[test]
    fn te_load_image_should_load_the_image() {
        let image = include_bytes!("../resources/test/te/test_image.te");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];
        assert_eq!(loaded_image.len(), image_info.size_of_image as usize);
        load_image(&image_info, image, &mut loaded_image).unwrap();

        let loaded_image_reference = include_bytes!("../resources/test/te/test_image_loaded.bin");

        assert_eq!(loaded_image.len(), loaded_image_reference.len());
        let first_mismatch = loaded_image.iter().enumerate().find(|(idx, byte)| &&loaded_image_reference[*idx] != byte);

        assert!(first_mismatch.is_none(), "First mismatch at index {:x}", first_mismatch.unwrap().0);
    }

    #[test]
    fn te_load_image_should_have_same_info() {
        let image = include_bytes!("../resources/test/te/test_image_with_reloc_section.te");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();

        let loaded_image_info = UefiPeInfo::parse(&loaded_image).unwrap();

        assert_eq!(image_info, loaded_image_info);
    }

    #[test]
    fn pe_load_image_should_load_the_image() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut loaded_image).unwrap();
        assert_eq!(loaded_image.len(), image_info.size_of_image as usize);

        let loaded_image_reference = include_bytes!("../resources/test/pe32/test_image_loaded.bin");
        assert_eq!(loaded_image.len(), loaded_image_reference.len());

        let first_mismatch = loaded_image.iter().enumerate().find(|(idx, byte)| &&loaded_image_reference[*idx] != byte);
        assert!(first_mismatch.is_none(), "loaded image mismatch at idx: {:#x?}", first_mismatch.unwrap());
    }

    #[test]
    fn pe_load_image_should_have_same_image_info() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let mut image_info = UefiPeInfo::parse(image).unwrap();

        let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut loaded_image).unwrap();
        let loaded_image_info = UefiPeInfo::parse(&loaded_image).unwrap();

        //debug information is not included when loading an image in the present implementation, so filename will not be present.
        image_info.filename = None;
        assert_eq!(image_info, loaded_image_info);
    }

    #[test]
    fn test_load_image_with_bad_image_too_short() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let pe_info = UefiPeInfo::parse(image).unwrap();
        let edit_image = &image[0..image.len() - 0x1000];

        let mut loaded_image: Vec<u8> = vec![0; pe_info.size_of_image as usize];
        match load_image(&pe_info, edit_image, &mut loaded_image) {
            Err(error::Error::BufferTooShort(..)) => {}
            Ok(_) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {:?}", e),
        }
    }

    #[test]
    fn te_relocate_image_with_reloc_sections_should_work() {
        let image = include_bytes!("../resources/test/te/test_image_with_reloc_section.te");
        let reference_image = include_bytes!("../resources/test/te/test_image_with_reloc_section_relocated.bin");

        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut relocated_image: Vec<u8> = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut relocated_image).unwrap();
        relocate_image(&image_info, 0x7CC5_8000, &mut relocated_image, &Vec::new()).unwrap();

        assert_eq!(relocated_image.len(), reference_image.len());
        let first_mismatch = relocated_image.iter().enumerate().find(|(idx, byte)| &&reference_image[*idx] != byte);
        assert!(first_mismatch.is_none(), "First mismatch at index {:x}", first_mismatch.unwrap().0);
    }

    #[test]
    fn te_relocate_to_same_address_should_do_nothing() {
        let image1 = include_bytes!("../resources/test/te/test_image_with_reloc_section.te");

        let image_info = UefiPeInfo::parse(image1).unwrap();

        let mut relocated_once = vec![0; image_info.size_of_image as usize];
        let mut relocated_twice = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image1, &mut relocated_once).unwrap();
        load_image(&image_info, image1, &mut relocated_twice).unwrap();

        let blocks = relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_once, &Vec::new()).unwrap();
        let blocks = relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_twice, &blocks).unwrap();
        relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_twice, &blocks).unwrap();

        assert_eq!(relocated_once, relocated_twice);
    }

    #[test]
    fn pe_relocate_image_should_relocate_the_image() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut relocated_image: Vec<u8> = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut relocated_image).unwrap();

        relocate_image(&image_info, 0x04158000, &mut relocated_image, &Vec::new()).unwrap();

        // the reference "test_image_relocated.bin" was generated by calling pe32_load_image and pe32_relocate_image
        // to generate a loaded image buffer and then dumping ito a file. This ensures that future changes to the code
        // that case load to change unexpectedly will fail to match.
        let relocated_image_reference = include_bytes!("../resources/test/pe32/test_image_relocated.bin");
        let first_mismatch =
            relocated_image.iter().enumerate().find(|(idx, byte)| &&relocated_image_reference[*idx] != byte);

        assert!(first_mismatch.is_none(), "relocated image mismatch at idx: {:#x?}", first_mismatch.unwrap());
    }

    #[test]
    fn pe_relocate_image_should_work_multiple_times() {
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut relocated_image: Vec<u8> = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut relocated_image).unwrap();

        let blocks = relocate_image(&image_info, 0x04158000, &mut relocated_image, &Vec::new()).unwrap();

        let mut reclocated_image_copy = relocated_image.clone();

        let blocks = relocate_image(&image_info, 0x80000415, &mut reclocated_image_copy, &blocks).unwrap();

        assert_ne!(relocated_image, reclocated_image_copy);

        relocate_image(&image_info, 0x04158000, &mut reclocated_image_copy, &blocks).unwrap();

        assert_eq!(relocated_image, reclocated_image_copy);
    }

    #[test]
    fn test_relocate_image_with_missing_reloc_dir() {
        let image = include_bytes!("../resources/test/te/test_image_with_reloc_section.te");
        let image_info = UefiPeInfo::parse(image).unwrap();
        let mut loaded_image = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();

        // Cut the image short at the reloc dir
        let reloc_addr = image_info.reloc_dir.unwrap().virtual_address;
        match relocate_image(&image_info, 0x04158000, &mut loaded_image[0..(reloc_addr + 1) as usize], &Vec::new()) {
            Err(error::Error::BufferTooShort(..)) => {}
            Ok(_) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {:?}", e),
        }
    }

    #[test]
    fn pe_load_resource_section_should_succeed() {
        // test_image_<toolchain>_hii.pe32 file is just a copy of TftpDynamicCommand.efi module copied and renamed.
        // the HII resource section layout slightly varies between Linux (GCC) and Windows (MSVC) bulids so both are
        // tested here.
        let test_msvc_image_buffer = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let test_msvc_image_info = UefiPeInfo::parse(test_msvc_image_buffer).unwrap();
        let mut test_msvc_loaded_image: Vec<u8> = vec![0; test_msvc_image_info.size_of_image as usize];
        load_image(&test_msvc_image_info, test_msvc_image_buffer, &mut test_msvc_loaded_image).unwrap();
        assert_eq!(test_msvc_loaded_image.len(), test_msvc_image_info.size_of_image as usize);

        let test_file_gcc_image = include_bytes!("../resources/test/pe32/test_image_gcc_hii.pe32");
        let test_gcc_image_info = UefiPeInfo::parse(test_file_gcc_image).unwrap();
        let mut test_gcc_loaded_image: Vec<u8> = vec![0; test_gcc_image_info.size_of_image as usize];
        load_image(&test_gcc_image_info, test_file_gcc_image, &mut test_gcc_loaded_image).unwrap();
        assert_eq!(test_gcc_loaded_image.len(), test_gcc_image_info.size_of_image as usize);

        let ref_file = include_bytes!("../resources/test/pe32/test_image_hii_section.bin");

        let msvc_result = load_resource_section(&test_msvc_image_info, test_msvc_image_buffer).unwrap();
        assert!(msvc_result.is_some());
        let (msvc_resource_section_offset, msvc_resource_section_size) = msvc_result.unwrap();
        assert_eq!(msvc_resource_section_size, ref_file.len());
        assert_eq!(
            &test_msvc_loaded_image
                [msvc_resource_section_offset..(msvc_resource_section_offset + msvc_resource_section_size)],
            ref_file
        );

        let gcc_result = load_resource_section(&test_gcc_image_info, test_file_gcc_image).unwrap();
        assert!(gcc_result.is_some());
        let (gcc_resource_section_offset, gcc_resource_section_size) = gcc_result.unwrap();
        assert_eq!(gcc_resource_section_size, ref_file.len());
        assert_eq!(
            &test_gcc_loaded_image
                [gcc_resource_section_offset..(gcc_resource_section_offset + gcc_resource_section_size)],
            ref_file
        );
    }

    #[test]
    fn te_load_resource_section_should_succeed() {
        let image = include_bytes!("../resources/test/te/test_image.te");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut loaded_image: Vec<u8> = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();

        let result = load_resource_section(&image_info, image).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_resource_section_using_size_of_raw_data() {
        const RELOC_DIR_ENTRY_INDEX: usize = 5;
        let image = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let mut image_info = UefiPeInfo::parse(image).unwrap();

        // Invalidate virtual size, backflow to size_of_raw_data
        image_info.sections[RELOC_DIR_ENTRY_INDEX].virtual_size = 0;
        assert!(load_resource_section(&image_info, image).is_ok())
    }

    #[test]
    fn test_load_resource_section_with_malformed_resource_dir() {
        const RELOC_DIR_ENTRY_INDEX: usize = 5;
        let image = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        // Set pointer_to_raw_data to a value that can overflow, failing checked add
        let mut image_info2 = image_info.clone();
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].pointer_to_raw_data = u32::MAX;
        match load_resource_section(&image_info2, image) {
            Err(error::Error::Goblin(goblin::error::Error::Malformed(..))) => {}
            Ok(_) => panic!("Expected Malformed error"),
            Err(e) => panic!("Expected Malformed error, got {:?}", e),
        }

        // set size_of_raw_data to a value outside the buffer, causing a buffer too short error
        let mut image_info2 = image_info.clone();
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].virtual_size = 0;
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].size_of_raw_data = image_info2.size_of_image;
        match load_resource_section(&image_info2, image) {
            Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(..))) => {}
            Ok(_) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {:?}", e),
        }
    }
}
