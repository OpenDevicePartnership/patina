use crate::byte_reader::ByteReader;
use crate::error::{Error, StResult};

// PE Header related constants
const MZ_SIGNATURE: u16 = 0x5A4D; // 'MZ' in little-endian.
const PAGE_SIZE: u64 = 0x1000; // 4KB pages.
const PE_MAGIC_OFFSET: usize = 0x18;
const PE_POINTER_OFFSET: usize = 0x3C;
const PE_SIGNATURE: u32 = 0x0000_4550; // 'PE\0\0' in little-endian.
const PE64_EXECUTABLE: u16 = 0x20B; // PE32+
const SIZE_OF_IMAGE_OFFSET: usize = 0x50;
const EXCEPTION_TABLE_POINTER_PE32_OFFSET: usize = 0x90;
const EXCEPTION_TABLE_POINTER_PE64_OFFSET: usize = 0xA0;

// PE Debug directory related constants
const DEBUG_DIRECTORY_POINTER_PE64_OFFSET: usize = EXCEPTION_TABLE_POINTER_PE64_OFFSET + 0x18;
const DEBUG_DIRECTORY_ENTRY_SIZE: usize = 0x1C;
const DEBUG_RECORD_RVA_OFFSET: usize = 0x14;
const DEBUG_RECORD_SIZE: usize = 0x10;
const DEBUG_RECORD_TYPE_OFFSET: usize = 0xC;
const DEBUG_RECORD_TYPE_CODEVIEW: u32 = 0x2; // 2 => The Visual C++ debug information.
const CODEVIEW_PDB70_SIGNATURE: u32 = 0x5344_5352; // RSDS
const CODEVIEW_PDB_FILE_NAME_OFFSET: usize = 0x18;

/// Module to provide in-memory PE file parsing
#[derive(Clone)]
pub struct PE<'a> {
    /// image base of the pe image in memory
    pub base_address: u64,

    /// size of the image in memory
    pub _size_of_image: u32,

    /// image name extracted from the loaded pe image
    pub image_name: Option<&'static str>,

    /// loaded image memory as a byte slice
    pub(crate) bytes: &'a [u8],
}

impl PE<'_> {
    /// Locate the image corresponding to the rip
    pub(crate) unsafe fn locate_image(mut rip: u64) -> StResult<Self> {
        let original_rip = rip;

        // Align to the start of a page
        rip &= !(PAGE_SIZE - 1);

        // Grok each 4K page in memory to identify the PE image corresponding to
        // the given rip
        while rip > 0 {
            // Convert the 4K page into a slice to make it easier to interpret the fields
            let page = unsafe { core::slice::from_raw_parts(rip as *const u8, PAGE_SIZE as usize) };

            // Check if the page begins with 'MZ' signature
            let dos_header_signature = page.read16(0)?;
            if dos_header_signature == MZ_SIGNATURE {
                // 'MZ' on a page boundary is not very common. But still, lets
                // do little bit more validation
                let pe_header_offset = page.read32(PE_POINTER_OFFSET)? as usize;
                let pe_header_signature = page.read32(pe_header_offset)?;

                // Check if it is indeed a valid PE header
                if pe_header_signature == PE_SIGNATURE {
                    // This field contains the size of entire loaded image in memory
                    let size_of_image = page.read32(pe_header_offset + SIZE_OF_IMAGE_OFFSET)?;

                    // Parse debug directory to process the image name later
                    let debug_directory_rva =
                        page.read32(pe_header_offset + DEBUG_DIRECTORY_POINTER_PE64_OFFSET).unwrap_or(0) as usize;
                    let debug_directory_size =
                        page.read32(pe_header_offset + DEBUG_DIRECTORY_POINTER_PE64_OFFSET + 4).unwrap_or(0) as usize;

                    // Identify the image name
                    let image_name = if debug_directory_size != 0 {
                        unsafe { Self::get_image_name(rip, debug_directory_rva, debug_directory_size) }
                    } else {
                        None
                    };

                    let bytes = unsafe { core::slice::from_raw_parts(rip as *const u8, size_of_image as usize) };

                    return Ok(Self { base_address: rip, _size_of_image: size_of_image, image_name, bytes });
                }
            }

            // Move one page before.
            rip -= PAGE_SIZE;
        }

        // Something is really bad with given rip
        Err(Error::ImageNotFound(original_rip))
    }

    /// Private function to locate the image name in the memory.
    unsafe fn get_image_name(
        page_base: u64,
        debug_directory_rva: usize,
        debug_directory_size: usize,
    ) -> Option<&'static str> {
        // Convert the debug data section into a slice to make it easier to interpret the fields.
        let debug_directory = unsafe {
            core::slice::from_raw_parts((page_base + debug_directory_rva as u64) as *const u8, debug_directory_size)
        };

        // - Break the debug directory into individual entries
        // - Filter entries of type IMAGE_DEBUG_TYPE_CODEVIEW (2)
        // - Extract the debug data RVA and its size
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
            // Bail out if this is not found
            return None;
        };

        if debug_data_rva == 0 || debug_data_size == 0 {
            return None;
        };

        let debug_data = page_base + debug_data_rva as u64;

        // Check codeview signature
        let codeview_signature = unsafe { *(debug_data as *const u32) };
        if codeview_signature != CODEVIEW_PDB70_SIGNATURE {
            return None;
        }

        // Extract the PDB file path
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
        if let Some(file_name_with_ext) = file_name.rsplit('\\').next() {
            if let Some((file_name, _ext)) = file_name_with_ext.rsplit_once('.') {
                return Some(file_name);
            }
        }

        // log::info!("Pdb file name : {}", file_name);

        Some(file_name)
    }

    pub(crate) unsafe fn get_exception_table(&self) -> StResult<(u32, u32)> {
        // Get PE Header offset
        let pe_header_offset = self.bytes.read32(PE_POINTER_OFFSET)? as usize;

        // Determine PE Type(PE32 or PE32+)
        let pe_type = self.bytes.read16(pe_header_offset + PE_MAGIC_OFFSET)?;

        // Jump to exception table data directory and read the exception table
        // rva
        let offset = if pe_type == PE64_EXECUTABLE {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE64_OFFSET
        } else {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE32_OFFSET
        };
        let exception_table_rva = self.bytes.read32(offset)?;

        // Jump to exception table section size
        let offset = if pe_type == PE64_EXECUTABLE {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE64_OFFSET + 4
        } else {
            pe_header_offset + EXCEPTION_TABLE_POINTER_PE32_OFFSET + 4
        };
        let exception_table_size = self.bytes.read32(offset)?;

        // Bail out if exception table section(aka .pdata section) is not
        // available
        if exception_table_rva == 0 || exception_table_size == 0 {
            return Err(Error::ExceptionDirectoryNotFound(self.image_name));
        }

        Ok((exception_table_rva, exception_table_size))
    }
}
