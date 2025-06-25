use alloc::vec::Vec;
use core::{mem, ptr, slice};
use patina_sdk::base::align_up;
use r_efi::efi;

use mu_pi::fw_fs::{
    ffs::{self, file},
    fv::{self, BlockMapEntry},
    fvb,
};

use crate::{file::FileRef, FirmwareFileSystemError};

pub struct VolumeRef<'a> {
    data: &'a [u8],
    fv_header: fv::Header,
    ext_header: Option<fv::ExtHeader>,
    block_map: Vec<fv::BlockMapEntry>,
    content_offset: usize,
}

impl<'a> VolumeRef<'a> {
    pub fn new(buffer: &'a [u8]) -> Result<Self, FirmwareFileSystemError> {
        // Verify that buffer has enough storage for a volume header.
        if buffer.len() < mem::size_of::<fv::Header>() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Safety: buffer is large enough to contain the header.
        let fv_header = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const fv::Header) };

        // Signature must be ASCII '_FVH'
        if fv_header.signature != u32::from_le_bytes(*b"_FVH") {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        let header_length = fv_header.header_length as usize;
        // Header length must be large enough to hold the header
        if header_length < mem::size_of::<fv::Header>() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Header length must fit inside the buffer.
        if header_length > buffer.len() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Header length must be a multiple of 2
        if header_length & 0x01 != 0 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Header checksum must be correct
        let header_slice = &buffer[..header_length];
        let sum = header_slice
            .chunks_exact(2)
            .fold(0u16, |sum, value| sum.wrapping_add(u16::from_le_bytes(value.try_into().unwrap())));
        if sum != 0 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // revision: must be at least fv::FFS_REVISION
        if fv_header.revision < fv::FFS_REVISION {
            Err(FirmwareFileSystemError::Unsupported)?;
        }

        // file_system_guid: must be EFI_FIRMWARE_FILE_SYSTEM2_GUID or EFI_FIRMWARE_FILE_SYSTEM3_GUID.
        if fv_header.file_system_guid != ffs::guid::EFI_FIRMWARE_FILE_SYSTEM2_GUID
            && fv_header.file_system_guid != ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID
        {
            Err(FirmwareFileSystemError::Unsupported)?;
        }

        // fv_length: must be large enough to hold the header.
        if fv_header.fv_length < header_length as u64 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // fv_length: must be less than or equal to fv_data buffer length
        if fv_header.fv_length > buffer.len() as u64 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        //ext_header_offset: must be inside the fv
        if fv_header.ext_header_offset as u64 > fv_header.fv_length {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        //if ext_header is present, its size must fit inside the FV.
        let ext_header = {
            if fv_header.ext_header_offset != 0 {
                let ext_header_offset = fv_header.ext_header_offset as usize;
                if ext_header_offset + mem::size_of::<fv::ExtHeader>() > buffer.len() {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }

                //Safety: previous check ensures that fv_data is large enough to contain the ext_header
                let ext_header = unsafe { ptr::read_unaligned(buffer[ext_header_offset..].as_ptr() as *const fv::ExtHeader) };
                let ext_header_end = ext_header_offset + ext_header.ext_header_size as usize;
                if ext_header_end > buffer.len() {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                Some(ext_header)
            } else {
                None
            }
        };

        //block map must fit within the fv header (which is checked above to guarantee it is within the fv_data buffer).
        let block_map = &buffer[mem::size_of::<fv::Header>()..fv_header.header_length as usize];

        //block map should be a multiple of 8 in size
        if block_map.len() & 0x7 != 0 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        let mut block_map = block_map
            .chunks_exact(8)
            .map(|x| fv::BlockMapEntry {
                num_blocks: u32::from_le_bytes(x[..4].try_into().unwrap()),
                length: u32::from_le_bytes(x[4..].try_into().unwrap()),
            })
            .collect::<Vec<_>>();

        //block map should terminate with zero entry
        if block_map.last() != Some(&fv::BlockMapEntry { num_blocks: 0, length: 0 }) {
            Err(FirmwareFileSystemError::InvalidBlockMap)?;
        }

        //remove the terminator.
        block_map.pop();

        //thre must be at least one valid entry in the block map.
        if block_map.is_empty() {
            Err(FirmwareFileSystemError::InvalidBlockMap)?;
        }

        //other entries in block map must be non-zero.
        if block_map.iter().any(|x| x == &fv::BlockMapEntry { num_blocks: 0, length: 0 }) {
            Err(FirmwareFileSystemError::InvalidBlockMap)?;
        }

        let content_offset = {
            if let Some(ext_header) = &ext_header {
                // if ext header exists, then data starts after ext header
                fv_header.ext_header_offset as usize + ext_header.ext_header_size as usize
            } else {
                // otherwise data starts after the fv_header.
                fv_header.header_length as usize
            }
        };

        Ok(Self { data: buffer, fv_header, ext_header, block_map, content_offset })
    }

    /// Instantiate a new FirmwareVolume from a base address.
    ///
    /// ## Safety
    /// Caller must ensure that base_address is the address of the start of a firmware volume.
    /// Caller must ensure that the lifetime of the buffer at base_address is longer than the
    /// returned VolumeRef.
    ///
    pub unsafe fn new_from_address(base_address: u64) -> Result<Self, FirmwareFileSystemError> {
        let fv_header = &*(base_address as *const fv::Header);
        if fv_header.signature != u32::from_le_bytes(*b"_FVH") {
            // base_address is not the start of a firmware volume.
            return Err(FirmwareFileSystemError::DataCorrupt);
        }

        let fv_buffer = slice::from_raw_parts(base_address as *const u8, fv_header.fv_length as usize);
        Self::new(fv_buffer)
    }

    pub fn erase_byte(&self) -> u8 {
        if self.fv_header.attributes & fvb::attributes::raw::fvb2::ERASE_POLARITY != 0 {
            0xff
        } else {
            0
        }
    }

    pub fn ext_header(&self) -> Option<fv::ExtHeader> {
        self.ext_header
    }

    pub fn fv_name(&self) -> Option<efi::Guid> {
        self.ext_header().map(|x| x.fv_name)
    }

    pub fn block_map(&self) -> &Vec<BlockMapEntry> {
        &self.block_map
    }

    pub fn lba_info(&self, lba: u32) -> Result<(u32, u32, u32), FirmwareFileSystemError> {
        let block_map = self.block_map();

        let mut total_blocks = 0;
        let mut offset = 0;
        let mut block_size = 0;

        for entry in block_map {
            total_blocks += entry.num_blocks;
            block_size = entry.length;
            if lba < total_blocks {
                break;
            }
            offset += entry.num_blocks * entry.length;
        }

        if lba >= total_blocks {
            return Err(FirmwareFileSystemError::InvalidParameter); //lba out of range.
        }

        let remaining_blocks = total_blocks - lba;
        Ok((offset + lba * block_size, block_size, remaining_blocks))
    }

    pub fn attributes(&self) -> fvb::attributes::EfiFvbAttributes2 {
        self.fv_header.attributes
    }

    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }

    pub fn files(&self) -> impl Iterator<Item = Result<FileRef<'a>, FirmwareFileSystemError>> {
        FileRefIter::new(&self.data[self.content_offset..], self.erase_byte())
    }
}

