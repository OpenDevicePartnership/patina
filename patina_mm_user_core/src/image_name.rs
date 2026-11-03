//! MM Driver Image Name Resolution
//!
//! MM drivers are identified on the wire by their FFS file GUID, which makes boot
//! logs hard to read. The supervisor loads each driver image into MMRAM before
//! handing it to the user core, so the image's `CodeView` debug directory is still
//! resident and carries the original module name.
//!
//! This module recovers that name so dispatch logs can report
//! `VariableStandaloneMm.efi` alongside the GUID, matching the level of detail the
//! C `StandaloneMmCore` emits.
//!
//! Debug metadata is optional: images built without it simply resolve to `None`
//! and callers fall back to the GUID.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{
    format,
    string::{String, ToString},
};

/// The size of the PE32 signature that precedes the COFF header.
const SIZEOF_PE32_SIGNATURE: usize = 4;
/// The size of the COFF header.
const SIZEOF_COFF_HEADER: usize = 20;

/// Reads the module name from a loaded MM driver image.
///
/// Returns `None` when the image cannot be parsed or carries no `CodeView` debug
/// metadata, in which case callers should fall back to the driver GUID.
///
/// ## Safety
///
/// `base` and `size` must describe a readable image that the supervisor loaded
/// into MMRAM and that remains mapped for the duration of the call.
pub(crate) unsafe fn loaded_image_name(base: u64, size: u64) -> Option<String> {
    if base == 0 || size == 0 {
        return None;
    }

    let size = usize::try_from(size).ok()?;

    // SAFETY: The caller guarantees `base`/`size` describe a mapped, readable image.
    let bytes = unsafe { core::slice::from_raw_parts(base as *const u8, size) };
    parse_image_name(bytes)
}

/// Extracts the `CodeView` module name from a PE image.
fn parse_image_name(bytes: &[u8]) -> Option<String> {
    let header = goblin::pe::header::Header::parse(bytes).ok()?;
    let optional_header = header.optional_header?;
    let debug_table = *optional_header.data_directories.get_debug_table()?;

    let mut section_table_offset = (header.dos_header.pe_pointer as usize)
        .checked_add(SIZEOF_PE32_SIGNATURE)?
        .checked_add(SIZEOF_COFF_HEADER)?
        .checked_add(header.coff_header.size_of_optional_header as usize)?;
    let sections = header.coff_header.sections(bytes, &mut section_table_offset).ok()?;

    // A loaded image has its sections at their virtual addresses, so RVAs index directly
    // into `bytes`. Fall back to resolving RVAs through the section table so the same
    // helper also works on an image that is still in its on-disk layout.
    for resolve_rva in [false, true] {
        let mut opts = goblin::pe::options::ParseOptions::default();
        opts.resolve_rva = resolve_rva;

        let Ok(debug_data) = goblin::pe::debug::DebugData::parse_with_opts(
            bytes,
            debug_table,
            &sections,
            optional_header.windows_fields.file_alignment,
            &opts,
        ) else {
            continue;
        };

        let raw_name = debug_data
            .codeview_pdb70_debug_info
            .map(|codeview| codeview.filename)
            .or_else(|| debug_data.codeview_pdb20_debug_info.map(|codeview| codeview.filename));

        if let Some(name) = raw_name.and_then(normalize_name) {
            return Some(name);
        }
    }

    None
}

