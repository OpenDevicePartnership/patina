//! UEFI PE/COFF Support Library
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use patina::log_debug_assert;
use scroll::{LE, Pread, Pwrite};

pub mod error;
pub mod relocation;
mod resource_directory;

#[allow(unused_imports)]
pub use goblin::pe::section_table::IMAGE_SCN_CNT_CODE;

use relocation::{RelocationBlock, parse_relocation_blocks};
use resource_directory::{DataEntry, Directory, DirectoryString};

// Magic value for PE32 header.
const PE32_MAGIC: u16 = 0x5A4D;
// The size of the PE32 signature.
const SIZEOF_PE32_SIGNATURE: usize = 4;
// The size of the COFF header.
const SIZEOF_COFF_HEADER: usize = 20;
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
const HII_RESOURCE_NAME: [u16; 3] = [b'H' as u16, b'I' as u16, b'I' as u16];

/// PE/COFF Specification Machine Type for `IMAGE_FILE_MACHINE_AMD64` images.
#[allow(dead_code)]
pub const IMAGE_MACHINE_TYPE_X64: u16 = 0x8664;
/// PE/COFF Specification Machine Type for `IMAGE_FILE_MACHINE_ARM64` images.
#[allow(dead_code)]
pub const IMAGE_MACHINE_TYPE_AARCH64: u16 = 0xAA64;

/// Type containing information about a PE32 image.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UefiPeInfo {
    /// Offset into an image header where the `image_base` address is located.
    /// NOT the actual image base address.
    pub image_base_header_field_offset: usize,
    /// RVA offset of the entry point.
    pub entry_point_offset: usize,
    /// The subsystem type (`IMAGE_SUBSYSTEM_EFI_BOOT_SERVICE_DRIVER` \[0xB\], etc.).
    pub image_type: u16,
    /// The total length of the image.
    pub size_of_image: u32,
    /// The size of an individual section in a power of 2 (4K \[0x1000\], etc.).
    pub section_alignment: u32,
    /// The total length of the image header.
    pub size_of_headers: usize,
    /// The COFF machine type (`IMAGE_FILE_MACHINE`_*).
    pub machine: u16,
    /// Structs representing the section table inside the image header.
    pub sections: Vec<goblin::pe::section_table::SectionTable>,
    /// The filename, if present, from `debug_data`
    pub filename: Option<String>,
    /// The relocation directory, if present.
    pub reloc_dir: Option<goblin::pe::data_directories::DataDirectory>,
    /// Whether the `NX_COMPAT` DLL Characteristic flag is set
    pub nx_compat: bool,
}

impl UefiPeInfo {
    /// Parses a PE32+ image, gathering the necessary data for operating on the image in a UEFI environment.
    ///
    /// Terse Executable (TE) images are not supported: they do not carry a section alignment, which Patina requires
    /// to be a multiple of 4 KB in order to apply image memory protections.
    pub fn parse(bytes: &[u8]) -> error::Result<Self> {
        match scroll::Pread::gread_with::<u16>(bytes, &mut 0, scroll::LE)? {
            PE32_MAGIC => UefiPeInfo::from_pe(bytes, &goblin::pe::options::ParseOptions::default()),
            sig => Err(error::Error::BadSignature(sig)),
        }
    }

    /// Parses a PE image that has already been loaded into memory (sections at virtual addresses).
    ///
    /// This method uses `resolve_rva: false` so that goblin treats RVAs as direct byte offsets into
    /// the slice. This is useful for images where sections have been mapped to their virtual addresses.
    ///
    /// TE images are not supported, see [`UefiPeInfo::parse`].
    pub fn parse_mapped(bytes: &[u8]) -> error::Result<Self> {
        let mut opts = goblin::pe::options::ParseOptions::default();
        opts.resolve_rva = false;
        match scroll::Pread::gread_with::<u16>(bytes, &mut 0, scroll::LE)? {
            PE32_MAGIC => UefiPeInfo::from_pe(bytes, &opts),
            sig => Err(error::Error::BadSignature(sig)),
        }
    }

    /// Verifies that every section lies entirely within the loaded image, that is, within `size_of_image`.
    ///
    /// A section occupies `VirtualAddress..VirtualAddress + VirtualSize` of the loaded image, with `VirtualSize`
    /// rounded up to the section alignment as it is when memory protections are applied. Sections are not required
    /// to be contiguous, so the sum of the section sizes may be smaller than `size_of_image`; each section only has
    /// to end within it. The end is computed in 64 bits so that the 32-bit header fields cannot wrap.
    ///
    /// ## Errors
    ///
    /// Returns [`BufferTooShort`](error::Error::BufferTooShort) if any section extends past the end of the image, or
    /// if its `VirtualSize` cannot be aligned to the section alignment.
    pub fn validate_sections(&self) -> error::Result<()> {
        let image_size = u64::from(self.size_of_image);
        for section in &self.sections {
            let aligned_virtual_size = patina::align_up(section.virtual_size, self.section_alignment)
                .map_err(|_| error::Error::BufferTooShort(section.virtual_size as usize, "loaded_image"))?;

            let end = u64::from(section.virtual_address) + u64::from(aligned_virtual_size);
            if end > image_size {
                return Err(error::Error::BufferTooShort(usize::try_from(end).unwrap_or(usize::MAX), "loaded_image"));
            }
        }
        Ok(())
    }

