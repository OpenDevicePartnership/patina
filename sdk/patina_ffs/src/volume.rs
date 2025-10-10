//! Firmware Volume (FV) parsing, inspection, and composition.
//!
//! This module provides:
//! - `VolumeRef`: a zero-copy, read-only view over a serialized FV backed by a byte slice.
//! - `Volume`: an owned builder for assembling FVs from a block map and FFS files and serializing.
//!
//! It validates FV headers and block maps, iterates contained files, and serializes with proper
//! alignment, optional extended headers, and checksum calculation per the PI specification.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use alloc::vec::Vec;
use core::{
    fmt, iter, mem, ptr,
    slice::{self, from_raw_parts},
};
use patina::base::align_up;
use r_efi::efi;

use patina_pi::fw_fs::{
    ffs::{self, file},
    fv::{self, BlockMapEntry},
    fvb,
};

use crate::{
    FirmwareFileSystemError,
    file::{File, FileRef},
    section::{self, Section, SectionComposer, SectionExtractor},
};

/// Zero-copy view over a Firmware Volume (FV) backed by a byte slice.
///
/// Parsing via [`VolumeRef::new`] validates the FV header, optional extended
/// header, block map, and computes the content start offset. Accessors expose
/// properties like attributes, FV name, block layout, and contained FFS files.
pub struct VolumeRef<'a> {
    data: &'a [u8],
    fv_header: fv::Header,
    ext_header: Option<fv::ExtHeader>,
    block_map: Vec<fv::BlockMapEntry>,
    content_offset: usize,
}

impl<'a> VolumeRef<'a> {
    /// Parse a byte slice as a Firmware Volume and validate its metadata.
    ///
    /// Validates signature, header length, checksum, revision, filesystem GUID,
    /// extended header bounds, and block map structure. On success, returns a
    /// zero-copy view tied to the provided buffer.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::{Volume, VolumeRef};
    /// use patina_pi::fw_fs::fv::BlockMapEntry;
    ///
    /// // Build a minimal FV in memory, then parse it back.
    /// let block_map = vec![BlockMapEntry { num_blocks: 1, length: 4096 }];
    /// let fv_bytes = Volume::new(block_map).serialize().unwrap();
    /// let fv_ref = VolumeRef::new(&fv_bytes).unwrap();
    /// assert!(fv_ref.size() >= 4096);
    /// ```
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
                let ext_header =
                    unsafe { ptr::read_unaligned(buffer[ext_header_offset..].as_ptr() as *const fv::ExtHeader) };
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

        // Files must be 8-byte aligned relative to the start of the FV (i.e. relative to start of &data), so align
        // content_offset to account for this.
        let content_offset =
            align_up(content_offset as u64, 8).map_err(|_| FirmwareFileSystemError::InvalidHeader)? as usize;

        Ok(Self { data: buffer, fv_header, ext_header, block_map, content_offset })
    }

    /// Instantiate a new FirmwareVolume from a base address.
    ///
    /// ## Safety
    ///
    /// Caller must ensure that base_address is the address of the start of a firmware volume.
    /// Caller must ensure that the lifetime of the buffer at base_address is longer than the
    /// returned VolumeRef.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::{Volume, VolumeRef};
    /// use patina_pi::fw_fs::fv::BlockMapEntry;
    ///
    /// let fv_bytes = Volume::new(vec![BlockMapEntry { num_blocks: 1, length: 4096 }])
    ///     .serialize()
    ///     .unwrap();
    /// let base = fv_bytes.as_ptr() as u64;
    /// let fv_ref = unsafe { VolumeRef::new_from_address(base) }.unwrap();
    /// assert!(fv_ref.size() >= 4096);
    /// ```
    pub unsafe fn new_from_address(base_address: u64) -> Result<Self, FirmwareFileSystemError> {
        let fv_header = unsafe { ptr::read_unaligned(base_address as *const fv::Header) };
        if fv_header.signature != u32::from_le_bytes(*b"_FVH") {
            // base_address is not the start of a firmware volume.
            return Err(FirmwareFileSystemError::DataCorrupt);
        }

        let fv_buffer = unsafe { slice::from_raw_parts(base_address as *const u8, fv_header.fv_length as usize) };
        Self::new(fv_buffer)
    }

    /// The erase/pad byte used by this FV according to its attributes.
    ///
    /// Returns 0xFF when erase polarity is 1, otherwise 0x00.
    pub fn erase_byte(&self) -> u8 {
        if self.fv_header.attributes & fvb::attributes::raw::fvb2::ERASE_POLARITY != 0 { 0xff } else { 0 }
    }

    /// The optional extended header and its vendor data, if present.
    ///
    /// Returns a tuple of the header struct and the associated data payload.
    pub fn ext_header(&self) -> Option<(fv::ExtHeader, Vec<u8>)> {
        self.ext_header.map(|ext_header| {
            let header_size = mem::size_of_val(&ext_header);
            let ext_header_data_start = self.fv_header.ext_header_offset as usize + header_size;
            let ext_header_end = ext_header_data_start + ext_header.ext_header_size as usize - header_size;
            let header_data = self.data[ext_header_data_start..ext_header_end].to_vec();
            (ext_header, header_data)
        })
    }

    /// The Firmware Volume name GUID from the extended header, if available.
    pub fn fv_name(&self) -> Option<efi::Guid> {
        self.ext_header().map(|(hdr, _)| hdr.fv_name)
    }

    /// The parsed block map describing block counts and sizes within the FV.
    pub fn block_map(&self) -> &Vec<BlockMapEntry> {
        &self.block_map
    }

    /// Resolve information about a Logical Block Address (LBA).
    ///
    /// Returns a tuple of (byte_offset_from_fv_start, block_size, remaining_blocks_in_region).
    /// Errors if `lba` is out of range per the block map.
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::{Volume, VolumeRef};
    /// use patina_pi::fw_fs::fv::BlockMapEntry;
    /// let fv_bytes = Volume::new(vec![BlockMapEntry { num_blocks: 1, length: 4096 }])
    ///     .serialize()
    ///     .unwrap();
    /// let fv_ref = VolumeRef::new(&fv_bytes).unwrap();
    /// let (offset, blk, rem) = fv_ref.lba_info(0).unwrap();
    /// assert_eq!((offset, blk, rem), (0, 4096, 1));
    /// ```
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

    /// The FV attributes bitfield (`EFI_FVB_ATTRIBUTES_2`).
    pub fn attributes(&self) -> fvb::attributes::EfiFvbAttributes2 {
        self.fv_header.attributes
    }

    /// Total FV size in bytes (`FvLength`).
    pub fn size(&self) -> u64 {
        self.fv_header.fv_length
    }

    /// Iterate over contained FFS files as zero-copy [`FileRef`]s.
    ///
    /// PAD files are filtered out per PI spec. Parsing errors are surfaced as iterator items.
    pub fn files(&self) -> impl Iterator<Item = Result<FileRef<'a>, FirmwareFileSystemError>> {
        FileRefIter::new(&self.data[self.content_offset..], self.erase_byte()).filter(|x| {
            //Per PI spec 1.8A, V3, section 2.1.4.1.8: "Standard firmware file system services will not return the
            //handle of any PAD files, nor will they permit explicit creation of such files."
            //Pad files are ignored on read, and will be inserted on serialization as needed to honor alignment
            //requirements. Filter them out here.
            !matches!(x, Ok(file) if file.file_type_raw() == ffs::file::raw::r#type::FFS_PAD)
        })
    }

    fn revision(&self) -> u8 {
        self.fv_header.revision
    }

    fn file_system_guid(&self) -> efi::Guid {
        self.fv_header.file_system_guid
    }
}

