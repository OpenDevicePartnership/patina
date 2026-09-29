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
const EXCEPTION_TABLE_POINTER_OFFSET: usize = 0xA0;
const NUMBER_OF_RVA_AND_SIZES_OFFSET: usize = 0x84;

// PE debug-directory related constants
const DEBUG_DIRECTORY_POINTER_OFFSET: usize = EXCEPTION_TABLE_POINTER_OFFSET + 0x18;
const DEBUG_DIRECTORY_INDEX: u32 = 6;
const DEBUG_DIRECTORY_ENTRY_SIZE: usize = 0x1C;
const DEBUG_RECORD_RVA_OFFSET: usize = 0x14;
const DEBUG_RECORD_SIZE: usize = 0x10;
const DEBUG_RECORD_TYPE_OFFSET: usize = 0xC;
const DEBUG_RECORD_TYPE_CODEVIEW: u32 = 0x2; // 2 => The Visual C++ debug information.
const CODEVIEW_SIGNATURE_NB10: u32 = 0x3031_424E; // NB10
const CODEVIEW_PDB70_SIGNATURE: u32 = 0x5344_5352; // RSDS
const CODEVIEW_NB10_FILE_NAME_OFFSET: usize = 0x10;
const CODEVIEW_PDB70_FILE_NAME_OFFSET: usize = 0x18;
const MAX_IMAGE_NAME_LENGTH: usize = 256;

/// Provides in-memory PE file parsing utilities.
#[derive(Clone)]
pub struct PE<'a> {
    /// Image base of the PE image in memory.
    pub base_address: u64,

    /// Size of the image in memory.
    pub size_of_image: u32,

    /// Image name extracted from the loaded PE image.
    pub image_name: Option<&'static str>,

    /// Loaded image memory as a byte slice.
    pub(crate) bytes: &'a [u8],
}