    /// Parses a PE image with a PE32 header, gathering the necessary data for operating on the image in a UEFI environment.
    fn from_pe(bytes: &[u8], opts: &goblin::pe::options::ParseOptions) -> error::Result<Self> {
        let mut pe = UefiPeInfo::default();
        let parsed_header = goblin::pe::header::Header::parse(bytes)?;
        let optional_header = parsed_header.optional_header.ok_or(error::Error::NoOptionalHeader)?;

        let mut section_table_offset = parsed_header.dos_header.pe_pointer as usize
            + SIZEOF_PE32_SIGNATURE
            + SIZEOF_COFF_HEADER
            + parsed_header.coff_header.size_of_optional_header as usize;

        let sections = parsed_header.coff_header.sections(bytes, &mut section_table_offset)?;

        // Set the simple fields
        pe.entry_point_offset = optional_header.standard_fields.address_of_entry_point as usize;
        pe.image_type = optional_header.windows_fields.subsystem;
        pe.machine = parsed_header.coff_header.machine;
        pe.section_alignment = optional_header.windows_fields.section_alignment;
        pe.size_of_image = optional_header.windows_fields.size_of_image;
        pe.sections = sections;
        pe.size_of_headers = optional_header.windows_fields.size_of_headers as usize;
        pe.nx_compat = optional_header.windows_fields.dll_characteristics
            & goblin::pe::dll_characteristic::IMAGE_DLLCHARACTERISTICS_NX_COMPAT
            != 0;

        // Set the relocation diretory if it exists
        if let Some(&reloc_section) = optional_header.data_directories.get_base_relocation_table() {
            pe.reloc_dir = Some(reloc_section);
        }

        // Calculate the image base offset by finding the offset of the windows fields
        // image_base is the first entry in the windows_fields
        let mut windows_fields_offset = parsed_header.dos_header.pe_pointer;
        windows_fields_offset += SIZEOF_COFF_HEADER as u32;
        windows_fields_offset += SIZEOF_PE32_SIGNATURE as u32;
        windows_fields_offset += SIZEOF_STANDARD_FIELDS_64 as u32;
        pe.image_base_header_field_offset = windows_fields_offset as usize;

        // Debug data is optional metadata. Use it when valid, but do not reject an otherwise
        // loadable image when its debug directory cannot be resolved or parsed.
        if let Some(&debug_table) = optional_header.data_directories.get_debug_table() {
            match goblin::pe::debug::DebugData::parse_with_opts(
                bytes,
                debug_table,
                &pe.sections,
                optional_header.windows_fields.file_alignment,
                opts,
            ) {
                Ok(debug_data) => {
                    if let Some(codeview_data) = debug_data.codeview_pdb70_debug_info {
                        pe.filename = UefiPeInfo::read_filename(codeview_data.filename)?;
                    } else if let Some(codeview_data) = debug_data.codeview_pdb20_debug_info {
                        pe.filename = UefiPeInfo::read_filename(codeview_data.filename)?;
                    }
                }
                Err(err) => {
                    log::warn!("Failed to parse optional PE/COFF debug metadata: {err:?}");
                }
            }
        }
        Ok(pe)
    }

    /// Parses a bytes buffer containing the filename.
    fn read_filename(bytes: &[u8]) -> error::Result<Option<String>> {
        let filename_end = bytes.iter().position(|&c| c == b'\0').unwrap_or(bytes.len());
        let mut filename = String::from_utf8_lossy(bytes.get(..filename_end).unwrap_or(bytes)).into_owned();

        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        // TODO: Determine whether case insensitivity is required here.
        if filename.ends_with(".pdb") || filename.ends_with(".dll") {
            filename.truncate(filename.len() - 4);
        }

        if let Some(index) = filename.rfind(|ref c| ['/', '\\'].contains(c)) {
            filename.drain(..=index);
        }

        Ok(Some(format!("{filename}.efi")))
    }

    /// Returns the filename of the image, or the provided default if not available.
    #[inline(always)]
    pub fn filename_or<'a, 'b>(&'a self, default: &'b str) -> &'b str
    where
        'a: 'b,
    {
        self.filename.as_deref().unwrap_or(default)
    }
}

/// Attempts to load the image into the specified bytes buffer.
///
/// Copies the provided image, section by section, into the zero'd out buffer after copying the
/// headers, returning an error if it failed.
///
/// ## Errors
///
/// Returns [`Parse`](error::Error::Parse) error if reading data from the image failed.
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing the PE32 header
/// failed. Contains the exact parsing [`Error`](goblin::error::Error).
///
/// Returns [`BufferTooShort`](error::Error::BufferTooShort) error if either of the buffers provided are
/// not large enough to contain the image as specified by the image header.
///
/// ## Panics
///
/// Panics if the `loaded_image` buffer is not the same length as the image.
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
        dst.copy_from_slice(src);
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
/// Returns [`Parse`](error::Error::Parse) error if reading data from the image failed.
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing the PE32 header
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
    // Read original image base for future relocations, then update it.
    let base = image.pread_with::<u64>(pe_info.image_base_header_field_offset, LE)?;
    image.pwrite_with::<u64>(destination as u64, pe_info.image_base_header_field_offset, LE)?;

    let adjustment = (destination as u64).wrapping_sub(base);

    if adjustment == 0 || pe_info.reloc_dir.is_none() {
        return Ok(Vec::new());
    }

    let dir = pe_info.reloc_dir.expect("Reloc Dir was not None above.");
    let relocation_data = image
        .get((dir.virtual_address as usize)..(dir.virtual_address as usize + dir.size as usize))
        .ok_or(error::Error::BufferTooShort(dir.size as usize, "image"))?;

    let mut relocation_block = parse_relocation_blocks(relocation_data)?;
    if !prev_reloc_blocks.is_empty() && relocation_block.len() != prev_reloc_blocks.len() {
        log_debug_assert!("Previous relocation blocks length does not match current relocation blocks length.");
        return Err(error::Error::RelocationBlockLengthMismatch);
    }
    for (block_idx, reloc_block) in relocation_block.iter_mut().enumerate() {
        for (reloc_idx, reloc) in reloc_block.relocations.iter_mut().enumerate() {
            let fixup_type = reloc.type_and_offset >> 12;
            let fixup = reloc_block.block_header.page_rva as usize + (reloc.type_and_offset & 0xFFF) as usize;

            match fixup_type {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_HIGHLOW => {
                    let value = image.pread_with::<u32>(fixup, LE)?;
                    image.pwrite_with(value.wrapping_add(adjustment as u32), fixup, LE)?;
                }
                IMAGE_REL_BASED_DIR64 => {
                    let mut value = image.pread_with::<u64>(fixup, LE)?;
                    image.pwrite_with(value.wrapping_add(adjustment), fixup, LE)?;

                    if !prev_reloc_blocks.is_empty() {
                        let prev_block =
                            prev_reloc_blocks.get(block_idx).ok_or(error::Error::RelocationBlockLengthMismatch)?;
                        let prev_reloc =
                            prev_block.relocations.get(reloc_idx).ok_or(error::Error::RelocationBlockLengthMismatch)?;
                        if prev_reloc.value != value {
                            continue;
                        }
                    }

                    value = value.wrapping_add(adjustment);
                    reloc.value = value;

                    let subslice = image.get_mut(fixup..fixup + 8).ok_or(error::Error::BufferTooShort(8, "image"))?;
                    subslice.copy_from_slice(&value.to_le_bytes()[..]);
                }
                _ => return Err(error::Error::UnsupportedRelocation(fixup_type)),
            }
        }
    }
    Ok(relocation_block)
}

