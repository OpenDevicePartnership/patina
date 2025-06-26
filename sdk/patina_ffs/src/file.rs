use mu_pi::fw_fs::{
    ffs::{self, attributes, file},
    fv,
};

use crate::{
    section::{Section, SectionExtractor, SectionIterator},
    FirmwareFileSystemError,
};

use alloc::{vec, vec::Vec};
use core::{mem, ptr};
use r_efi::efi;

#[derive(Clone)]
pub struct FileRef<'a> {
    data: &'a [u8],
    header: file::Header,
    size: usize,
    content_offset: usize,
}

impl<'a> FileRef<'a> {
    pub fn new(buffer: &'a [u8]) -> Result<Self, FirmwareFileSystemError> {
        // Verify that buffer has enough storage for a file header.
        if buffer.len() < mem::size_of::<file::Header>() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // safety: buffer is large enough to contain file header.
        let header = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const file::Header) };

        // determine actual size and content_offset
        let (size, content_offset) = {
            if (header.attributes & attributes::raw::LARGE_FILE) == 0 {
                //standard header with 24-bit size.
                let mut size = vec![00u8; 4];
                size[0..3].copy_from_slice(&header.size);
                let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
                (size, mem::size_of::<file::Header>())
            } else {
                //extended header with 64-bit size.
                if buffer.len() < mem::size_of::<file::Header2>() {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // safety: buffer is large enough to contain file header.
                let header = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const file::Header2) };
                (header.extended_size as usize, mem::size_of::<file::Header2>())
            }
        };

        // Verify that the total size of the file fits within the buffer.
        if size > buffer.len() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Verify the state field.
        // Interpreting the state field requires knowledge of the EFI_FVB_ERASE_POLARITY from the FV header, which is not
        // available here unless the constructor API is modified to specify it. So it is inferred based on the state of
        // the reserved bits in the EFI_FFS_FILE_STATE which spec requires to be set to EFI_FVB_ERASE_POLARITY.
        // This implementation does not support FV modification, so the only valid state is EFI_FILE_DATA_VALID.
        if (header.state & 0x80) == 0 {
            //erase polarity = 0. Verify DATA_VALID is set, and no higher-order bits are set.
            if header.state & 0xFC != file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(FirmwareFileSystemError::InvalidState)?;
            }
        } else {
            //erase polarity = 1. Verify DATA_VALID is clear, and no higher-order bits are clear.
            if (!header.state) & 0xFC != file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(FirmwareFileSystemError::InvalidState)?;
            }
        }

        // Verify the file data checksum.
        if header.attributes & attributes::raw::CHECKSUM == 0 {
            if header.integrity_check_file != 0xAA {
                Err(FirmwareFileSystemError::InvalidHeader)?;
            }
        } else {
            let sum = buffer[content_offset..size].iter().fold(0u8, |sum, val| sum.wrapping_add(*val));
            if sum != 0 {
                Err(FirmwareFileSystemError::DataCorrupt)?;
            }
        }
        Ok(Self { data: &buffer[..size], header, size, content_offset })
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn name(&self) -> efi::Guid {
        self.header.name
    }

    pub fn file_type_raw(&self) -> u8 {
        self.header.file_type
    }

    pub fn content(&self) -> &[u8] {
        &self.data[self.content_offset..]
    }

    pub fn data(&self) -> &[u8] {
        self.data
    }

    pub fn fv_attributes(&self) -> fv::file::EfiFvFileAttributes {
        let attributes = self.header.attributes;
        let data_alignment = (attributes & ffs::attributes::raw::DATA_ALIGNMENT) >> 3;
        // decode alignment per Table 3.3 in PI spec 1.8 Part III.
        let mut file_attributes: u32 = match (
            data_alignment,
            (attributes & ffs::attributes::raw::DATA_ALIGNMENT_2) == ffs::attributes::raw::DATA_ALIGNMENT_2,
        ) {
            (0, false) => 0,
            (1, false) => 4,
            (2, false) => 7,
            (3, false) => 9,
            (4, false) => 10,
            (5, false) => 12,
            (6, false) => 15,
            (7, false) => 16,
            (x @ 0..=7, true) => (17 + x) as u32,
            (_, _) => panic!("Invalid data_alignment!"),
        };
        if attributes & ffs::attributes::raw::FIXED != 0 {
            file_attributes |= fv::file::raw::attribute::FIXED;
        }
        file_attributes as fv::file::EfiFvFileAttributes
    }

    pub fn attributes_raw(&self) -> u8 {
        self.header.attributes
    }

    pub fn sections(&self) -> Result<Vec<Section>, FirmwareFileSystemError> {
        let sections = SectionIterator::new(&self.data[self.content_offset..])
            .collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;
        Ok(sections.iter().flat_map(|x| x.sections().cloned().collect::<Vec<_>>()).collect())
    }

    pub fn sections_with_extractor(
        &self,
        extractor: &dyn SectionExtractor,
    ) -> Result<Vec<Section>, FirmwareFileSystemError> {
        let sections = SectionIterator::new(&self.data[self.content_offset..])
            .map(|mut x| {
                if let Ok(ref mut section) = x {
                    section.extract(extractor)?;
                }
                x
            })
            .collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;
        Ok(sections.iter().flat_map(|x| x.sections().cloned().collect::<Vec<_>>()).collect())
    }
}