impl fmt::Display for PE<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PE Image:\n  Name: {}\n  Base Address: 0x{:016X}\n  Size: {} bytes\n  Bytes: {} bytes",
            self.image_name.unwrap_or("<unknown>"),
            self.base_address,
            self.size_of_image,
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
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) unsafe fn locate_image(mut rip: u64) -> StResult<Self> {
        let original_rip = rip;

        // Align to the start of a page.
        rip &= !(PAGE_SIZE - 1);

        // Scan each 4 KB page in memory to identify the PE image corresponding
        // to the given RIP.
        while rip > 0 {
            // SAFETY: `rip` has been aligned to a page and the caller keeps that page
            // readable for the lifetime of this probe.
            if let Some(image) = unsafe { Self::try_parse_image(rip) } {
                return Ok(image);
            }

            // Move to the previous page.
            rip -= PAGE_SIZE;
        }

        // The given RIP does not correspond to a valid image.
        Err(Error::ImageNotFound { rip: original_rip })
    }

    /// Private helper that attempts to interpret `base_address` as the start of
    /// a loaded PE image.
    ///
    /// Returns `None` when the page is not the start of a well formed image, so
    /// that the caller can keep scanning instead of aborting the walk.
    // SAFETY: `base_address` must be page aligned and the page starting there
    // must stay mapped and readable for the duration of this call. If the page
    // holds a valid PE header, the whole `SizeOfImage` range must also be
    // mapped and readable.
    unsafe fn try_parse_image(base_address: u64) -> Option<Self> {
        // Convert the 4 KB page into a slice to make it easier to interpret the
        // fields.
        // SAFETY: The caller guarantees that the page at `base_address` is readable.
        let page = unsafe { core::slice::from_raw_parts(base_address as *const u8, PAGE_SIZE as usize) };

        // Check whether the page begins with the 'MZ' signature.
        if page.read16(0).ok()? != MZ_SIGNATURE {
            return None;
        }

        // Although 'MZ' on a page boundary is uncommon, perform additional
        // validation. Every read below is bounds checked against the page and
        // yields `None` on failure so that a page that merely starts with 'MZ'
        // does not abort the scan.
        let pe_header_offset = page.read32(PE_POINTER_OFFSET).ok()? as usize;
        if page.read32(pe_header_offset).ok()? != PE_SIGNATURE {
            return None;
        }

        // Only PE32+ images are supported, so reject any other optional header
        // magic instead of reading data directories at the wrong offsets.
        if page.read16(pe_header_offset.checked_add(PE_MAGIC_OFFSET)?).ok()? != PE64_EXECUTABLE {
            return None;
        }

        // This field contains the size of the entire loaded image in memory.
        let size_of_image_offset = pe_header_offset.checked_add(SIZE_OF_IMAGE_OFFSET)?;
        let size_of_image = page.read32(size_of_image_offset).ok()?;

        // The image must at least contain the headers that were just parsed.
        if (size_of_image as usize) <= size_of_image_offset {
            return None;
        }

        // SAFETY: The caller ensures the mapped image remains readable;
        // `base_address` is page aligned and the image starts there.
        let bytes: &'static [u8] =
            unsafe { core::slice::from_raw_parts(base_address as *const u8, size_of_image as usize) };

        // Identify the image name from the debug directory. Failures here are
        // not fatal: the image is still usable for unwinding, just unnamed.
        let image_name = Self::get_image_name(bytes, pe_header_offset);

        Some(Self { base_address, size_of_image, image_name, bytes })
    }

    /// Private helper that locates the image name in the `CodeView` (PDB) record
    /// referenced by the debug directory.
    ///
    /// Returns `None` whenever the headers are malformed, so every offset and
    /// size taken from the image is validated against `image` before use.
    fn get_image_name(image: &[u8], pe_header_offset: usize) -> Option<&str> {
        // The debug directory is data directory index 6, so it only exists when
        // the optional header declares more than six data directories.
        let number_of_rva_and_sizes =
            image.read32(pe_header_offset.checked_add(NUMBER_OF_RVA_AND_SIZES_OFFSET)?).ok()?;
        if number_of_rva_and_sizes <= DEBUG_DIRECTORY_INDEX {
            return None;
        }

        let debug_directory_pointer = pe_header_offset.checked_add(DEBUG_DIRECTORY_POINTER_OFFSET)?;
        let debug_directory_rva = image.read32(debug_directory_pointer).ok()? as usize;
        let debug_directory_size = image.read32(debug_directory_pointer.checked_add(4)?).ok()? as usize;

        // The debug directory must lie entirely within the image and hold at
        // least one complete entry.
        if debug_directory_rva == 0 || debug_directory_size < DEBUG_DIRECTORY_ENTRY_SIZE {
            return None;
        }
        let debug_directory = image.get(debug_directory_rva..debug_directory_rva.checked_add(debug_directory_size)?)?;

        // Break the debug directory into individual entries, filter the entries
        // of type IMAGE_DEBUG_TYPE_CODEVIEW (2), and extract the debug data RVA
        // and its size. A trailing partial entry is discarded.
        let (debug_data_rva, debug_data_size) = debug_directory
            .chunks_exact(DEBUG_DIRECTORY_ENTRY_SIZE)
            .filter(|&bytes| {
                let debug_record_type = bytes.read32(DEBUG_RECORD_TYPE_OFFSET).unwrap_or(0);
                debug_record_type == DEBUG_RECORD_TYPE_CODEVIEW
            })
            .map(|bytes| {
                let debug_data_size = bytes.read32(DEBUG_RECORD_SIZE).unwrap_or(0) as usize;
                let debug_data_rva = bytes.read32(DEBUG_RECORD_RVA_OFFSET).unwrap_or(0) as usize;
                (debug_data_rva, debug_data_size)
            })
            .next()?;

        if debug_data_rva == 0 || debug_data_size == 0 {
            return None;
        }

        // The CodeView record must lie entirely within the image.
        let debug_data = image.get(debug_data_rva..debug_data_rva.checked_add(debug_data_size)?)?;

        // Check the CodeView signature. This read is bounds checked and does
        // not require the record to be aligned.
        let codeview_signature = debug_data.read32(0).ok()?;

        // Determine the file name offset based on the CodeView format
        let file_name_offset = match codeview_signature {
            CODEVIEW_SIGNATURE_NB10 => CODEVIEW_NB10_FILE_NAME_OFFSET,
            CODEVIEW_PDB70_SIGNATURE => CODEVIEW_PDB70_FILE_NAME_OFFSET,
            _ => return None, // Unsupported CodeView format
        };

        // Extract the PDB file path. `get` yields `None` when the record is
        // too small to hold the fixed CodeView header, so the file name length
        // can never underflow.
        let file_name_bytes = debug_data.get(file_name_offset..)?;

        // The path is NUL terminated; drop the terminator and any trailing
        // padding before validating the bytes as UTF-8. The path itself is not
        // length limited: it is already bounded by the record, which in turn is
        // bounded by the image, and build paths are routinely long.
        let file_name_bytes = file_name_bytes.split(|&byte| byte == 0).next()?;
        if file_name_bytes.is_empty() {
            return None;
        }

        let file_name = core::str::from_utf8(file_name_bytes).ok()?;

        // Handle both Windows (\) and Linux (/) path separators
        let file_name_with_ext = file_name.rsplit(['\\', '/']).next().unwrap_or(file_name);

        // Strip the extension, if any, to get the module name.
        let image_name =
            file_name_with_ext.rsplit_once('.').map_or(file_name_with_ext, |(image_name, _ext)| image_name);

        // Bound what ends up in the log. A well formed record names a single
        // file here, so anything longer is treated as garbage.
        if image_name.is_empty() || image_name.len() > MAX_IMAGE_NAME_LENGTH {
            return None;
        }

        Some(image_name)
    }

    // SAFETY: `self.bytes` refers to raw image memory supplied by the runtime.
    // The caller must ensure that the PE headers referenced by this method are
    // readable for the duration of the call.
    pub(crate) unsafe fn get_exception_table(&self) -> StResult<(u32, u32)> {
        // Get the PE header offset.
        let pe_header_offset = self.bytes.read32(PE_POINTER_OFFSET)? as usize;

        // Only PE32+ images are supported.
        if self.bytes.read16(pe_header_offset + PE_MAGIC_OFFSET)? != PE64_EXECUTABLE {
            return Err(Error::Malformed { module: self.image_name, reason: "Image is not a PE32+ image" });
        }

        // Jump to the exception table data directory and read the exception table
        // RVA and the exception table section size.
        let offset = pe_header_offset + EXCEPTION_TABLE_POINTER_OFFSET;
        let exception_table_rva = self.bytes.read32(offset)?;
        let exception_table_size = self.bytes.read32(offset + 4)?;

        // Bail out if the exception table section (the `.pdata` section) is not
        // available.
        if exception_table_rva == 0 || exception_table_size == 0 {
            return Err(Error::ExceptionDirectoryNotFound { module: self.image_name });
        }

        Ok((exception_table_rva, exception_table_size))
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::error::Error;

    const PE_HEADER_OFFSET: usize = 0x80;
    const PE32_EXECUTABLE: u16 = 0x10B;
    const DEBUG_DIRECTORY_RVA: usize = 0x400;
    const DEBUG_DATA_RVA: usize = 0x800;

    /// Helper: create a minimal fake PE image in memory for the given
    /// optional-header magic.
    fn make_fake_pe_image_of_type(pe_type: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x2000]; // 8 KB buffer to simulate PE image

        // DOS header ('MZ')
        bytes[0..2].copy_from_slice(&MZ_SIGNATURE.to_le_bytes());

        // PE header pointer at 0x3C -> points to offset 0x80
        bytes[PE_POINTER_OFFSET..PE_POINTER_OFFSET + 4].copy_from_slice(&(PE_HEADER_OFFSET as u32).to_le_bytes());

        // Write PE signature ('PE\0\0') at 0x80
        bytes[PE_HEADER_OFFSET..PE_HEADER_OFFSET + 4].copy_from_slice(&PE_SIGNATURE.to_le_bytes());

        // Optional header magic
        let magic_offset = PE_HEADER_OFFSET + PE_MAGIC_OFFSET;
        bytes[magic_offset..magic_offset + 2].copy_from_slice(&pe_type.to_le_bytes());

        // SizeOfImage (at +0x50)
        let size_of_image = 0x2000u32;
        let size_of_image_offset = PE_HEADER_OFFSET + SIZE_OF_IMAGE_OFFSET;
        bytes[size_of_image_offset..size_of_image_offset + 4].copy_from_slice(&size_of_image.to_le_bytes());

        // NumberOfRvaAndSizes
        let count_offset = PE_HEADER_OFFSET + NUMBER_OF_RVA_AND_SIZES_OFFSET;
        bytes[count_offset..count_offset + 4].copy_from_slice(&16u32.to_le_bytes());

        // Debug directory pointer (RVA + size)
        let debug_dir_offset = PE_HEADER_OFFSET + DEBUG_DIRECTORY_POINTER_OFFSET;
        bytes[debug_dir_offset..debug_dir_offset + 4].copy_from_slice(&(DEBUG_DIRECTORY_RVA as u32).to_le_bytes());
        bytes[debug_dir_offset + 4..debug_dir_offset + 8]
            .copy_from_slice(&(DEBUG_DIRECTORY_ENTRY_SIZE as u32).to_le_bytes());

        // Debug directory (1 entry)
        // IMAGE_DEBUG_DIRECTORY.Type = 2 (CodeView)
        let debug_type_offset = DEBUG_DIRECTORY_RVA + DEBUG_RECORD_TYPE_OFFSET;
        bytes[debug_type_offset..debug_type_offset + 4].copy_from_slice(&DEBUG_RECORD_TYPE_CODEVIEW.to_le_bytes());
        // Debug data RVA and size
        let debug_rva_off = DEBUG_DIRECTORY_RVA + DEBUG_RECORD_RVA_OFFSET;
        bytes[debug_rva_off..debug_rva_off + 4].copy_from_slice(&(DEBUG_DATA_RVA as u32).to_le_bytes());
        let debug_size_off = DEBUG_DIRECTORY_RVA + DEBUG_RECORD_SIZE;
        bytes[debug_size_off..debug_size_off + 4].copy_from_slice(&0x100u32.to_le_bytes());

        // CodeView data section
        bytes[DEBUG_DATA_RVA..DEBUG_DATA_RVA + 4].copy_from_slice(&CODEVIEW_PDB70_SIGNATURE.to_le_bytes());

        // Insert a fake PDB path (RSDS... + "C:\\path\\app.exe\0")
        let fake_pdb_path = b"C:\\path\\app.exe\0";
        let name_off = DEBUG_DATA_RVA + CODEVIEW_PDB70_FILE_NAME_OFFSET;
        bytes[name_off..name_off + fake_pdb_path.len()].copy_from_slice(fake_pdb_path);

        bytes
    }

    /// Helper: create a minimal fake PE32+ image in memory.
    fn make_fake_pe_image() -> Vec<u8> {
        make_fake_pe_image_of_type(PE64_EXECUTABLE)
    }

    /// Helper: overwrite the `CodeView` record `SizeOfData` field.
    fn set_debug_data_size(bytes: &mut [u8], size: u32) {
        let offset = DEBUG_DIRECTORY_RVA + DEBUG_RECORD_SIZE;
        bytes[offset..offset + 4].copy_from_slice(&size.to_le_bytes());
    }

    /// Helper: overwrite the `CodeView` record `AddressOfRawData` field.
    fn set_debug_data_rva(bytes: &mut [u8], rva: u32) {
        let offset = DEBUG_DIRECTORY_RVA + DEBUG_RECORD_RVA_OFFSET;
        bytes[offset..offset + 4].copy_from_slice(&rva.to_le_bytes());
    }

    /// Helper: overwrite the PDB path stored in the PDB70 `CodeView` record.
    fn set_pdb_path(bytes: &mut [u8], path: &[u8]) {
        let name_off = DEBUG_DATA_RVA + CODEVIEW_PDB70_FILE_NAME_OFFSET;
        bytes[name_off..name_off + 0x100 - CODEVIEW_PDB70_FILE_NAME_OFFSET].fill(0xFF);
        bytes[name_off..name_off + path.len()].copy_from_slice(path);
    }

    fn image_name_of(bytes: &[u8]) -> Option<&str> {
        PE::get_image_name(bytes, PE_HEADER_OFFSET)
    }

    #[test]
    fn test_locate_image_success() {
        let bytes = make_fake_pe_image();
        let base = bytes.as_ptr() as u64;

        let pe = PE { base_address: base, size_of_image: bytes.len() as u32, image_name: Some("fake"), bytes: &bytes };

        // Since we didn't define exception table fields, expect an error.
        // SAFETY: Test creates a fake PE image structure for validation; `pe.bytes` points to a valid slice
        // in that file.
        assert!(matches!(unsafe { pe.get_exception_table() }, Err(Error::ExceptionDirectoryNotFound { .. })));
    }

    #[test]
    fn test_get_image_name_success() {
        let bytes = make_fake_pe_image();
        assert_eq!(image_name_of(&bytes), Some("app"));
    }

    #[test]
    fn test_try_parse_image_accepts_pe32_plus_image() {
        let bytes = make_fake_pe_image();
        // SAFETY: The fixture is a 8 KB buffer whose `SizeOfImage` matches its
        // length, so both the page probe and the image slice stay in bounds.
        let image = unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.expect("PE32+ image must parse");
        assert_eq!(image.image_name, Some("app"));
        assert_eq!(image.size_of_image, bytes.len() as u32);
    }

    #[test]
    fn test_try_parse_image_rejects_pe32_image() {
        // PE32 images are not supported, so they must be skipped rather than
        // parsed with PE32+ data-directory offsets.
        let bytes = make_fake_pe_image_of_type(PE32_EXECUTABLE);
        // SAFETY: Same fixture layout as the PE32+ case above.
        assert!(unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.is_none());
    }

    #[test]
    fn test_try_parse_image_rejects_page_without_mz_signature() {
        let mut bytes = make_fake_pe_image();
        bytes[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        // SAFETY: Same fixture layout as the PE32+ case above.
        assert!(unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.is_none());
    }

    #[test]
    fn test_try_parse_image_rejects_mz_page_without_pe_signature() {
        // A page may begin with MZ by chance. Without a PE header it must be
        // skipped so the caller keeps scanning.
        let mut bytes = make_fake_pe_image();
        bytes[PE_HEADER_OFFSET..PE_HEADER_OFFSET + 4].copy_from_slice(&0u32.to_le_bytes());
        // SAFETY: Same fixture layout as the PE32+ case above.
        assert!(unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.is_none());
    }

    #[test]
    fn test_try_parse_image_rejects_pe_header_offset_outside_page() {
        let mut bytes = make_fake_pe_image();
        let outside = (PAGE_SIZE as u32) + 0x100;
        bytes[PE_POINTER_OFFSET..PE_POINTER_OFFSET + 4].copy_from_slice(&outside.to_le_bytes());
        // SAFETY: Same fixture layout as the PE32+ case above.
        assert!(unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.is_none());
    }

    #[test]
    fn test_try_parse_image_rejects_size_of_image_below_headers() {
        // SizeOfImage must at least span the headers that were just parsed.
        let mut bytes = make_fake_pe_image();
        let offset = PE_HEADER_OFFSET + SIZE_OF_IMAGE_OFFSET;
        bytes[offset..offset + 4].copy_from_slice(&(offset as u32).to_le_bytes());
        // SAFETY: Same fixture layout as the PE32+ case above.
        assert!(unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.is_none());
    }

    #[test]
    fn test_try_parse_image_without_debug_directory_has_no_name() {
        // A valid image with no usable debug directory is still returned, just
        // without a module name.
        let mut bytes = make_fake_pe_image();
        let count_offset = PE_HEADER_OFFSET + NUMBER_OF_RVA_AND_SIZES_OFFSET;
        bytes[count_offset..count_offset + 4].copy_from_slice(&DEBUG_DIRECTORY_INDEX.to_le_bytes());
        // SAFETY: Same fixture layout as the PE32+ case above.
        let image = unsafe { PE::try_parse_image(bytes.as_ptr() as u64) }.expect("image must still parse");
        assert_eq!(image.image_name, None);
    }

    #[test]
    fn test_get_exception_table_rejects_pe32_image() {
        let bytes = make_fake_pe_image_of_type(PE32_EXECUTABLE);
        let pe = PE { base_address: 0, size_of_image: bytes.len() as u32, image_name: None, bytes: &bytes };
        // SAFETY: `pe.bytes` points to a valid in-memory fixture.
        assert!(matches!(unsafe { pe.get_exception_table() }, Err(Error::Malformed { .. })));
    }

    #[test]
    fn test_get_image_name_failure_invalid_signature() {
        let mut bytes = make_fake_pe_image();
        // Corrupt the signature
        bytes[DEBUG_DATA_RVA..DEBUG_DATA_RVA + 4].copy_from_slice(&0x12345678u32.to_le_bytes());
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_nb10_signature() {
        let mut bytes = make_fake_pe_image();

        // Overwrite the CodeView signature with NB10.
        bytes[DEBUG_DATA_RVA..DEBUG_DATA_RVA + 4].copy_from_slice(&CODEVIEW_SIGNATURE_NB10.to_le_bytes());

        // Write a fake PDB path at the NB10 file name offset (0x10).
        let fake_pdb_path = b"/home/build/driver.dll\0";
        let name_off = DEBUG_DATA_RVA + CODEVIEW_NB10_FILE_NAME_OFFSET;
        bytes[name_off..name_off + fake_pdb_path.len()].copy_from_slice(fake_pdb_path);

        assert_eq!(image_name_of(&bytes), Some("driver"));
    }

    #[test]
    fn test_get_image_name_rejects_size_of_data_smaller_than_codeview_header() {
        // `SizeOfData` values below the fixed CodeView header size must not
        // underflow the file name length computation.
        for size in 1..CODEVIEW_PDB70_FILE_NAME_OFFSET as u32 {
            let mut bytes = make_fake_pe_image();
            set_debug_data_size(&mut bytes, size);
            assert_eq!(image_name_of(&bytes), None, "SizeOfData = {size} must be rejected");
        }
    }

    #[test]
    fn test_get_image_name_rejects_empty_name() {
        let mut bytes = make_fake_pe_image();
        set_debug_data_size(&mut bytes, CODEVIEW_PDB70_FILE_NAME_OFFSET as u32);
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_debug_data_outside_image() {
        let mut bytes = make_fake_pe_image();
        set_debug_data_rva(&mut bytes, 0x7FFF_0000);
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_debug_data_crossing_image_end() {
        let mut bytes = make_fake_pe_image();
        let rva = bytes.len() as u32 - 0x10;
        set_debug_data_rva(&mut bytes, rva);
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_debug_directory_outside_image() {
        let mut bytes = make_fake_pe_image();
        let debug_dir_offset = PE_HEADER_OFFSET + DEBUG_DIRECTORY_POINTER_OFFSET;
        bytes[debug_dir_offset..debug_dir_offset + 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_partial_debug_directory_entry() {
        let mut bytes = make_fake_pe_image();
        let debug_dir_offset = PE_HEADER_OFFSET + DEBUG_DIRECTORY_POINTER_OFFSET;
        bytes[debug_dir_offset + 4..debug_dir_offset + 8]
            .copy_from_slice(&(DEBUG_DIRECTORY_ENTRY_SIZE as u32 - 1).to_le_bytes());
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_missing_debug_data_directory() {
        let mut bytes = make_fake_pe_image();
        // Declare fewer data directories than the debug directory index.
        let count_offset = PE_HEADER_OFFSET + NUMBER_OF_RVA_AND_SIZES_OFFSET;
        bytes[count_offset..count_offset + 4].copy_from_slice(&DEBUG_DIRECTORY_INDEX.to_le_bytes());
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_stops_at_nul_terminator() {
        let mut bytes = make_fake_pe_image();
        // A path without an extension followed by trailing record padding.
        set_pdb_path(&mut bytes, b"\\build\\driver\0");
        assert_eq!(image_name_of(&bytes), Some("driver"));
    }

    #[test]
    fn test_get_image_name_ignores_padding_after_nul_terminator() {
        let mut bytes = make_fake_pe_image();
        // Trailing padding contains a '.' that must not influence the result.
        set_pdb_path(&mut bytes, b"\\build\\driver.pdb\0trailing.junk");
        assert_eq!(image_name_of(&bytes), Some("driver"));
    }

    #[test]
    fn test_get_image_name_rejects_non_utf8_path() {
        let mut bytes = make_fake_pe_image();
        set_pdb_path(&mut bytes, b"\\build\\dr\xFFiver.pdb\0");
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_overlong_name() {
        let mut bytes = make_fake_pe_image();
        // Enlarge the CodeView record so the name is bounded by the length cap
        // rather than by the record size.
        set_debug_data_size(&mut bytes, 0x400);
        let name_off = DEBUG_DATA_RVA + CODEVIEW_PDB70_FILE_NAME_OFFSET;
        let mut path = vec![b'a'; MAX_IMAGE_NAME_LENGTH + 1];
        path.push(0);
        bytes[name_off..name_off + path.len()].copy_from_slice(&path);
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_accepts_long_path_with_short_name() {
        let mut bytes = make_fake_pe_image();
        // Build paths embedded by the toolchain are often longer than the name
        // cap. Only the name that gets logged is bounded.
        set_debug_data_size(&mut bytes, 0x400);
        let mut path = Vec::new();
        for _ in 0..40 {
            path.extend_from_slice(b"\\a_long_build_directory");
        }
        path.extend_from_slice(b"\\driver.pdb\0");
        assert!(path.len() > MAX_IMAGE_NAME_LENGTH);
        let name_off = DEBUG_DATA_RVA + CODEVIEW_PDB70_FILE_NAME_OFFSET;
        bytes[name_off..name_off + path.len()].copy_from_slice(&path);
        assert_eq!(image_name_of(&bytes), Some("driver"));
    }

    #[test]
    fn test_get_image_name_rejects_path_ending_in_separator() {
        let mut bytes = make_fake_pe_image();
        set_pdb_path(&mut bytes, b"C:\\path\\\0");
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_extension_only_name() {
        let mut bytes = make_fake_pe_image();
        set_pdb_path(&mut bytes, b"C:\\path\\.pdb\0");
        assert_eq!(image_name_of(&bytes), None);
    }

    #[test]
    fn test_get_image_name_rejects_zero_debug_data_rva() {
        let mut bytes = make_fake_pe_image();
        set_debug_data_rva(&mut bytes, 0);
        assert_eq!(image_name_of(&bytes), None);
    }
}