impl fmt::Debug for VolumeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VolumeRef")
            .field("data ({:#x} bytes)", &self.data.len())
            .field("fv_header", &self.fv_header)
            .field("ext_header", &self.ext_header)
            .field("block_map", &self.block_map)
            .field("content_offset", &self.content_offset)
            .finish()
    }
}

impl<'a> TryFrom<&'a [u8]> for VolumeRef<'a> {
    type Error = FirmwareFileSystemError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        VolumeRef::new(value)
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
                Ok(next_offset) => {
                    self.next_offset = next_offset as usize;
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

enum Capacity {
    Unbounded,
    Size(usize),
}

/// Owned, mutable representation of a Firmware Volume for composition and serialization.
///
/// Use this to build an FV from a block map and a list of FFS files, set attributes,
/// optionally attach an extended header, and then [`serialize`](Self::serialize) to bytes.
pub struct Volume {
    file_system_guid: efi::Guid,
    attributes: fvb::attributes::EfiFvbAttributes2,
    ext_header: Option<(fv::ExtHeader, Vec<u8>)>,
    block_map: Vec<BlockMapEntry>,
    files: Vec<File>,
    capacity: Capacity,
}

impl Volume {
    /// Create a new empty Firmware Volume builder with the given block map.
    ///
    /// Defaults to the FFSv3 filesystem GUID, no extended header, and unbounded capacity.
    pub fn new(block_map: Vec<BlockMapEntry>) -> Self {
        Self {
            file_system_guid: ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID,
            attributes: 0,
            ext_header: None,
            block_map,
            files: Vec::new(),
            capacity: Capacity::Unbounded,
        }
    }

    /// Read-only access to the list of FFS files contained in this FV.
    pub fn files(&self) -> impl Iterator<Item = &File> {
        self.files.iter()
    }

    /// Mutable access to the list of FFS files contained in this FV.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::Volume;
    /// use patina_ffs::file::File;
    /// use patina_pi::fw_fs::{ffs, fv::BlockMapEntry};
    /// use r_efi::efi;
    ///
    /// let mut fv = Volume::new(vec![BlockMapEntry { num_blocks: 1, length: 4096 }]);
    /// fv.files_mut().push(File::new(efi::Guid::from_bytes(&[0u8; 16]), ffs::file::raw::r#type::FFS_PAD));
    /// assert_eq!(fv.files().count(), 1);
    /// ```
    pub fn files_mut(&mut self) -> &mut Vec<File> {
        &mut self.files
    }

    /// Serialize the Firmware Volume into a valid FV byte stream.
    ///
    /// Produces a correct FV header (including checksum), inserts PAD files to
    /// satisfy file alignment and optional extended header placement, respects
    /// filesystem capabilities (FFSv2 vs FFSv3), and pads to capacity when set.
    /// Errors propagate from serializing files and sections or when constraints
    /// are violated (e.g., file too large for FFSv2).
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::Volume;
    /// use patina_ffs::file::File;
    /// use patina_ffs::section::{Section, SectionHeader};
    /// use patina_pi::fw_fs::{ffs, fv::BlockMapEntry};
    /// use r_efi::efi;
    ///
    /// // Create a volume and add several files, each with a RAW section.
    /// let mut fv = Volume::new(vec![BlockMapEntry { num_blocks: 4, length: 4096 }]);
    ///
    /// for (i, payload) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
    ///     let guid = efi::Guid::from_bytes(&[i as u8; 16]);
    ///     let mut file = File::new(guid, 0x07); // arbitrary file type for example
    ///
    ///     let data = payload.as_bytes().to_vec();
    ///     let section = Section::new_from_header_with_data(
    ///         SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///         data,
    ///     ).unwrap();
    ///     file.sections_mut().push(section);
    ///     fv.files_mut().push(file);
    /// }
    ///
    /// let bytes = fv.serialize().unwrap();
    /// assert!(!bytes.is_empty());
    /// ```
    pub fn serialize(&self) -> Result<Vec<u8>, FirmwareFileSystemError> {
        let pad_byte =
            if (self.attributes & fvb::attributes::raw::fvb2::ERASE_POLARITY) != 0 { 0xffu8 } else { 0x00u8 };

        let large_file_support = self.file_system_guid == ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID;

        let mut fv_header = fv::Header {
            zero_vector: [0u8; 16],
            file_system_guid: self.file_system_guid,
            fv_length: 0,
            signature: u32::from_le_bytes(*b"_FVH"),
            attributes: self.attributes,
            header_length: 0,
            checksum: 0,
            ext_header_offset: 0,
            reserved: 0,
            revision: fv::FFS_REVISION,
            block_map: [BlockMapEntry { num_blocks: 0, length: 0 }; 0],
        };

        //Patch the initial header into the output buffer
        let mut fv_buffer =
            unsafe { from_raw_parts(&raw mut fv_header as *mut u8, mem::size_of_val(&fv_header)).to_vec() };

        // add the block map
        for block in self.block_map.iter().chain(iter::once(&BlockMapEntry { num_blocks: 0, length: 0 })) {
            fv_buffer.extend_from_slice(unsafe {
                from_raw_parts(block as *const BlockMapEntry as *const u8, mem::size_of_val(block))
            });
        }

        let header_len = fv_buffer.len();

        // add the ext_header, if present
        let ext_header_offset = if let Some((ext_header, data)) = &self.ext_header {
            let offset = fv_buffer.len();
            let mut ext_hdr_data = unsafe {
                from_raw_parts(ext_header as *const fv::ExtHeader as *const u8, mem::size_of_val(ext_header)).to_vec()
            };
            ext_hdr_data.extend(data);

            // ext_header data is added as a "Pad" file
            let mut ext_header_pad_file =
                File::new(efi::Guid::from_bytes(&[0xffu8; 16]), ffs::file::raw::r#type::FFS_PAD);
            let ext_header_section = Section::new_from_header_with_data(
                section::SectionHeader::Pad(
                    ext_hdr_data.len().try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?,
                ),
                ext_hdr_data,
            )?;

            ext_header_pad_file.sections_mut().push(ext_header_section);
            ext_header_pad_file.set_data_checksum(false);

            fv_buffer.extend(ext_header_pad_file.serialize()?);

            offset + ext_header_pad_file.content_offset()?
        } else {
            0
        };

        // add padding to ensure first file is 8-byte aligned.
        let rem = fv_buffer.len() % 8;
        if rem != 0 {
            fv_buffer.extend(iter::repeat_n(pad_byte, 8 - rem));
        }

        //Serialize the file list into a content vector.
        for file in &self.files {
            let file_buffer = &file.serialize()?;

            // Check if the file is too big for the filesystem format.
            if file_buffer.len() >= fv::FFS_V2_MAX_FILE_SIZE && !large_file_support {
                Err(FirmwareFileSystemError::Unsupported)?;
            }

            let file_ref = FileRef::new(file_buffer)?;

            //check if a pad file needs to be inserted to align the file content.
            let required_content_alignment = file_ref.fv_attributes() & fv::file::raw::attribute::ALIGNMENT;
            let required_content_alignment: usize = 1 << required_content_alignment;

            if (fv_buffer.len() + file_ref.content_offset()) % required_content_alignment != 0 {
                //need to insert a pad file to ensure content is aligned to the required alignment specified in the
                //file attributes.

                //Per spec, max required_content_alignment is pad files is 16M (2^24). That means that pad file size
                //will always be less than 16M so we can always use Header (instead of Header2) for pad header.
                assert!(required_content_alignment < 0x1000000);

                let pad_len_base = fv_buffer.len() + mem::size_of::<ffs::file::Header>() + file_ref.content_offset();
                let rem = pad_len_base % required_content_alignment;
                let pad_len = if rem == 0 { 0 } else { required_content_alignment - rem };

                // check the padding math.
                debug_assert_eq!(
                    (fv_buffer.len() + mem::size_of::<ffs::file::Header>() + pad_len + file_ref.content_offset())
                        % required_content_alignment,
                    0
                );

                let mut pad_file = File::new(efi::Guid::from_bytes(&[0xffu8; 16]), ffs::file::raw::r#type::FFS_PAD);
                let pad_section = Section::new_from_header_with_data(
                    section::SectionHeader::Pad(
                        pad_len.try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?,
                    ),
                    iter::repeat_n(0xffu8, pad_len).collect(),
                )?;
                pad_file.sections_mut().push(pad_section);

                fv_buffer.extend(pad_file.serialize()?);
            }

            fv_buffer.extend_from_slice(file_buffer);

            //pad to next 8-byte aligned length, since files start at 8-byte aligned offsets.
            if fv_buffer.len() % 8 != 0 {
                let pad_length = 8 - (fv_buffer.len() % 8);

                fv_buffer.extend(iter::repeat_n(pad_byte, pad_length));
            }
        }

        if let Capacity::Size(size) = self.capacity
            && size > fv_buffer.len()
        {
            fv_buffer.extend(iter::repeat_n(pad_byte, size - fv_buffer.len()));
        }

        // calculate/patch the various header fields that need knowledge of buffer.
        fv_header.fv_length = fv_buffer.len().try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
        fv_header.header_length = header_len.try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
        fv_header.ext_header_offset =
            ext_header_offset.try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;

        // calculate the checksum.
        let checksum = fv_buffer[..header_len]
            .chunks_exact(2)
            //in the fv_buffer the following 3 fields are still set to zero, so manually add them to the checksum calculation.
            .chain(fv_header.fv_length.to_le_bytes().chunks_exact(2))
            .chain(fv_header.header_length.to_le_bytes().chunks_exact(2))
            .chain(fv_header.ext_header_offset.to_le_bytes().chunks_exact(2))
            .fold(0u16, |sum, value| sum.wrapping_add(u16::from_le_bytes(value.try_into().unwrap())));
        fv_header.checksum = 0u16.wrapping_sub(checksum);

        //re-write the updated fv_header into the front of the fv_buffer.
        fv_buffer[..mem::size_of_val(&fv_header)]
            .copy_from_slice(unsafe { from_raw_parts(&raw mut fv_header as *mut u8, mem::size_of_val(&fv_header)) });

        // verify the checksum
        debug_assert_eq!(
            fv_buffer[..header_len]
                .chunks_exact(2)
                .fold(0u16, |sum, value| sum.wrapping_add(u16::from_le_bytes(value.try_into().unwrap()))),
            0
        );

        Ok(fv_buffer)
    }

    /// Compose all sections for all files in the volume using the provided composer.
    ///
    /// Useful after editing encapsulated sections so that serialization has the
    /// correct composed bytes.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::volume::Volume;
    /// use patina_ffs::section::{Section, SectionComposer, SectionHeader};
    /// use patina_pi::fw_fs::{ffs, fv::BlockMapEntry};
    /// use r_efi::efi;
    ///
    /// struct Passthrough;
    /// impl SectionComposer for Passthrough {
    ///     fn compose(&self, section: &Section) -> Result<(SectionHeader, Vec<u8>), patina_ffs::FirmwareFileSystemError> {
    ///         Ok((section.header().clone(), section.try_content_as_slice()?.to_vec()))
    ///     }
    /// }
    ///
    /// let mut fv = Volume::new(vec![BlockMapEntry { num_blocks: 1, length: 4096 }]);
    /// // Add an empty PAD file to keep it simple
    /// fv.files_mut().push(patina_ffs::file::File::new(efi::Guid::from_bytes(&[0u8; 16]), ffs::file::raw::r#type::FFS_PAD));
    /// fv.compose(&Passthrough).unwrap();
    /// ```
    pub fn compose(&mut self, composer: &dyn SectionComposer) -> Result<(), FirmwareFileSystemError> {
        for file in self.files_mut() {
            file.compose(composer)?;
        }
        Ok(())
    }
}

impl TryFrom<&VolumeRef<'_>> for Volume {
    type Error = FirmwareFileSystemError;

    fn try_from(src: &VolumeRef<'_>) -> Result<Self, Self::Error> {
        if src.revision() > fv::FFS_REVISION {
            Err(FirmwareFileSystemError::Unsupported)?;
        }
        let files = src
            .files()
            .map(|x| match x {
                Ok(file_ref) => file_ref.try_into(),
                Err(err) => Err(err),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            file_system_guid: src.file_system_guid(),
            attributes: src.attributes(),
            ext_header: src.ext_header(),
            block_map: src.block_map().clone(),
            files,
            capacity: Capacity::Size(src.size() as usize),
        })
    }
}

impl TryFrom<(&VolumeRef<'_>, &dyn SectionExtractor)> for Volume {
    type Error = FirmwareFileSystemError;

    fn try_from(src: (&VolumeRef<'_>, &dyn SectionExtractor)) -> Result<Self, Self::Error> {
        let (src, extractor) = src;
        if src.revision() > fv::FFS_REVISION {
            Err(FirmwareFileSystemError::Unsupported)?;
        }
        let files = src
            .files()
            .map(|x| match x {
                Ok(file_ref) => (file_ref, extractor).try_into(),
                Err(err) => Err(err),
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            file_system_guid: src.file_system_guid(),
            attributes: src.attributes(),
            ext_header: src.ext_header(),
            block_map: src.block_map().clone(),
            files,
            capacity: Capacity::Size(src.size() as usize),
        })
    }
}

#[cfg(test)]
mod test {
    use core::{mem, sync::atomic::AtomicBool};
    use log::{self, Level, LevelFilter, Metadata, Record};
    use lzma_rs::lzma_decompress;
    use patina_pi::fw_fs::{self, ffs, fv};
    use r_efi::efi;
    use serde::Deserialize;
    use std::{
        collections::HashMap,
        env,
        error::Error,
        fs::{self, File},
        io::Cursor,
        path::Path,
    };
    use uuid::Uuid;

    use crate::{
        FirmwareFileSystemError,
        section::{Section, SectionComposer, SectionExtractor, SectionHeader},
        volume::{Volume, VolumeRef},
    };

    #[derive(Debug, Deserialize, Clone)]
    struct TargetValues {
        total_number_of_files: u32,
        files_to_test: HashMap<String, FfsFileTargetValues>,
    }

    #[derive(Debug, Deserialize, Clone)]
    struct FfsFileTargetValues {
        file_type: u8,
        attributes: u8,
        size: usize,
        number_of_sections: usize,
        sections: HashMap<usize, FfsSectionTargetValues>,
    }

    #[derive(Debug, Deserialize, Clone)]
    struct FfsSectionTargetValues {
        section_type: Option<ffs::section::EfiSectionType>,
        size: usize,
        text: Option<String>,
    }

    struct NullExtractor {}
    impl SectionExtractor for NullExtractor {
        fn extract(&self, _: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
            Err(FirmwareFileSystemError::Unsupported)
        }
    }

    // Sample logger for log crate to dump stuff in tests
    struct SimpleLogger;
    impl log::Log for SimpleLogger {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= Level::Info
        }

        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                println!("{}", record.args());
            }
        }

        fn flush(&self) {}
    }
    static LOGGER: SimpleLogger = SimpleLogger;

    fn set_logger() {
        let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));
    }

    fn stringify(error: FirmwareFileSystemError) -> String {
        format!("efi error: {:x?}", error).to_string()
    }

    fn extract_text_from_section(section: &Section) -> Option<String> {
        if section.section_type() == Some(ffs::section::Type::UserInterface) {
            let display_name_chars: Vec<u16> = section
                .try_content_as_slice()
                .unwrap()
                .chunks(2)
                .map(|x| u16::from_le_bytes(x.try_into().unwrap()))
                .collect();
            Some(String::from_utf16_lossy(&display_name_chars).trim_end_matches(char::from(0)).to_string())
        } else {
            None
        }
    }

    fn test_firmware_volume_ref_worker(
        fv: &VolumeRef,
        mut expected_values: TargetValues,
        extractor: &dyn SectionExtractor,
    ) -> Result<(), Box<dyn Error>> {
        let mut count = 0;
        for ffs_file in fv.files() {
            let ffs_file = ffs_file.map_err(stringify)?;
            count += 1;
            let file_name = Uuid::from_bytes_le(*ffs_file.name().as_bytes()).to_string().to_uppercase();
            if let Some(mut target) = expected_values.files_to_test.remove(&file_name) {
                assert_eq!(target.file_type, ffs_file.file_type_raw(), "[{file_name}] Error with the file type.");
                assert_eq!(
                    target.attributes,
                    ffs_file.attributes_raw(),
                    "[{file_name}] Error with the file attributes."
                );
                assert_eq!(target.size, ffs_file.size(), "[{file_name}] Error with the file size (Full size).");
                let sections = ffs_file.sections_with_extractor(extractor).map_err(stringify)?;
                for section in sections.iter().enumerate() {
                    println!("{:x?}", section);
                }
                assert_eq!(
                    target.number_of_sections,
                    sections.len(),
                    "[{file_name}] Error with the number of section in the File"
                );

                for (idx, section) in sections.iter().enumerate() {
                    if let Some(target) = target.sections.remove(&idx) {
                        assert_eq!(
                            target.section_type,
                            section.section_type().map(|x| x as u8),
                            "[{file_name}, section: {idx}] Error with the section Type"
                        );
                        assert_eq!(
                            target.size,
                            section.try_content_as_slice().unwrap().len(),
                            "[{file_name}, section: {idx}] Error with the section Size"
                        );
                        assert_eq!(
                            target.text,
                            extract_text_from_section(section),
                            "[{file_name}, section: {idx}] Error with the section Text"
                        );
                    }
                }

                assert!(target.sections.is_empty(), "Some section use case has not been run.");
            }
        }
        assert_eq!(
            expected_values.total_number_of_files, count,
            "The number of file found does not match the expected one."
        );
        assert!(expected_values.files_to_test.is_empty(), "Some file use case has not been run.");
        Ok(())
    }

    fn test_firmware_volume_worker(fv: &Volume, mut expected_values: TargetValues) -> Result<(), Box<dyn Error>> {
        let mut count = 0;
        for ffs_file in fv.files() {
            count += 1;
            let file_name = Uuid::from_bytes_le(*ffs_file.name().as_bytes()).to_string().to_uppercase();
            if let Some(mut target) = expected_values.files_to_test.remove(&file_name) {
                assert_eq!(target.file_type, ffs_file.file_type_raw(), "[{file_name}] Error with the file type.");
                assert_eq!(
                    target.attributes,
                    ffs_file.attributes_raw(),
                    "[{file_name}] Error with the file attributes."
                );
                let sections: Vec<&Section> = ffs_file.section_iter().collect();
                for section in sections.iter().enumerate() {
                    println!("{:x?}", section);
                }
                assert_eq!(
                    target.number_of_sections,
                    sections.len(),
                    "[{file_name}] Error with the number of section in the File"
                );

                for (idx, section) in sections.iter().enumerate() {
                    if let Some(target) = target.sections.remove(&idx) {
                        assert_eq!(
                            target.section_type,
                            section.section_type().map(|x| x as u8),
                            "[{file_name}, section: {idx}] Error with the section Type"
                        );
                        assert_eq!(
                            target.size,
                            section.try_content_as_slice().unwrap().len(),
                            "[{file_name}, section: {idx}] Error with the section Size"
                        );
                        assert_eq!(
                            target.text,
                            extract_text_from_section(section),
                            "[{file_name}, section: {idx}] Error with the section Text"
                        );
                    }
                }

                assert!(target.sections.is_empty(), "Some section use case has not been run.");
            }
        }
        assert_eq!(
            expected_values.total_number_of_files, count,
            "The number of file found does not match the expected one."
        );
        assert!(expected_values.files_to_test.is_empty(), "Some file use case has not been run.");
        Ok(())
    }

    #[test]
    fn test_firmware_volume() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv = VolumeRef::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("DXEFV_expected_values.yml"))?)?;

        test_firmware_volume_ref_worker(&fv, expected_values, &NullExtractor {})
    }

    #[test]
    fn test_giant_firmware_volume() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
        let fv = VolumeRef::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("GIGANTOR_expected_values.yml"))?)?;

        test_firmware_volume_ref_worker(&fv, expected_values, &NullExtractor {})
    }

    #[test]
    fn test_section_extraction() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("FVMAIN_COMPACT.Fv"))?;

        let expected_values = serde_yaml::from_reader::<File, TargetValues>(File::open(
            root.join("FVMAIN_COMPACT_expected_values.yml"),
        )?)?;

        struct TestExtractor {
            invoked: AtomicBool,
        }

        impl SectionExtractor for TestExtractor {
            fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
                let SectionHeader::GuidDefined(metadata, _, _) = section.header() else {
                    panic!("Unexpected section metadata");
                };
                assert_eq!(metadata.section_definition_guid, fw_fs::guid::BROTLI_SECTION);
                self.invoked.store(true, core::sync::atomic::Ordering::SeqCst);
                Err(FirmwareFileSystemError::Unsupported)
            }
        }

        let test_extractor = TestExtractor { invoked: AtomicBool::new(false) };

        let fv = VolumeRef::new(&fv_bytes).unwrap();

        test_firmware_volume_ref_worker(&fv, expected_values, &test_extractor)?;

        assert!(test_extractor.invoked.load(core::sync::atomic::Ordering::SeqCst));

        Ok(())
    }

    #[test]
    fn test_malformed_firmware_volume() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        // bogus signature.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).signature ^= 0xdeadbeef;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus header_length.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).header_length = 0;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus checksum.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).checksum ^= 0xbeef;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus revision.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).revision = 1;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus filesystem guid.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).file_system_guid = efi::Guid::from_bytes(&[0xa5; 16]);
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus fv length.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).fv_length = 0;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        // bogus ext header offset.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).fv_length = ((*fv_header).ext_header_offset - 1) as u64;
        };
        assert_eq!(VolumeRef::new(&fv_bytes).unwrap_err(), FirmwareFileSystemError::InvalidHeader);

        Ok(())
    }

    #[test]
    fn zero_size_block_map_gives_same_offset_as_no_block_map() {
        set_logger();
        //code in FirmwareVolume::new() assumes that the size of a struct that ends in a zero-size array is the same
        //as an identical struct that doesn't have the array at all. This unit test validates that assumption.
        #[repr(C)]
        struct A {
            foo: usize,
            bar: u32,
            baz: u32,
            block_map: [fv::BlockMapEntry; 0],
        }

        #[repr(C)]
        struct B {
            foo: usize,
            bar: u32,
            baz: u32,
        }
        assert_eq!(mem::size_of::<A>(), mem::size_of::<B>());

        let a = A { foo: 0, bar: 0, baz: 0, block_map: [fv::BlockMapEntry { length: 0, num_blocks: 0 }; 0] };

        let a_ptr = &a as *const A;

        unsafe {
            assert_eq!(((*a_ptr).block_map).as_ptr(), a_ptr.offset(1) as *const fv::BlockMapEntry);
        }
    }

    struct ExampleSectionExtractor {}
    impl SectionExtractor for ExampleSectionExtractor {
        fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
            println!("Encapsulated section: {:?}", section);
            Ok(Vec::new()) //A real section extractor would provide the extracted buffer on return.
        }
    }

    #[test]
    fn section_extract_should_extract() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
        let fv_bytes: Vec<u8> = fs::read(root.join("GIGANTOR.Fv"))?;
        let fv = VolumeRef::new(&fv_bytes).expect("Firmware Volume Corrupt");
        for file in fv.files() {
            let file = file.map_err(|_| "parse error".to_string())?;
            let sections = file.sections_with_extractor(&ExampleSectionExtractor {}).map_err(stringify)?;
            for (idx, section) in sections.iter().enumerate() {
                println!("file: {:?}, section: {:?} type: {:?}", file.name(), idx, section.section_type());
            }
        }
        Ok(())
    }

    #[test]
    fn section_should_have_correct_metadata() -> Result<(), Box<dyn Error>> {
        set_logger();
        let empty_pe32: [u8; 4] = [0x04, 0x00, 0x00, 0x10];
        let section = Section::new_from_buffer(&empty_pe32).unwrap();
        assert!(matches!(section.header(), SectionHeader::Standard(ffs::section::raw_type::PE32, _)));

        let empty_compression: [u8; 0x11] =
            [0x11, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let section = Section::new_from_buffer(&empty_compression).unwrap();
        match section.header() {
            SectionHeader::Compression(header, _) => {
                let length = header.uncompressed_length;
                assert_eq!(length, 0);
                assert_eq!(header.compression_type, 1);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_guid_defined: [u8; 32] = [
            0x20, 0x00, 0x00, 0x02, //Header
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, //GUID
            0x1C, 0x00, //Data offset
            0x12, 0x34, //Attributes
            0x00, 0x01, 0x02, 0x03, //GUID-specific fields
            0x04, 0x15, 0x19, 0x80, //Data
        ];
        let section = Section::new_from_buffer(&empty_guid_defined).unwrap();
        match section.header() {
            SectionHeader::GuidDefined(header, guid_data, _) => {
                assert_eq!(
                    header.section_definition_guid,
                    efi::Guid::from_bytes(&[
                        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF
                    ])
                );
                assert_eq!(header.data_offset, 0x1C);
                assert_eq!(header.attributes, 0x3412);
                assert_eq!(guid_data.to_vec(), &[0x00u8, 0x01, 0x02, 0x03]);
                assert_eq!(section.try_content_as_slice().unwrap(), &[0x04, 0x15, 0x19, 0x80]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_version: [u8; 14] =
            [0x0E, 0x00, 0x00, 0x14, 0x00, 0x00, 0x31, 0x00, 0x2E, 0x00, 0x30, 0x00, 0x00, 0x00];
        let section = Section::new_from_buffer(&empty_version).unwrap();
        match section.header() {
            SectionHeader::Version(version, _) => {
                assert_eq!(version.build_number, 0);
                assert_eq!(section.try_content_as_slice().unwrap(), &[0x31, 0x00, 0x2E, 0x00, 0x30, 0x00, 0x00, 0x00]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_freeform_subtype: [u8; 24] = [
            0x18, 0x00, 0x00, 0x18, //Header
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, //GUID
            0x04, 0x15, 0x19, 0x80, //Data
        ];
        let section = Section::new_from_buffer(&empty_freeform_subtype).unwrap();
        match section.header() {
            SectionHeader::FreeFormSubtypeGuid(ffst_header, _) => {
                assert_eq!(
                    ffst_header.sub_type_guid,
                    efi::Guid::from_bytes(&[
                        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF
                    ])
                );
                assert_eq!(section.try_content_as_slice().unwrap(), &[0x04, 0x15, 0x19, 0x80]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        Ok(())
    }

    #[test]
    fn test_firmware_volume_serialization() -> Result<(), Box<dyn Error>> {
        set_logger();
        let paths = &[
            Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources/DXEFV.Fv"),
            Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources/GIGANTOR.Fv"),
            Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources/FVMAIN_COMPACT.Fv"),
        ];

        for path in paths {
            let original_fv_bytes = fs::read(path)?;
            let fv_ref = VolumeRef::new(&original_fv_bytes).map_err(stringify)?;

            // convert the ref to a volume.
            let fv: Volume = (&fv_ref).try_into().map_err(stringify)?;

            // serialize the volume back to bytes
            let serialized_fv_bytes = fv.serialize().map_err(stringify)?;

            // the two buffers should match.
            assert_eq!(original_fv_bytes.len(), serialized_fv_bytes.len());

            let mismatch = &original_fv_bytes.iter().zip(&serialized_fv_bytes).enumerate().find_map(
                |(offset, (expected, actual))| {
                    if *expected != *actual { Some((offset, (*expected, *actual))) } else { None }
                },
            );

            if let Some((offset, (expected, actual))) = mismatch {
                panic!("mismatch in serialized buffer at offset {offset:x}. Expected {expected:x}, actual: {actual:x}");
            }
        }
        Ok(())
    }

    #[test]
    fn test_serialization_with_extractor_composer() -> Result<(), Box<dyn Error>> {
        set_logger();

        struct LzmaExtractorComposer {}
        impl SectionExtractor for LzmaExtractorComposer {
            fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
                if let SectionHeader::GuidDefined(guid_header, _, _) = section.header()
                    && guid_header.section_definition_guid == fw_fs::guid::LZMA_SECTION
                {
                    let data = section.try_content_as_slice()?;
                    let mut decompressed: Vec<u8> = Vec::new();
                    lzma_decompress(&mut Cursor::new(data), &mut decompressed)
                        .map_err(|_| FirmwareFileSystemError::DataCorrupt)?;
                    return Ok(decompressed);
                }
                Err(FirmwareFileSystemError::Unsupported)
            }
        }
        impl SectionComposer for LzmaExtractorComposer {
            fn compose(&self, _section: &Section) -> Result<(SectionHeader, Vec<u8>), FirmwareFileSystemError> {
                unreachable!()
            }
        }

        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
        let original_fv_bytes: Vec<u8> = fs::read(root.join("LZMATEST.Fv"))?;
        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("LZMATEST_expected_values.yml"))?)?;

        let fv_ref = VolumeRef::new(&original_fv_bytes).map_err(stringify)?;
        let fv: Volume = (&fv_ref, &LzmaExtractorComposer {} as &dyn SectionExtractor).try_into().map_err(stringify)?;
        //Verify expected results for both ref and owned versions of FV.
        test_firmware_volume_ref_worker(
            &fv_ref,
            expected_values.clone(),
            &LzmaExtractorComposer {} as &dyn SectionExtractor,
        )?;
        test_firmware_volume_worker(&fv, expected_values)?;

        //re-serializing the FV without modifying it should work and generate the same byte stream. This doesn't require
        //a composer since the sections haven't been modified, so they are already composed (i.e. the backing section
        //data for the LZMA section already exists and is correct)
        let serialized_fv_bytes = fv.serialize().map_err(stringify)?;

        // the two buffers should match.
        assert_eq!(original_fv_bytes.len(), serialized_fv_bytes.len());

        let mismatch = original_fv_bytes
            .iter()
            .zip(serialized_fv_bytes)
            .enumerate()
            .find(|(_offset, (expected, actual))| *expected != actual);

        if let Some((offset, (expected, actual))) = mismatch {
            panic!("mismatch in serialized buffer at offset {offset:x}. Expected {expected:x}, actual: {actual:x}");
        }
        Ok(())
    }

    #[test]
    fn test_extractor_composer_round_trip() -> Result<(), Box<dyn Error>> {
        struct LzmaExtractorComposer {}

        impl SectionExtractor for LzmaExtractorComposer {
            fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
                if let SectionHeader::GuidDefined(guid_header, _, _) = section.header()
                    && guid_header.section_definition_guid == fw_fs::guid::LZMA_SECTION
                {
                    let data = section.try_content_as_slice()?;
                    let mut decompressed: Vec<u8> = Vec::new();
                    lzma_decompress(&mut Cursor::new(data), &mut decompressed)
                        .map_err(|_| FirmwareFileSystemError::DataCorrupt)?;
                    return Ok(decompressed);
                }
                Err(FirmwareFileSystemError::Unsupported)
            }
        }

        impl SectionComposer for LzmaExtractorComposer {
            fn compose(&self, section: &Section) -> Result<(SectionHeader, Vec<u8>), FirmwareFileSystemError> {
                if let SectionHeader::GuidDefined(guid_header, _, _) = section.header()
                    && guid_header.section_definition_guid == fw_fs::guid::LZMA_SECTION
                {
                    let mut content = Vec::new();
                    let mut section_iter = section.sub_sections().peekable();
                    while let Some(section) = &section_iter.next() {
                        content.extend(section.serialize()?);
                        if section_iter.peek().is_some() {
                            //pad to next 4-byte aligned length, since sections start at 4-byte aligned offsets. No padding is added
                            //after the last section.
                            if content.len() % 4 != 0 {
                                let pad_length = 4 - (content.len() % 4);
                                //Per PI 1.8A volume 3 section 2.2.4, pad byte is always zero.
                                content.extend(core::iter::repeat_n(0u8, pad_length));
                            }
                        }
                    }
                    let mut compressed: Vec<u8> = Vec::new();
                    let options = lzma_rs::compress::Options {
                        unpacked_size: lzma_rs::compress::UnpackedSize::WriteToHeader(Some(content.len() as u64)),
                    };
                    lzma_rs::lzma_compress_with_options(&mut Cursor::new(content), &mut compressed, &options)
                        .map_err(|_| FirmwareFileSystemError::ComposeFailed)?;

                    let mut header = section.header().clone();
                    header.set_content_size(compressed.len()).map_err(|_| FirmwareFileSystemError::InvalidHeader)?;

                    return Ok((header, compressed));
                }

                Err(FirmwareFileSystemError::Unsupported)
            }
        }

        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
        let original_fv_bytes: Vec<u8> = fs::read(root.join("LZMATEST.Fv"))?;

        let fv_ref = VolumeRef::new(&original_fv_bytes).map_err(stringify)?;
        let mut fv: Volume =
            (&fv_ref, &LzmaExtractorComposer {} as &dyn SectionExtractor).try_into().map_err(stringify)?;

        // modify the compressed logo section of the logo file.
        let logo_file = fv.files_mut().get_mut(0).unwrap();
        let lzma_section = &mut logo_file.sections_mut()[0];
        match lzma_section.header() {
            SectionHeader::GuidDefined(header, _, _) => {
                assert_eq!(header.section_definition_guid, fw_fs::guid::LZMA_SECTION);
            }
            _ => panic!("Expected LZMA section header"),
        }

        let logo_sub_section = lzma_section.sub_sections_mut().next().unwrap();
        assert_eq!(logo_sub_section.section_type(), Some(ffs::section::Type::Raw));

        let orig_logo_data = logo_sub_section.try_content_as_slice().map_err(stringify)?.to_vec();
        logo_sub_section.set_section_data(vec![0x04u8, 0x15, 0x19, 0x80]).map_err(stringify)?;

        //serialize the modified FV; it should fail since it hasn't been composed.
        assert_eq!(fv.serialize(), Err(FirmwareFileSystemError::NotComposed));

        //compose the FV.
        fv.compose(&LzmaExtractorComposer {}).map_err(stringify)?;

        //re-serialize the FV with the modified logo file. This should succeed.
        let serialized_fv_bytes = fv.serialize().map_err(stringify)?;

        //Create a new fv_ref/volume from the serialized bytes.
        let serialized_fv_ref = VolumeRef::new(&serialized_fv_bytes).map_err(stringify)?;
        let mut serialized_fv: Volume =
            (&serialized_fv_ref, &LzmaExtractorComposer {} as &dyn SectionExtractor).try_into().map_err(stringify)?;

        // read the compressed logo section of the logo file and confirm it matches the test value.
        let logo_file = serialized_fv.files_mut().get_mut(0).unwrap();
        let lzma_section = &mut logo_file.sections_mut()[0];
        match lzma_section.header() {
            SectionHeader::GuidDefined(header, _, _) => {
                assert_eq!(header.section_definition_guid, fw_fs::guid::LZMA_SECTION);
            }
            _ => panic!("Expected LZMA section header"),
        }

        let logo_sub_section = lzma_section.sub_sections_mut().next().unwrap();
        assert_eq!(logo_sub_section.section_type(), Some(ffs::section::Type::Raw));
        assert_eq!(logo_sub_section.try_content_as_slice().map_err(stringify)?, &[0x04u8, 0x15, 0x19, 0x80]);

        // now put back the original logo.
        logo_sub_section.set_section_data(orig_logo_data).map_err(stringify)?;

        //compose the FV.
        serialized_fv.compose(&LzmaExtractorComposer {}).map_err(stringify)?;

        //re-serialize the FV with the original logo file.
        let serialized_fv_bytes = serialized_fv.serialize().map_err(stringify)?;

        //unfortunately, the lzma-rs encoder isn't robust enough to encode with the expected lzma parameters,
        //otherwise we could just just compare the original fv bytes to the serialized fv bytes directly.
        //instead, we'll compare the contents.
        let serialized_fv_ref = VolumeRef::new(&serialized_fv_bytes).map_err(stringify)?;

        let org_files = fv_ref.files().collect::<Vec<_>>();
        let round_trip_files = serialized_fv_ref.files().collect::<Vec<_>>();

        assert_eq!(org_files.len(), round_trip_files.len());

        for (org_file, round_trip_file) in Iterator::zip(org_files.into_iter(), round_trip_files.into_iter()) {
            let org_file = org_file.map_err(stringify)?;
            let round_trip_file = round_trip_file.map_err(stringify)?;
            assert_eq!(org_file.name(), round_trip_file.name());
            assert_eq!(org_file.file_type_raw(), round_trip_file.file_type_raw());
            assert_eq!(org_file.attributes_raw(), round_trip_file.attributes_raw());
            //assert_eq!(org_file.size(), round_trip_file.size()); //file sizes are different due to difference in LZMA compression.

            let org_sections = org_file.sections().map_err(stringify)?;
            let round_trip_sections = round_trip_file.sections().map_err(stringify)?;
            assert_eq!(org_sections.len(), round_trip_sections.len());

            for (org_section, round_trip_section) in Iterator::zip(org_sections.iter(), round_trip_sections.iter()) {
                assert_eq!(org_section.section_type(), round_trip_section.section_type());
                if org_section.section_type() == Some(ffs::section::Type::GuidDefined) {
                    // the GUID-defined section content is LZMA compressed, but lzma-rs encoder doesn't support the UEFI
                    // parameter set, so the content won't match because the compression parameters are different.
                    // however, the sub-sections will be produced as part of the section iterator, so those will be compared.
                    continue;
                }
                assert_eq!(
                    org_section.try_content_as_slice().map_err(stringify)?,
                    round_trip_section.try_content_as_slice().map_err(stringify)?
                );
            }
            assert_eq!(org_sections.len(), round_trip_sections.len());
        }

        Ok(())
    }
}
