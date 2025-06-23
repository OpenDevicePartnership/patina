use mu_pi::fw_fs::{
    ffs::{attributes, file},
    FfsRawAttribute::LARGE_FILE,
};

use crate::{section::Section, FirmwareFileSystemError};

use alloc::{vec, vec::Vec};
use core::mem;
use patina_sdk::base::align_up;
use r_efi::efi;

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
        let header = unsafe { *(buffer.as_ptr() as *const file::Header) };

        // determine actual size and content_offset
        let (size, content_offset) = {
            if (header.attributes & LARGE_FILE) == 0 {
                //standard header with 24-bit size.
                let mut size = vec![00u8; 4];
                size[0..2].copy_from_slice(&header.size);
                let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
                (size, mem::size_of::<file::Header>())
            } else {
                //extended header with 64-bit size.
                if buffer.len() < mem::size_of::<file::Header2>() {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // safety: buffer is large enough to contain file header.
                let header = unsafe { *(buffer.as_ptr() as *const file::Header2) };
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

    pub fn sections(&self) -> Result<Vec<Section>, FirmwareFileSystemError> {
        let sections = FileSectionIterator::new(self).collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;
        Ok(sections.iter().flat_map(|x| x.sections().cloned().collect::<Vec<_>>()).collect())
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn name(&self) -> efi::Guid {
        self.header.name
    }
}

struct FileSectionIterator<'a> {
    file: &'a FileRef<'a>,
    next_offset: usize,
    error: bool,
}

impl<'a> FileSectionIterator<'a> {
    pub fn new(file: &'a FileRef<'a>) -> Self {
        Self { file, next_offset: file.content_offset, error: false }
    }
}

impl Iterator for FileSectionIterator<'_> {
    type Item = Result<Section, FirmwareFileSystemError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.error {
            return None;
        }

        if self.next_offset >= self.file.data.len() {
            return None;
        }

        let result = Section::new(&self.file.data[self.next_offset..]);
        match result {
            Ok(ref section) => {
                let section_size = section.size().expect("Section must be composed");
                self.next_offset += match align_up(section_size as u64, 4) {
                    Ok(addr) => addr as usize,
                    Err(_) => {
                        self.error = true;
                        return Some(Err(FirmwareFileSystemError::DataCorrupt));
                    }
                };
            }
            Err(_) => self.error = true,
        }
        Some(result)
    }
}
