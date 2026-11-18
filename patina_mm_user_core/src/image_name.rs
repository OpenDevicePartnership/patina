//! MM Driver Image Name Resolution
//!
//! Reads a driver's module name from the `CodeView` debug entry of its loaded image, so
//! dispatch logs can show `VariableStandaloneMm.efi` next to the FFS file GUID. Images
//! without debug data resolve to `None`, and callers fall back to the GUID.
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
/// Returns `None` when the image cannot be parsed or has no `CodeView` debug entry.
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

    // A loaded image has its sections at their virtual addresses, so RVAs index `bytes`
    // directly. Retry with RVAs resolved through the section table for an on-disk layout.
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
    let name_end = raw_name.iter().position(|&byte| byte == b'\0').unwrap_or(raw_name.len());
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