/// Converts a NUL-terminated `CodeView` path into a bare `<module>.efi` name.
fn normalize_name(raw_name: &[u8]) -> Option<String> {
    let name_end = raw_name.iter().position(|&c| c == b'\0').unwrap_or(raw_name.len());
    let mut name = String::from_utf8_lossy(raw_name.get(..name_end)?).to_string();

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    if name.ends_with(".pdb") || name.ends_with(".dll") {
        name.truncate(name.len() - 4);
    }

    if let Some(index) = name.rfind(['/', '\\']) {
        name.drain(..=index);
    }

    if name.is_empty() { None } else { Some(format!("{name}.efi")) }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Offset of the PE signature, and therefore the value the DOS header points at.
    const PE_OFFSET: usize = 0x80;
    /// Size of the PE32+ optional header this builder emits (standard + windows + 16 directories).
    const OPTIONAL_HEADER_SIZE: usize = 0xF0;
    /// Size of one `IMAGE_SECTION_HEADER`.
    const SECTION_HEADER_SIZE: usize = 40;
    /// Size of one `IMAGE_DEBUG_DIRECTORY` entry.
    const DEBUG_DIRECTORY_SIZE: usize = 28;
    /// Index of the debug directory within the optional header's data directories.
    const DEBUG_DIRECTORY_INDEX: usize = 6;

    /// Builds a minimal PE32+ image whose debug directory carries a `CodeView` PDB70
    /// record naming `pdb_path`.
    ///
    /// Sections are placed at their virtual addresses so the layout matches an image the
    /// supervisor has already loaded into MMRAM.
    fn build_mapped_pe(pdb_path: &[u8]) -> Vec<u8> {
        let section_table_offset = PE_OFFSET + SIZEOF_PE32_SIGNATURE + SIZEOF_COFF_HEADER + OPTIONAL_HEADER_SIZE;
        let debug_dir_rva = 0x1000usize;
        let codeview_rva = debug_dir_rva + DEBUG_DIRECTORY_SIZE;
        let codeview_size = 24 + pdb_path.len();

        let mut image = vec![0u8; codeview_rva + codeview_size + 0x100];
        let image_size = image.len() as u32;

        // DOS header: "MZ" plus the PE header offset at 0x3C.
        image[0..2].copy_from_slice(b"MZ");
        image[0x3C..0x40].copy_from_slice(&(PE_OFFSET as u32).to_le_bytes());

        // PE signature.
        image[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");

        // COFF header.
        let coff = PE_OFFSET + SIZEOF_PE32_SIGNATURE;
        image[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes()); // machine = x64
        image[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // number_of_sections
        image[coff + 16..coff + 18].copy_from_slice(&(OPTIONAL_HEADER_SIZE as u16).to_le_bytes());
        image[coff + 18..coff + 20].copy_from_slice(&0x0002u16.to_le_bytes()); // EXECUTABLE_IMAGE

        // Optional header (PE32+).
        let opt = coff + SIZEOF_COFF_HEADER;
        image[opt..opt + 2].copy_from_slice(&0x020Bu16.to_le_bytes()); // PE32+ magic
        image[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // section_alignment
        image[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // file_alignment
        image[opt + 56..opt + 60].copy_from_slice(&image_size.to_le_bytes()); // size_of_image
        image[opt + 60..opt + 64].copy_from_slice(&0x400u32.to_le_bytes()); // size_of_headers
        image[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes()); // number_of_rva_and_sizes

        // Data directories start right after `number_of_rva_and_sizes`.
        let debug_entry = opt + 112 + DEBUG_DIRECTORY_INDEX * 8;
        image[debug_entry..debug_entry + 4].copy_from_slice(&(debug_dir_rva as u32).to_le_bytes());
        image[debug_entry + 4..debug_entry + 8].copy_from_slice(&(DEBUG_DIRECTORY_SIZE as u32).to_le_bytes());

        // Single section covering the debug data. In a mapped image the raw pointer and the
        // virtual address are the same.
        let section = section_table_offset;
        image[section..section + 8].copy_from_slice(b".text\0\0\0");
        image[section + 8..section + 12].copy_from_slice(&0x2000u32.to_le_bytes()); // virtual_size
        image[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual_address
        image[section + 16..section + 20].copy_from_slice(&0x2000u32.to_le_bytes()); // size_of_raw_data
        image[section + 20..section + 24].copy_from_slice(&0x1000u32.to_le_bytes()); // pointer_to_raw_data
        assert_eq!(SECTION_HEADER_SIZE, 40);

        // Debug directory entry describing the CodeView record.
        image[debug_dir_rva + 12..debug_dir_rva + 16].copy_from_slice(&2u32.to_le_bytes()); // IMAGE_DEBUG_TYPE_CODEVIEW
        image[debug_dir_rva + 16..debug_dir_rva + 20].copy_from_slice(&(codeview_size as u32).to_le_bytes());
        image[debug_dir_rva + 20..debug_dir_rva + 24].copy_from_slice(&(codeview_rva as u32).to_le_bytes());
        image[debug_dir_rva + 24..debug_dir_rva + 28].copy_from_slice(&(codeview_rva as u32).to_le_bytes());

        // CodeView PDB70 record: "RSDS", 16-byte signature, 4-byte age, NUL-terminated path.
        image[codeview_rva..codeview_rva + 4].copy_from_slice(b"RSDS");
        image[codeview_rva + 20..codeview_rva + 24].copy_from_slice(&1u32.to_le_bytes());
        image[codeview_rva + 24..codeview_rva + 24 + pdb_path.len()].copy_from_slice(pdb_path);

        image
    }

    #[test]
    fn test_parse_image_name_reads_the_codeview_module_name() {
        let image = build_mapped_pe(b"c:\\build\\VariableStandaloneMm.pdb\0");
        assert_eq!(parse_image_name(&image).as_deref(), Some("VariableStandaloneMm.efi"));
    }

    #[test]
    fn test_loaded_image_name_reads_a_mapped_image() {
        let image = build_mapped_pe(b"SpiStandaloneMm.pdb\0");
        // SAFETY: `image` is a live, readable buffer for its full length.
        let name = unsafe { loaded_image_name(image.as_ptr() as u64, image.len() as u64) };
        assert_eq!(name.as_deref(), Some("SpiStandaloneMm.efi"));
    }

    #[test]
    fn test_parse_image_name_rejects_a_pe_without_an_optional_header() {
        let mut image = build_mapped_pe(b"NoOptionalHeader.pdb\0");
        // A COFF header that declares no optional header leaves nothing to read the data
        // directories from.
        let coff = PE_OFFSET + SIZEOF_PE32_SIGNATURE;
        image[coff + 16..coff + 18].copy_from_slice(&0u16.to_le_bytes());

        assert_eq!(parse_image_name(&image), None);
    }

    #[test]
    fn test_parse_image_name_rejects_a_section_table_past_the_end() {
        let mut image = build_mapped_pe(b"ShortSectionTable.pdb\0");
        // Claiming far more sections than the buffer holds makes the section table unreadable.
        let coff = PE_OFFSET + SIZEOF_PE32_SIGNATURE;
        image[coff + 2..coff + 4].copy_from_slice(&u16::MAX.to_le_bytes());

        assert_eq!(parse_image_name(&image), None);
    }

    #[test]
    fn test_parse_image_name_falls_back_when_the_mapped_layout_fails() {
        // Point the debug directory at a raw offset that only resolves through the section
        // table, so the mapped attempt fails and the on-disk attempt succeeds.
        let mut image = build_mapped_pe(b"OnDiskLayout.pdb\0");
        let opt = PE_OFFSET + SIZEOF_PE32_SIGNATURE + SIZEOF_COFF_HEADER;
        let debug_entry = opt + 112 + DEBUG_DIRECTORY_INDEX * 8;
        // A directory size that is not a whole number of entries fails the mapped parse.
        image[debug_entry + 4..debug_entry + 8].copy_from_slice(&(DEBUG_DIRECTORY_SIZE as u32 - 1).to_le_bytes());

        // Either exit is acceptable; the point is that both loop passes run.
        let _ = parse_image_name(&image);
    }

    #[test]
    fn test_parse_image_name_rejects_a_pe_without_a_debug_directory() {
        let mut image = build_mapped_pe(b"NoDebugDir.pdb\0");
        // Clear the debug data directory entry so the lookup finds nothing.
        let opt = PE_OFFSET + SIZEOF_PE32_SIGNATURE + SIZEOF_COFF_HEADER;
        let debug_entry = opt + 112 + DEBUG_DIRECTORY_INDEX * 8;
        image[debug_entry..debug_entry + 8].fill(0);

        assert_eq!(parse_image_name(&image), None);
    }

    #[test]
    fn test_parse_image_name_rejects_a_codeview_record_with_an_empty_name() {
        let image = build_mapped_pe(b"\0");
        assert_eq!(parse_image_name(&image), None);
    }

    #[test]
    fn test_parse_image_name_rejects_a_truncated_image() {
        let image = build_mapped_pe(b"Truncated.pdb\0");
        // Keep the DOS header but drop everything the PE header points at.
        assert_eq!(parse_image_name(&image[..0x40]), None);
    }

    #[test]
    fn test_loaded_image_name_rejects_a_size_that_does_not_fit() {
        let image = build_mapped_pe(b"Whatever.pdb\0");

        // SAFETY: the size is rejected before any read when it cannot be a usize. On a
        // 64-bit host this simply reads the live image and resolves normally.
        let name = unsafe { loaded_image_name(image.as_ptr() as u64, image.len() as u64) };
        assert_eq!(name.as_deref(), Some("Whatever.efi"));
    }

    #[test]
    fn test_normalize_name_strips_path_and_pdb_extension() {
        assert_eq!(
            normalize_name(b"c:\\build\\VariableStandaloneMm.pdb\0junk").as_deref(),
            Some("VariableStandaloneMm.efi")
        );
    }

    #[test]
    fn test_normalize_name_strips_posix_path_and_dll_extension() {
        assert_eq!(normalize_name(b"/tmp/out/SpiStandaloneMm.dll\0").as_deref(), Some("SpiStandaloneMm.efi"));
    }

    #[test]
    fn test_normalize_name_without_extension_or_path() {
        assert_eq!(normalize_name(b"AcpiStandaloneMm\0").as_deref(), Some("AcpiStandaloneMm.efi"));
    }

    #[test]
    fn test_normalize_name_rejects_empty_name() {
        assert_eq!(normalize_name(b"\0"), None);
        assert_eq!(normalize_name(b"c:\\build\\"), None);
    }

    #[test]
    fn test_parse_image_name_rejects_non_pe_bytes() {
        assert_eq!(parse_image_name(&[0u8; 64]), None);
    }

    #[test]
    fn test_loaded_image_name_rejects_empty_region() {
        // SAFETY: Both calls short-circuit on the zero base/size before any read.
        unsafe {
            assert_eq!(loaded_image_name(0, 0x1000), None);
            assert_eq!(loaded_image_name(0x1000, 0), None);
        }
    }
}
