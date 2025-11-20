use core::fmt;

use crate::{
    byte_reader::ByteReader,
    error::{Error, StResult},
};

// PE header-related constants
const MZ_SIGNATURE: u16 = 0x5A4D; // 'MZ' in little-endian.
const PAGE_SIZE: u64 = 0x1000; // 4KB pages.
const PE_MAGIC_OFFSET: usize = 0x18;
const PE_POINTER_OFFSET: usize = 0x3C;
const PE_SIGNATURE: u32 = 0x0000_4550; // 'PE\0\0' in little-endian.
const PE64_EXECUTABLE: u16 = 0x20B; // PE32+
const SIZE_OF_IMAGE_OFFSET: usize = 0x50;
const EXCEPTION_TABLE_POINTER_PE32_OFFSET: usize = 0x90;
const EXCEPTION_TABLE_POINTER_PE64_OFFSET: usize = 0xA0;

// PE debug-directory related constants
const DEBUG_DIRECTORY_POINTER_PE64_OFFSET: usize = EXCEPTION_TABLE_POINTER_PE64_OFFSET + 0x18;
const DEBUG_DIRECTORY_ENTRY_SIZE: usize = 0x1C;
const DEBUG_RECORD_RVA_OFFSET: usize = 0x14;
const DEBUG_RECORD_SIZE: usize = 0x10;
const DEBUG_RECORD_TYPE_OFFSET: usize = 0xC;
const DEBUG_RECORD_TYPE_CODEVIEW: u32 = 0x2; // 2 => The Visual C++ debug information.
const CODEVIEW_PDB70_SIGNATURE: u32 = 0x5344_5352; // RSDS
const CODEVIEW_PDB_FILE_NAME_OFFSET: usize = 0x18;

/// Provides in-memory PE file parsing utilities.
#[derive(Clone)]
pub struct PE<'a> {
    /// Image base of the PE image in memory.
    pub base_address: u64,

    /// Size of the image in memory.
    pub _size_of_image: u32,

    /// Image name extracted from the loaded PE image.
    pub image_name: Option<&'static str>,

    /// Loaded image memory as a byte slice.
    pub(crate) bytes: &'a [u8],
}

impl<'a> fmt::Display for PE<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PE Image:\n  Name: {}\n  Base Address: 0x{:016X}\n  Size: {} bytes\n  Bytes: {} bytes",
            self.image_name.unwrap_or("<unknown>"),
            self.base_address,
            self._size_of_image,
            self.bytes.len()
        )
    }
}