/// Converts a vector of relocation blocks into a flat buffer suitable for use in the runtime protocol.
///
/// ## Errors
///
/// Returns [`UnsupportedRelocation`](error::Error::UnsupportedRelocation) if a block contains a
/// relocation type this loader does not support.
pub fn flatten_runtime_relocation_data(relocation_data: &[RelocationBlock]) -> error::Result<&'static mut [u8]> {
    // The runtime protocol expects linearly appended values, determine how much space
    // is needed to store this.

    let mut size = 0;
    for block in relocation_data {
        for reloc in &block.relocations {
            let fixup_type = reloc.type_and_offset >> 12;
            size += match fixup_type {
                IMAGE_REL_BASED_ABSOLUTE => 0,
                IMAGE_REL_BASED_HIGHLOW => core::mem::size_of::<u32>(),
                IMAGE_REL_BASED_DIR64 => core::mem::size_of::<u64>(),
                // If this is reached, an unsupported relocation type might need to be
                // supported or there is a bug.
                _ => return Err(error::Error::UnsupportedRelocation(fixup_type)),
            }
        }
    }

    let mut flat_data = Vec::with_capacity_in(size, &crate::allocator::EFI_RUNTIME_SERVICES_DATA_ALLOCATOR);
    for block in relocation_data {
        for reloc in &block.relocations {
            let fixup_type = reloc.type_and_offset >> 12;
            match fixup_type {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_HIGHLOW => {
                    flat_data.extend_from_slice(&(reloc.value as u32).to_le_bytes());
                }
                IMAGE_REL_BASED_DIR64 => {
                    flat_data.extend_from_slice(&reloc.value.to_le_bytes());
                }
                // Since the size-calculation pass above returns `Err` for unsupported types
                // over this same slice before this loop runs, this should be unreachable.
                _ => unreachable!("unsupported relocation types are rejected by the size-calculation pass above"),
            }
        }
    }

    // Double check that the capacity calculation was correct.
    debug_assert!(flat_data.capacity() == size && flat_data.len() == size);
    Ok(flat_data.leak())
}

/// Attempts to load the HII resource section data for a given PE32 image.
///
/// Extracts the HII resource section data from the provided image, returning None
/// if the image does not contain the HII resource section.
///
/// ## Errors
///
/// Returns [`Goblin`](error::Error::Goblin) error if parsing a image containing a PE32 header
/// failed. Contains the exact parsing [`Error`](goblin::error::Error).
///
/// Returns [`BufferTooShort`](error::Error::BufferTooShort) if a resource directory structure extends beyond the
/// resource section.
pub fn load_resource_section(pe_info: &UefiPeInfo, image: &[u8]) -> error::Result<Option<(usize, usize)>> {
    let Some(resource_section) = get_section(".rsrc", pe_info, image)? else {
        return Ok(None);
    };
    let directory = Directory::parse(resource_section)?;

    for directory_entry in directory.named_entries() {
        let mut directory_entry = directory_entry;

        if directory_entry.name_is_string() {
            let resource_directory_string =
                DirectoryString::parse(resource_section_at(resource_section, directory_entry.name_offset())?)?;

            if resource_directory_string.matches(&HII_RESOURCE_NAME) {
                // We found the HII resource section. The actual resource data can be located up to two levels deep in
                // subdirectories.
                for _ in 0..2 {
                    if !directory_entry.data_is_directory() {
                        break;
                    }
                    let directory = Directory::parse(resource_section_at(
                        resource_section,
                        directory_entry.offset_to_directory(),
                    )?)?;
                    let Some(entry) = directory.first_entry() else {
                        break;
                    };
                    directory_entry = entry;
                }

                if !directory_entry.data_is_directory() {
                    let resource_data_entry =
                        DataEntry::parse(resource_section_at(resource_section, directory_entry.data_offset())?)?;
                    return Ok(Some((
                        resource_data_entry.offset_to_data() as usize,
                        resource_data_entry.size() as usize,
                    )));
                }
            }
        }
    }
    Ok(None)
}