struct FileRefIter<'a> {
    data: &'a [u8],
    next_offset: usize,
    erase_byte: u8,
    error: bool,
}

impl<'a> FileRefIter<'a> {
    pub fn new(data: &'a [u8], erase_byte: u8) -> Self {
        Self { data, next_offset: 0, erase_byte, error: false }
    }
}

impl<'a> Iterator for FileRefIter<'a> {
    type Item = Result<FileRef<'a>, FirmwareFileSystemError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.error {
            return None;
        }
        if self.next_offset > self.data.len() {
            return None;
        }
        if self.data[self.next_offset..].len() < mem::size_of::<file::Header>() {
            return None;
        }
        if self.data[self.next_offset..self.next_offset + mem::size_of::<file::Header>()]
            .iter()
            .all(|&x| x == self.erase_byte)
        {
            return None;
        }
        let result = FileRef::new(&self.data[self.next_offset..]);
        if let Ok(ref file) = result {
            // per the PI spec, "Given a file F, the next file FvHeader is located at the next 8-byte aligned firmware volume
            // offset following the last byte the file F"
            match align_up(self.next_offset as u64 + file.size() as u64, 8) {
                Ok(offset) => {
                    self.next_offset += offset as usize;
                }
                Err(_) => {
                    self.error = true;
                    return Some(Err(FirmwareFileSystemError::DataCorrupt));
                }
            }
        } else {
            self.error = true;
        }
        Some(result)
    }
}