impl PE<'_> {
    /// Locates the image corresponding to the RIP.
    // SAFETY: `rip` must be a virtual address that stays mapped and readable
    // for at least one page on every probe performed by this routine. The
    // caller guarantees that probing the surrounding pages does not perform
    // an out-of-bounds or use-after-free memory access.
    #[coverage(off)]
    pub(crate) unsafe fn locate_image(mut rip: u64) -> StResult<Self> {
        let original_rip = rip;

        // Align to the start of a page.
        rip &= !(PAGE_SIZE - 1);

        // Scan each 4 KB page in memory to identify the PE image corresponding
        // to the given RIP.
        while rip > 0 {
            // Convert the 4 KB page into a slice to make it easier to interpret
            // the fields.
            // SAFETY: `rip` has been aligned to a page and the caller keeps that page
            // readable for the lifetime of this probe.
            let page = unsafe { core::slice::from_raw_parts(rip as *const u8, PAGE_SIZE as usize) };

            // Check whether the page begins with the 'MZ' signature.
            let dos_header_signature = page.read16(0)?;
            if dos_header_signature == MZ_SIGNATURE {
                // Although 'MZ' on a page boundary is uncommon, perform
                // additional validation.
                let pe_header_offset = page.read32(PE_POINTER_OFFSET)? as usize;
                let pe_header_signature = page.read32(pe_header_offset)?;

                // Confirm that this is a valid PE header.
                if pe_header_signature == PE_SIGNATURE {
                    // This field contains the size of the entire loaded image in
                    // memory.
                    let size_of_image = page.read32(pe_header_offset + SIZE_OF_IMAGE_OFFSET)?;

                    // Parse the debug directory so we can process the image
                    // name later.
                    let debug_directory_rva =
                        page.read32(pe_header_offset + DEBUG_DIRECTORY_POINTER_PE64_OFFSET).unwrap_or(0) as usize;
                    let debug_directory_size =
                        page.read32(pe_header_offset + DEBUG_DIRECTORY_POINTER_PE64_OFFSET + 4).unwrap_or(0) as usize;

                    // Identify the image name.
                    let image_name = if debug_directory_size != 0 {
                        // SAFETY: `rip` still denotes the mapped image base, and the computed
                        // debug-directory range lies within that mapping per PE header offsets.
                        unsafe { Self::get_image_name(rip, debug_directory_rva, debug_directory_size) }
                    } else {
                        None
                    };

                    // SAFETY: The caller ensures the mapped image remains readable;
                    // `rip` is still page-aligned and within that mapping.
                    let bytes = unsafe { core::slice::from_raw_parts(rip as *const u8, size_of_image as usize) };

                    return Ok(Self { base_address: rip, _size_of_image: size_of_image, image_name, bytes });
                }
            }

            // Move to the previous page.
            rip -= PAGE_SIZE;
        }

        // The given RIP does not correspond to a valid image.
        Err(Error::ImageNotFound { rip: original_rip })
    }

    /// Private helper that locates the image name in memory.
    // SAFETY: `page_base` must reference the same mapped image passed to
    // `locate_image`. The caller guarantees that the debug directory and its
    // derived ranges are readable for the duration of this routine.
    unsafe fn get_image_name(
        page_base: u64,
        debug_directory_rva: usize,
        debug_directory_size: usize,
    ) -> Option<&'static str> {
        // Convert the debug data section into a slice to make it easier to interpret the fields.
        // SAFETY: The caller guarantees that `page_base + debug_directory_rva` points to
        // a readable region of length `debug_directory_size`.
        let debug_directory = unsafe {
            core::slice::from_raw_parts((page_base + debug_directory_rva as u64) as *const u8, debug_directory_size)
        };

        // Break the debug directory into individual entries, filter the entries
        // of type IMAGE_DEBUG_TYPE_CODEVIEW (2), and extract the debug data RVA
        // and its size.
        let debug_record = debug_directory
            .chunks(DEBUG_DIRECTORY_ENTRY_SIZE)
            .filter(|&bytes| {
                let debug_record_type = bytes.read32(DEBUG_RECORD_TYPE_OFFSET).unwrap_or(0);
                debug_record_type == DEBUG_RECORD_TYPE_CODEVIEW
            })
            .map(|bytes| {
                let debug_data_size = bytes.read32(DEBUG_RECORD_SIZE).unwrap_or(0);
                let debug_data_rva = bytes.read32(DEBUG_RECORD_RVA_OFFSET).unwrap_or(0);
                (debug_data_rva, debug_data_size)
            })
            .next();

        let Some((debug_data_rva, debug_data_size)) = debug_record else {
            // Bail out if this record is not found.
            return None;
        };

        if debug_data_rva == 0 || debug_data_size == 0 {
            return None;
        };

        let debug_data = page_base + debug_data_rva as u64;

        // Check the CodeView signature.
        // SAFETY: `debug_data` is within the caller-provided PE image and points to
        // the beginning of the CodeView structure.
        let codeview_signature = unsafe { *(debug_data as *const u32) };
        if codeview_signature != CODEVIEW_PDB70_SIGNATURE {
            return None;
        }

        // Extract the PDB file path.
        // SAFETY: The caller guarantees that the CodeView record, including the
        // file-name payload, is fully mapped and readable.
        let file_name_bytes = unsafe {
            core::slice::from_raw_parts(
                (debug_data + CODEVIEW_PDB_FILE_NAME_OFFSET as u64) as *const u8,
                debug_data_size as usize - CODEVIEW_PDB_FILE_NAME_OFFSET,
            )
        };

        // Extract the PDB file name. This should be the image name.
        let Ok(file_name) = core::str::from_utf8(file_name_bytes) else {
            return None;
        };
        if let Some(file_name_with_ext) = file_name.rsplit('\\').next()
            && let Some((file_name, _ext)) = file_name_with_ext.rsplit_once('.')
        {
            return Some(file_name);
        }

        // log::info!("Pdb file name : {}", file_name);

        Some(file_name)
    }

    // SAFETY: `self.bytes` refers to raw image memory supplied by the runtime.
    // The caller must ensure that the PE headers referenced by this method are
    // readable for the duration of the call.
    pub(crate) unsafe fn get_exception_table(&self) -> StResult<(u32, u32)> {
        // Get the PE header offset.
        let pe_header_offset = self.bytes.read32(PE_POINTER_OFFSET)? as usize;

        // Determine the PE type (PE32 or PE32+).
        let pe_type = self.bytes.read16(pe_header_offset + PE_MAGIC_OFFSET)?;

        // Jump to the exception table data directory and read the exception table
        // RVA.
        let offset = if pe_type == PE64_EXECUTABLE {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE64_OFFSET
        } else {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE32_OFFSET
        };
        let exception_table_rva = self.bytes.read32(offset)?;

        // Jump to the exception table section size.
        let offset = if pe_type == PE64_EXECUTABLE {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE64_OFFSET + 4
        } else {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE32_OFFSET + 4
        };
        let exception_table_size = self.bytes.read32(offset)?;

        // Bail out if the exception table section (the `.pdata` section) is not
        // available.
        if exception_table_rva == 0 || exception_table_size == 0 {
            return Err(Error::ExceptionDirectoryNotFound { module: self.image_name });
        }

        Ok((exception_table_rva, exception_table_size))
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// Helper: create a minimal fake PE image in memory.
    fn make_fake_pe_image() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x2000]; // 8 KB buffer to simulate PE image

        // DOS header ('MZ')
        bytes[0..2].copy_from_slice(&MZ_SIGNATURE.to_le_bytes());

        // PE header pointer at 0x3C -> points to offset 0x80
        let pe_header_offset = 0x80u32;
        bytes[PE_POINTER_OFFSET..PE_POINTER_OFFSET + 4].copy_from_slice(&pe_header_offset.to_le_bytes());

        // Write PE signature ('PE\0\0') at 0x80
        bytes[pe_header_offset as usize..pe_header_offset as usize + 4].copy_from_slice(&PE_SIGNATURE.to_le_bytes());

        // SizeOfImage (at +0x50)
        let size_of_image = 0x2000u32;
        let size_of_image_offset = pe_header_offset as usize + SIZE_OF_IMAGE_OFFSET;
        bytes[size_of_image_offset..size_of_image_offset + 4].copy_from_slice(&size_of_image.to_le_bytes());

        // Debug directory pointer (RVA + size)
        let debug_dir_rva = 0x400u32;
        let debug_dir_size = 0x1Cu32;
        let debug_dir_offset = pe_header_offset as usize + DEBUG_DIRECTORY_POINTER_PE64_OFFSET;
        bytes[debug_dir_offset..debug_dir_offset + 4].copy_from_slice(&debug_dir_rva.to_le_bytes());
        bytes[debug_dir_offset + 4..debug_dir_offset + 8].copy_from_slice(&debug_dir_size.to_le_bytes());

        // Debug directory (1 entry)
        let debug_dir_entry_offset = debug_dir_rva as usize;
        // IMAGE_DEBUG_DIRECTORY.Type = 2 (CodeView)
        let debug_type_offset = debug_dir_entry_offset + DEBUG_RECORD_TYPE_OFFSET;
        bytes[debug_type_offset..debug_type_offset + 4].copy_from_slice(&DEBUG_RECORD_TYPE_CODEVIEW.to_le_bytes());
        // Debug data RVA and size
        let debug_data_rva = 0x800u32;
        let debug_data_size = 0x100u32;
        let debug_rva_off = debug_dir_entry_offset + DEBUG_RECORD_RVA_OFFSET;
        bytes[debug_rva_off..debug_rva_off + 4].copy_from_slice(&debug_data_rva.to_le_bytes());
        let debug_size_off = debug_dir_entry_offset + DEBUG_RECORD_SIZE;
        bytes[debug_size_off..debug_size_off + 4].copy_from_slice(&debug_data_size.to_le_bytes());

        // CodeView data section
        let debug_data_offset = debug_data_rva as usize;
        bytes[debug_data_offset..debug_data_offset + 4].copy_from_slice(&CODEVIEW_PDB70_SIGNATURE.to_le_bytes());

        // Insert a fake PDB path (RSDS... + "C:\\path\\app.exe\0")
        let fake_pdb_path = b"C:\\path\\app.exe\0";
        let name_off = debug_data_offset + CODEVIEW_PDB_FILE_NAME_OFFSET;
        bytes[name_off..name_off + fake_pdb_path.len()].copy_from_slice(fake_pdb_path);

        bytes
    }

    #[test]
    fn test_locate_image_success() {
        let bytes = make_fake_pe_image();
        let base = bytes.as_ptr() as u64;

        let pe = PE { base_address: base, _size_of_image: bytes.len() as u32, image_name: Some("fake"), bytes: &bytes };

        // Since we didn't define exception table fields, expect an error.
        // SAFETY: Test creates a fake PE image structure for validation; `pe.bytes` points to a valid slice
        // in that file.
        assert!(matches!(unsafe { pe.get_exception_table() }, Err(Error::ExceptionDirectoryNotFound { .. })));
    }

    #[test]
    fn test_get_image_name_success() {
        let bytes = make_fake_pe_image();
        let base = bytes.as_ptr() as u64;
        // SAFETY: Test creates a fake PE image with valid debug directory at specified RVA.
        let image_name = unsafe { PE::get_image_name(base, 0x400, 0x1C) };
        assert_eq!(image_name, Some("app"));
    }

    #[test]
    fn test_get_image_name_failure_invalid_signature() {
        let mut bytes = make_fake_pe_image();
        let base = bytes.as_ptr() as u64;
        // Corrupt the signature
        let debug_data = 0x800;
        bytes[debug_data..debug_data + 4].copy_from_slice(&0x12345678u32.to_le_bytes());
        // SAFETY: Test creates a fake PE image with corrupted signature for validation.
        let image_name = unsafe { PE::get_image_name(base, 0x400, 0x1C) };
        assert_eq!(image_name, None);
    }
}