/// Returns the resource section bytes beginning at `offset`.
fn resource_section_at(resource_section: &[u8], offset: u32) -> error::Result<&[u8]> {
    let offset = offset as usize;
    resource_section.get(offset..).ok_or(error::Error::BufferTooShort(offset, "resource section"))
}

/// Returns a slice of the provided image containing the data in the requested section, if it exists.
///
/// ## Errors
///
/// Returns [Goblin(Malformed)](error::Error::Goblin) error if the section size is invalid
/// Returns [Goblin(BufferTooShort)](error::Error::Goblin) error if the section is not fully contained in the image
pub fn get_section<'a>(target: &str, pe_info: &UefiPeInfo, image: &'a [u8]) -> error::Result<Option<&'a [u8]>> {
    let target_bytes = target.as_bytes();
    for section in &pe_info.sections {
        let section_name_bytes = section.name.split(|&b| b == 0).next().unwrap_or_default();
        if section_name_bytes == target_bytes {
            let mut size = section.virtual_size;
            if size == 0 || size > section.size_of_raw_data {
                size = section.size_of_raw_data;
            }

            let start = section.pointer_to_raw_data as usize;
            let end = match section.pointer_to_raw_data.checked_add(size) {
                Some(offset) => offset as usize,
                None => {
                    return Err(error::Error::Goblin(goblin::error::Error::Malformed(format!(
                        "Section {target} size of {size} is invalid."
                    ))));
                }
            };

            return Ok(Some(
                image
                    .get(start..end)
                    .ok_or(error::Error::Goblin(goblin::error::Error::BufferTooShort(end - start, "bytes")))?,
            ));
        }
    }

    Ok(None)
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use crate::test_support;

    use super::*;
    extern crate std;

    use std::vec;

    #[test]
    fn test_image_bad_signature() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let mut image = *image;
        image.as_mut()[0] = 00;
        let result = UefiPeInfo::parse(&image);
        assert!(result.is_err());
        assert_eq!(format!("{:?}", result.unwrap_err()), "BadSignature(23040)");
    }

    fn section_validation_test_info(size_of_image: u32, sections: &[(u32, u32)]) -> UefiPeInfo {
        let mut pe_info = UefiPeInfo { size_of_image, section_alignment: 0x1000, ..Default::default() };
        for &(virtual_address, virtual_size) in sections {
            pe_info.sections.push(goblin::pe::section_table::SectionTable {
                virtual_address,
                virtual_size,
                ..Default::default()
            });
        }
        pe_info
    }

    #[test]
    fn test_validate_sections_accepts_sections_within_the_image() {
        // Sections need not be contiguous: the sizes below do not sum to the image size.
        let pe_info = section_validation_test_info(0x10000, &[(0x1000, 0x1234), (0xF000, 0x10)]);
        pe_info.validate_sections().expect("sections within the image are valid");

        // A section whose size is not aligned, but which ends exactly at the end of the image once rounded up.
        let pe_info = section_validation_test_info(0x3000, &[(0x2000, 0x10)]);
        pe_info.validate_sections().expect("a section may end exactly at the end of the image");

        let pe_info = section_validation_test_info(0x3000, &[]);
        pe_info.validate_sections().expect("an image without sections is valid");
    }

    #[test]
    fn test_validate_sections_accepts_the_test_images() {
        for image in [
            &include_bytes!("../resources/test/pe32/test_image.pe32")[..],
            &include_bytes!("../resources/test/pe32/DisplayEngine512BFileAlignment.efi")[..],
        ] {
            UefiPeInfo::parse(image).unwrap().validate_sections().expect("a valid image has sections in the image");
        }
    }

    #[test]
    fn test_validate_sections_rejects_sections_beyond_the_image() {
        const FAILING: &[(u32, u32, u32)] = &[
            // (size_of_image, virtual_address, virtual_size)
            (0x3000, 0x2000, 0x1001),        // rounds up to a page past the end
            (0x3000, 0x2FF0, 0x10),          // unaligned start: the rounded size runs past the end
            (0x3000, 0x4000, 0x10),          // section starts after the image
            (0x3000, 0x1000, 0xFFFF_FFF1),   // virtual size that cannot be aligned
            (0x50000, 0x40000, 0xFFFC_0000), // virtual address + virtual size wraps 32 bits
            (0x3000, 0xFFFF_F000, 0x2000),   // virtual address + virtual size wraps 32 bits
        ];
        for &(size_of_image, virtual_address, virtual_size) in FAILING {
            let pe_info = section_validation_test_info(size_of_image, &[(virtual_address, virtual_size)]);
            assert!(
                matches!(pe_info.validate_sections(), Err(error::Error::BufferTooShort(_, "loaded_image"))),
                "image {size_of_image:#x} section {virtual_address:#x}/{virtual_size:#x} must be rejected"
            );
        }

        // Only the last of several sections is out of range.
        let pe_info = section_validation_test_info(0x3000, &[(0x1000, 0x1000), (0x2000, 0x2000)]);
        assert!(matches!(pe_info.validate_sections(), Err(error::Error::BufferTooShort(_, "loaded_image"))));

        // A section alignment that is not a power of two cannot be used to round the size.
        let mut pe_info = section_validation_test_info(0x3000, &[(0x1000, 0x10)]);
        pe_info.section_alignment = 0;
        assert!(matches!(pe_info.validate_sections(), Err(error::Error::BufferTooShort(_, "loaded_image"))));
    }

    #[test]
    fn test_te_image_is_not_supported() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/te/test_image.te");

        // A TE image starts with the "VZ" magic, which is not a PE32 signature.
        let result = UefiPeInfo::parse(image);
        assert!(matches!(result, Err(error::Error::BadSignature(0x5A56))), "unexpected result: {result:?}");

        let result = UefiPeInfo::parse_mapped(image);
        assert!(matches!(result, Err(error::Error::BadSignature(0x5A56))), "unexpected result: {result:?}");
    }

    #[test]
    fn pe_image_info_should_be_correct() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert_eq!(image_info.image_type, 0x0B);
        assert_eq!(image_info.section_alignment, 0x1000);
        assert_eq!(image_info.filename, Some(String::from("RustFfiTestDxe.efi")));
        assert_eq!(image_info.size_of_image, 0x14000);
        assert_eq!(image_info.entry_point_offset, 0x11B8);
    }

    #[test]
    fn msvc_pe_image_info_should_be_correct() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/DisplayEngine512BFileAlignment.efi");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert_eq!(image_info.image_type, 0x0B);
        assert_eq!(image_info.section_alignment, 0x1000);
        // Although the file name is "DisplayEngine512BFileAlignment.efi", the file
        // name inside the debug data is "DisplayEngine.pdb"
        assert_eq!(image_info.filename, Some(String::from("DisplayEngine.efi")));
        assert_eq!(image_info.size_of_image, 0x19000);
        assert_eq!(image_info.entry_point_offset, 0x11EC);
    }

    #[test]
    fn clangpdb_pe_image_info_should_be_correct() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/DisplayEngine32BFileAlignment.efi");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert_eq!(image_info.image_type, 0x0B);
        assert_eq!(image_info.section_alignment, 0x1000);
        // Although the file name is "DisplayEngine32BFileAlignment.efi", the file
        // name inside the debug data is "DisplayEngine.pdb"
        assert_eq!(image_info.filename, Some(String::from("DisplayEngine.efi")));
        assert_eq!(image_info.size_of_image, 0x12000);
        assert_eq!(image_info.entry_point_offset, 0xBE4B);
    }

    #[test]
    fn pe_load_image_should_load_the_image() {
        test_support::init_test_logger();
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
        test_support::init_test_logger();
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
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let pe_info = UefiPeInfo::parse(image).unwrap();
        let edit_image = &image[0..image.len() - 0x1000];

        let mut loaded_image: Vec<u8> = vec![0; pe_info.size_of_image as usize];
        match load_image(&pe_info, edit_image, &mut loaded_image) {
            Err(error::Error::BufferTooShort(..)) => {}
            Ok(()) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {e:?}"),
        }
    }

    #[test]
    fn pe_relocate_to_same_address_should_do_nothing() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");

        let image_info = UefiPeInfo::parse(image).unwrap();

        let mut relocated_once = vec![0; image_info.size_of_image as usize];
        let mut relocated_twice = vec![0; image_info.size_of_image as usize];

        load_image(&image_info, image, &mut relocated_once).unwrap();
        load_image(&image_info, image, &mut relocated_twice).unwrap();

        let blocks = relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_once, &Vec::new()).unwrap();
        let blocks = relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_twice, &blocks).unwrap();
        relocate_image(&image_info, 0x0FFF_FFFF, &mut relocated_twice, &blocks).unwrap();

        assert_eq!(relocated_once, relocated_twice);
    }

    #[test]
    fn pe_relocate_image_should_relocate_the_image() {
        test_support::init_test_logger();
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
        test_support::init_test_logger();
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
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();
        let mut loaded_image = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();

        // Cut the image short at the reloc dir
        let reloc_addr = image_info.reloc_dir.unwrap().virtual_address;
        match relocate_image(&image_info, 0x04158000, &mut loaded_image[0..(reloc_addr + 1) as usize], &Vec::new()) {
            Err(error::Error::BufferTooShort(..)) => {}
            Ok(_) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {e:?}"),
        }
    }

    #[test]
    fn relocate_image_with_unsupported_reloc_type_returns_error() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();
        let mut loaded_image = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();

        // Corrupt the first relocation entry's type nibble (top 4 bits of the u16
        // `type_and_offset`) to IMAGE_REL_BASED_HIGH (0x1).
        let dir = image_info.reloc_dir.unwrap();
        let first_entry_offset = dir.virtual_address as usize + 8;
        let entry = u16::from_le_bytes([loaded_image[first_entry_offset], loaded_image[first_entry_offset + 1]]);
        let corrupted = (entry & 0x0FFF) | (0x1 << 12);
        loaded_image[first_entry_offset..first_entry_offset + 2].copy_from_slice(&corrupted.to_le_bytes());

        let result = relocate_image(&image_info, 0x7CC5_8000, &mut loaded_image, &Vec::new());
        assert!(matches!(result, Err(error::Error::UnsupportedRelocation(0x1))), "unexpected result: {result:?}");
    }

    #[test]
    fn flatten_runtime_relocation_data_rejects_unsupported_type() {
        test_support::init_test_logger();
        // A block whose relocation carries an unsupported type (0x1 = IMAGE_REL_BASED_HIGH) must be
        // rejected during the size-calculation pass, before any allocation occurs.
        let block = RelocationBlock {
            block_header: relocation::BaseRelocationBlockHeader { page_rva: 0, block_size: 0 },
            relocations: vec![relocation::Relocation { type_and_offset: 0x1 << 12, value: 0 }],
        };
        let result = flatten_runtime_relocation_data(&[block]);
        assert!(matches!(result, Err(error::Error::UnsupportedRelocation(0x1))), "unexpected result: {result:?}");
    }

    #[test]
    fn flatten_runtime_relocation_data_serializes_supported_types() {
        test_support::with_clean_global_lock(|| {
            test_support::init_test_logger();
            // SAFETY: Initialization for testing under the global lock so the runtime services data
            // allocator can service allocations.
            unsafe {
                test_support::init_test_gcd(None);
                test_support::reset_allocators();
            }

            let block = RelocationBlock {
                block_header: relocation::BaseRelocationBlockHeader { page_rva: 0, block_size: 0 },
                relocations: vec![
                    relocation::Relocation { type_and_offset: IMAGE_REL_BASED_ABSOLUTE << 12, value: 0xAAAA },
                    relocation::Relocation { type_and_offset: IMAGE_REL_BASED_HIGHLOW << 12, value: 0xAABB_CCDD },
                    relocation::Relocation {
                        type_and_offset: IMAGE_REL_BASED_DIR64 << 12,
                        value: 0x1122_3344_5566_7788,
                    },
                ],
            };

            let flattened = flatten_runtime_relocation_data(&[block]).unwrap();

            let mut expected = Vec::new();
            expected.extend_from_slice(&0xAABB_CCDDu32.to_le_bytes());
            expected.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
            assert_eq!(&flattened[..], &expected[..]);
        })
        .unwrap();
    }

    #[test]
    fn pe_load_resource_section_should_succeed() {
        test_support::init_test_logger();
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
    fn test_load_resource_section_using_size_of_raw_data() {
        test_support::init_test_logger();
        const RELOC_DIR_ENTRY_INDEX: usize = 5;
        let image = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let mut image_info = UefiPeInfo::parse(image).unwrap();

        // Invalidate virtual size, backflow to size_of_raw_data
        image_info.sections[RELOC_DIR_ENTRY_INDEX].virtual_size = 0;
        assert!(load_resource_section(&image_info, image).is_ok());
    }

    #[test]
    fn test_load_resource_section_with_malformed_resource_dir() {
        test_support::init_test_logger();
        const RELOC_DIR_ENTRY_INDEX: usize = 5;
        let image = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        // Set pointer_to_raw_data to a value that can overflow, failing checked add
        let mut image_info2 = image_info.clone();
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].pointer_to_raw_data = u32::MAX;
        match load_resource_section(&image_info2, image) {
            Err(error::Error::Goblin(goblin::error::Error::Malformed(..))) => {}
            Ok(_) => panic!("Expected Malformed error"),
            Err(e) => panic!("Expected Malformed error, got {e:?}"),
        }

        // set size_of_raw_data to a value outside the buffer, causing a buffer too short error
        let mut image_info2 = image_info.clone();
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].virtual_size = 0;
        image_info2.sections[RELOC_DIR_ENTRY_INDEX].size_of_raw_data = image_info2.size_of_image;
        match load_resource_section(&image_info2, image) {
            Err(error::Error::Goblin(goblin::error::Error::BufferTooShort(..))) => {}
            Ok(_) => panic!("Expected BufferTooShort error"),
            Err(e) => panic!("Expected BufferTooShort error, got {e:?}"),
        }
    }

    /// Builds a `.rsrc` section whose root directory has the named entries in `names` (each three characters),
    /// but claims `declared_named` named entries and `declared_id` ID entries regardless of how many it really has.
    ///
    /// The entry named "HII" points to a data entry with `offset_to_data` 0xABCD and `size` 0x100. Layout:
    ///   0x00: Directory (16 bytes)
    ///   then: one 8 byte entry for each name, one 8 byte `DirectoryString` for each name, and the 16 byte `DataEntry`.
    fn rsrc_section_with_named_entries(names: &[&str], declared_named: u16, declared_id: u16) -> Vec<u8> {
        let entries_end = 0x10 + names.len() * 8;
        let data_entry = entries_end + names.len() * 8;
        let mut rsrc = vec![0u8; data_entry + 16];

        rsrc[12..14].copy_from_slice(&declared_named.to_le_bytes());
        rsrc[14..16].copy_from_slice(&declared_id.to_le_bytes());

        for (i, name) in names.iter().enumerate() {
            assert_eq!(name.chars().count(), 3, "the test strings are three characters long");
            let entry = 0x10 + i * 8;
            let string = entries_end + i * 8;

            // Named by the string at `string`, with data (not a directory) at the DataEntry for "HII".
            rsrc[entry..entry + 4].copy_from_slice(&(0x8000_0000 | string as u32).to_le_bytes());
            if *name == "HII" {
                rsrc[entry + 4..entry + 8].copy_from_slice(&(data_entry as u32).to_le_bytes());
            }

            rsrc[string..string + 2].copy_from_slice(&3u16.to_le_bytes());
            for (j, unit) in name.encode_utf16().enumerate() {
                rsrc[string + 2 + j * 2..string + 4 + j * 2].copy_from_slice(&unit.to_le_bytes());
            }
        }

        rsrc[data_entry..data_entry + 4].copy_from_slice(&0xABCDu32.to_le_bytes());
        rsrc[data_entry + 4..data_entry + 8].copy_from_slice(&0x100u32.to_le_bytes());
        rsrc
    }

    fn pe_info_with_rsrc_section(rsrc: &[u8]) -> UefiPeInfo {
        let mut pe_info = UefiPeInfo::default();
        pe_info.sections.push(goblin::pe::section_table::SectionTable {
            name: *b".rsrc\0\0\0",
            virtual_size: rsrc.len() as u32,
            size_of_raw_data: rsrc.len() as u32,
            pointer_to_raw_data: 0,
            ..Default::default()
        });
        pe_info
    }

    // The complete declared entry array is validated before searching it, so an overclaimed count is rejected even
    // when a valid HII entry appears before the end of the section.
    #[test]
    fn test_load_resource_section_with_overclaimed_entry_counts_rejects_early_hii() {
        test_support::init_test_logger();

        for names in [&["HII"][..], &["ABC", "HII"][..]] {
            let rsrc = rsrc_section_with_named_entries(names, u16::MAX, u16::MAX);
            let pe_info = pe_info_with_rsrc_section(&rsrc);
            let result = load_resource_section(&pe_info, &rsrc);
            assert!(matches!(result, Err(error::Error::BufferTooShort(..))), "{names:?}: {result:?}");
        }
    }

    // Parsing the claimed entries stops at the end of the section with an error, instead of reading past it.
    #[test]
    fn test_load_resource_section_with_overclaimed_entry_counts_stops_at_the_end_of_the_section() {
        test_support::init_test_logger();

        let rsrc = rsrc_section_with_named_entries(&["ABC"], u16::MAX, u16::MAX);
        let pe_info = pe_info_with_rsrc_section(&rsrc);
        let result = load_resource_section(&pe_info, &rsrc);
        assert!(matches!(result, Err(error::Error::BufferTooShort(..))), "unexpected result: {result:?}");
    }

    #[test]
    fn test_load_resource_section_with_underclaimed_entry_counts_does_not_find_hii() {
        test_support::init_test_logger();

        for (declared_named, declared_id) in [(1, 0), (0, 0)] {
            let rsrc = rsrc_section_with_named_entries(&["ABC", "HII"], declared_named, declared_id);
            let pe_info = pe_info_with_rsrc_section(&rsrc);
            assert_eq!(load_resource_section(&pe_info, &rsrc).unwrap(), None, "{declared_named} {declared_id}");
        }

        for declared_named in [0, 1] {
            let rsrc = rsrc_section_with_named_entries(&["ABC", "HII"], declared_named, u16::MAX);
            let pe_info = pe_info_with_rsrc_section(&rsrc);
            let result = load_resource_section(&pe_info, &rsrc);
            assert!(matches!(result, Err(error::Error::BufferTooShort(..))), "{declared_named}: {result:?}");
        }

        // The same section with an accurate count does find it.
        let rsrc = rsrc_section_with_named_entries(&["ABC", "HII"], 2, 0);
        let pe_info = pe_info_with_rsrc_section(&rsrc);
        assert_eq!(load_resource_section(&pe_info, &rsrc).unwrap(), Some((0xABCD, 0x100)));
    }

    #[test]
    fn test_load_resource_section_with_out_of_bounds_offsets_returns_error() {
        test_support::init_test_logger();

        const OUT_OF_BOUNDS: u32 = 0x7FFF_FFF0;
        const IS_DIRECTORY_OR_STRING: u32 = 0x8000_0000;

        // The only entry is at 0x10: its name is at 0x10..0x14 and its data is at 0x14..0x18.
        let valid = rsrc_section_with_named_entries(&["HII"], 1, 0);
        let end_of_section = valid.len() as u32;

        // A section with a valid nested directory at 0x30 whose first entry (at 0x40) is another directory.
        let mut nested = valid.clone();
        nested.extend_from_slice(&[0u8; 0x18]);
        nested[0x14..0x18].copy_from_slice(&(IS_DIRECTORY_OR_STRING | 0x30).to_le_bytes());
        nested[0x3C..0x3E].copy_from_slice(&1u16.to_le_bytes());

        let cases = [
            ("name", valid.clone(), 0x10, IS_DIRECTORY_OR_STRING | OUT_OF_BOUNDS),
            ("name at the end of the section", valid.clone(), 0x10, IS_DIRECTORY_OR_STRING | end_of_section),
            ("directory", valid.clone(), 0x14, IS_DIRECTORY_OR_STRING | OUT_OF_BOUNDS),
            ("data entry", valid.clone(), 0x14, OUT_OF_BOUNDS),
            ("nested directory", nested, 0x44, IS_DIRECTORY_OR_STRING | OUT_OF_BOUNDS),
        ];

        for (what, mut rsrc, field, value) in cases {
            rsrc[field..field + 4].copy_from_slice(&value.to_le_bytes());
            let pe_info = pe_info_with_rsrc_section(&rsrc);
            let result = load_resource_section(&pe_info, &rsrc);
            assert!(matches!(result, Err(error::Error::BufferTooShort(..))), "{what}: unexpected result: {result:?}");
        }
    }

    /// Verifies that the HII resource string comparison in `load_resource_section` uses the
    /// correct UTF-16LE encoding and reads from the correct offset in the resource directory.
    #[test]
    fn test_hii_string_uses_correct_utf16le_encoding_at_correct_offset() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image_msvc_hii.pe32");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let result = load_resource_section(&image_info, image).unwrap();
        assert!(result.is_some(), "load_resource_section should find the HII resource section");

        // Knowing that the string is at offset 0xAA5A in test_image_msvc_hii.pe32,
        // verify load_resource_section correctly identifies it by checking that the
        // UTF-16LE encoding of "HII" is correct at that offset.
        const HII_STRING_FILE_OFFSET: usize = 0xAA5A;
        assert_eq!(
            &image[HII_STRING_FILE_OFFSET..HII_STRING_FILE_OFFSET + 6],
            &[0x48u8, 0x00, 0x49, 0x00, 0x49, 0x00],
            "UTF-16LE for 'HII' should be [0x48, 0x00, 0x49, 0x00, 0x49, 0x00]"
        );
    }

    /// Verifies that `load_resource_section` iterates past non-HII named entries to find the HII entry.
    ///
    /// The resource section constructed in the test has two named entries: "ABC" (first) and "HII" (second).
    /// Note: This test creates a .rsrc section in-memory because there is not a known image that has multiple
    ///       named entries with the HII entry not being the first entry.
    #[test]
    fn test_load_resource_section_finds_hii_when_not_first_entry() {
        test_support::init_test_logger();

        // Build a .rsrc section with 2 named entries.
        //
        // From the PE/COFF spec:
        //
        //   The resource directory string area consists of Unicode strings, which are word-aligned. These
        //   strings are stored together after the last Resource Directory entry and before the first
        //   Resource Data entry.
        //
        // Layout:
        //   0x00: Directory (16 bytes) — 2 named entries, 0 id entries
        //   0x10: Entry[0] (8 bytes)   — named "ABC" at string offset 0x30
        //   0x18: Entry[1] (8 bytes)   — named "HII" at string offset 0x38, data at 0x40
        //   0x20: (padding, 16 bytes)
        //   0x30: DirectoryString "ABC" — length=3, then "ABC" (8 bytes)
        //   0x38: DirectoryString "HII" — length=3, then "HII" (8 bytes)
        //   0x40: DataEntry (16 bytes)  — offset_to_data=0xABCD, size=0x100
        //
        // Structure definitions:
        //   - Directory: https://learn.microsoft.com/windows/win32/debug/pe-format#resource-directory-table
        //   - Entry: https://learn.microsoft.com/windows/win32/debug/pe-format#resource-directory-entries
        //   - DirectoryString: https://learn.microsoft.com/windows/win32/debug/pe-format#resource-directory-entries
        let mut rsrc = vec![0u8; 0x50];

        // Directory header at 0x00
        rsrc[12] = 2; // number_of_named_entries = 2

        // Entry[0] at 0x10: "ABC" — id=0x80000030 (name_is_string + offset 0x30)
        rsrc[0x10..0x14].copy_from_slice(&0x8000_0030u32.to_le_bytes());
        // data=0x00000000 (not a directory, won't be reached since "ABC" != "HII")
        rsrc[0x14..0x18].copy_from_slice(&0x0000_0000u32.to_le_bytes());

        // Entry[1] at 0x18: "HII" — id=0x80000038 (name_is_string + offset 0x38)
        rsrc[0x18..0x1C].copy_from_slice(&0x8000_0038u32.to_le_bytes());
        // data=0x00000040 (not a directory, points to DataEntry at 0x40)
        rsrc[0x1C..0x20].copy_from_slice(&0x0000_0040u32.to_le_bytes());

        // DirectoryString "ABC" at 0x30: length=3
        rsrc[0x30..0x32].copy_from_slice(&3u16.to_le_bytes());
        // "ABC" (UTF-16LE) = [0x46, 0x00, 0x4F, 0x00, 0x4F, 0x00]
        rsrc[0x32..0x38].copy_from_slice(&[0x46, 0x00, 0x4F, 0x00, 0x4F, 0x00]);

        // DirectoryString "HII" at 0x38: length=3
        rsrc[0x38..0x3A].copy_from_slice(&3u16.to_le_bytes());
        // "HII" (UTF-16LE) = [0x48, 0x00, 0x49, 0x00, 0x49, 0x00]
        rsrc[0x3A..0x40].copy_from_slice(&[0x48, 0x00, 0x49, 0x00, 0x49, 0x00]);

        // DataEntry at 0x40: offset_to_data=0xABCD, size=0x100
        rsrc[0x40..0x44].copy_from_slice(&0x0000_ABCDu32.to_le_bytes());
        rsrc[0x44..0x48].copy_from_slice(&0x0000_0100u32.to_le_bytes());

        let mut pe_info = UefiPeInfo::default();
        pe_info.sections.push(goblin::pe::section_table::SectionTable {
            name: *b".rsrc\0\0\0",
            virtual_size: rsrc.len() as u32,
            size_of_raw_data: rsrc.len() as u32,
            pointer_to_raw_data: 0,
            ..Default::default()
        });

        let result = load_resource_section(&pe_info, &rsrc).unwrap();
        assert_eq!(result, Some((0xABCD, 0x100)));
    }

    #[test]
    fn test_get_section_with_valid_section() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image_with_sbom_section.bin");
        let expected = include_bytes!("../resources/test/pe32/sbom_section.bin");
        let image_info = UefiPeInfo::parse(image).unwrap();

        let section_bytes = get_section(".sbom", &image_info, image).unwrap().unwrap();
        assert_eq!(section_bytes, expected);
    }

    #[test]
    fn test_get_section_with_nonexistent_section() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/pe32/test_image_with_sbom_section.bin");
        let image_info = UefiPeInfo::parse(image).unwrap();

        assert!(get_section(".fake", &image_info, image).unwrap().is_none());
    }

    #[test]
    fn test_pe_image_with_unmapped_debug_directory_loads_without_filename() {
        test_support::init_test_logger();
        let image = include_bytes!("../resources/test/invalid_debug_directory.efi");

        let parsed_header = goblin::pe::header::Header::parse(image).unwrap();
        let optional_header = parsed_header.optional_header.unwrap();
        let debug_table = optional_header.data_directories.get_debug_table().unwrap();
        let debug_table_end = debug_table.virtual_address.checked_add(debug_table.size).unwrap();
        assert!(debug_table_end <= optional_header.windows_fields.size_of_headers);

        let image_info = UefiPeInfo::parse(image).unwrap();

        assert!(image_info.filename.is_none());

        let mut loaded_image = vec![0; image_info.size_of_image as usize];
        load_image(&image_info, image, &mut loaded_image).unwrap();
    }
}
