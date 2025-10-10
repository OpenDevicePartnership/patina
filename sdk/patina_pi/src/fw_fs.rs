//! Firmware Filesystem
//!
//! Exports services used to interact with the Firmware Filesystem (FFS).
//!
//! See <https://uefi.org/specs/PI/1.8A/V3_Design_Discussion.html#firmware-storage-introduction>.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

extern crate alloc;

use core::{fmt, mem, num::Wrapping, slice};

pub mod ffs;
pub mod fv;
pub mod fvb;

use ffs::{attributes::raw::LARGE_FILE, file, section};
pub use ffs::{
    attributes::{Attribute as FfsAttribute, raw as FfsRawAttribute},
    file::{
        State as FfsFileState, Type as FfsFileType,
        raw::{state as FfsFileRawState, r#type as FfsFileRawType},
    },
    section::{
        EfiSectionType, Type as FfsSectionType, header as FfsSectionHeader, raw_type as FfsSectionRawType,
        raw_type::encapsulated as FfsEncapsulatedSectionRawType,
    },
};
pub use fv::{
    EfiFvFileType, WritePolicy,
    attributes::{EfiFvAttributes, Fv2 as Fv2Attributes, raw::fv2 as Fv2RawAttributes},
    file::{Attribute as FvFileAttribute, EfiFvFileAttributes, raw::attribute as FvFileRawAttribute},
};
pub use fvb::attributes::{EfiFvbAttributes2, Fvb2 as Fvb2Attributes, raw::fvb2 as Fvb2RawAttributes};

use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use num_traits::WrappingSub;
use r_efi::efi;

use crate::address_helper::align_up;

pub mod guid {
    use r_efi::efi;

    pub const BROTLI_SECTION: efi::Guid =
        efi::Guid::from_fields(0x3D532050, 0x5CDA, 0x4FD0, 0x87, 0x9E, &[0x0F, 0x7F, 0x63, 0x0D, 0x5A, 0xFB]);
    pub const CRC32_SECTION: efi::Guid =
        efi::Guid::from_fields(0xFC1BCDB0, 0x7D31, 0x49aa, 0x93, 0x6A, &[0xA4, 0x60, 0x0D, 0x9D, 0xD0, 0x83]);
    pub const LZMA_SECTION: efi::Guid =
        efi::Guid::from_fields(0xEE4E5898, 0x3914, 0x4259, 0x9D, 0x6E, &[0xDC, 0x7B, 0xD7, 0x94, 0x03, 0xCF]);
    pub const LZMA_F86_SECTION: efi::Guid =
        efi::Guid::from_fields(0xD42AE6BD, 0x1352, 0x4BFB, 0x90, 0x9A, &[0xCA, 0x72, 0xA6, 0xEA, 0xE8, 0x89]);
    pub const LZMA_PARALLEL_SECTION: efi::Guid =
        efi::Guid::from_fields(0xBD9921EA, 0xED91, 0x404A, 0x8B, 0x2F, &[0xB4, 0xD7, 0x24, 0x74, 0x7C, 0x8C]);
    pub const TIANO_DECOMPRESS_SECTION: efi::Guid =
        efi::Guid::from_fields(0xA31280AD, 0x481E, 0x41B6, 0x95, 0xE8, &[0x12, 0x7F, 0x4C, 0x98, 0x47, 0x79]);
}

/// Defines an interface that can be implemented to provide extraction logic for encapsulation sections.
///
/// ## Example
///```
/// # use std::{env, fs, path::Path, error::Error};
/// use patina_pi::fw_fs::{FirmwareVolume, Section, SectionExtractor};
/// use r_efi::efi;
///
/// struct ExampleSectionExtractor {}
/// impl SectionExtractor for ExampleSectionExtractor {
///   fn extract(&self, section: &Section) -> Result<Box<[u8]>, efi::Status> {
///     println!("Encapsulated section: {:?}", section);
///     Ok(Box::new([0u8; 0])) //A real section extractor would provide the extracted buffer on return.
///   }
/// }
///
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
/// # let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
/// let mut fv = FirmwareVolume::new(&fv_bytes).expect("Firmware Volume Corrupt");
/// for file in fv.file_iter() {
///   let file = file.map_err(|_|"parse error".to_string())?;
///   for (idx, section) in file.section_iter_with_extractor(&ExampleSectionExtractor {}).enumerate() {
///     let section = section.map_err(|_|"parse error".to_string())?;
///     println!("file: {:?}, section: {:?} type: {:?}", file.name(), idx, section.section_type());
///   }
/// }
/// # Ok(())
/// # }
///```
pub trait SectionExtractor {
    /// Extracts the given section and returns the resulting buffer.
    ///
    /// This will be called for every encapsulation section when supplied as an argument to
    /// [`File::section_iter_with_extractor`].
    ///
    /// If section extraction is successful, then the resulting buffer is returned.
    ///
    /// If the section extraction implementation supports extracting the section, but there is an error extracting it,
    /// then an error should be returned.
    ///
    /// If the section extraction implementation does not support the encapsulations type used in this section, it can
    /// return a successful extraction with a zero-size buffer - this will allow parsing the rest of the FFS while only
    /// exposing the encapsulation section as a whole (without exposing sections it contains that cannot be extracted).
    fn extract(&self, section: &Section) -> Result<Box<[u8]>, efi::Status>;
}

// Null implementation of SectionExtractor used by [`FirmwareVolume::new`] and [`File::new`] when no extraction is
// desired.
struct NullSectionExtractor {}

impl SectionExtractor for NullSectionExtractor {
    fn extract(&self, _section: &Section) -> Result<Box<[u8]>, efi::Status> {
        Ok(Box::new([0u8; 0]))
    }
}

#[derive(Clone)]
pub struct FirmwareVolumeExtHeader<'a> {
    header: fv::ExtHeader,
    data: &'a [u8],
}

impl fmt::Debug for FirmwareVolumeExtHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirmwareVolumeExtHeader")
            .field("header", &self.header)
            .field("data.len()", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// Firmware Volume access support
///
/// Provides access to firmware volume contents.
///
/// ## Example
///```
/// # use std::{env, fs, path::Path, error::Error};
/// use patina_pi::fw_fs::FirmwareVolume;
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
/// # let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
/// let fv = FirmwareVolume::new(&fv_bytes).expect("Firmware Volume Corrupt");
/// println!("{:#x?}", fv);
/// # Ok(())
/// # }
///```
#[derive(Clone)]
pub struct FirmwareVolume<'a> {
    data: &'a [u8],
    attributes: EfiFvbAttributes2,
    block_map: Vec<fv::BlockMapEntry>,
    ext_header: Option<FirmwareVolumeExtHeader<'a>>,
    data_offset: usize,
    erase_byte: u8,
}

impl<'a> FirmwareVolume<'a> {
    /// Instantiate a new FirmwareVolume.
    ///
    /// Contents of the FirmwareVolume will be cached in this instance.
    pub fn new(buffer: &'a [u8]) -> Result<Self, efi::Status> {
        //buffer must be large enough to hold the header structure.
        if buffer.len() < mem::size_of::<fv::Header>() {
            Err(efi::Status::INVALID_PARAMETER)?;
        }

        //Safety: buffer is large enough to contain the header, so can cast to a ref.
        let fv_header = unsafe { &*(buffer.as_ptr() as *const fv::Header) };

        // signature: must be ASCII '_FVH'
        if fv_header.signature != u32::from_le_bytes(*b"_FVH") {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // header_length: must be large enough to hold the header.
        if (fv_header.header_length as usize) < mem::size_of::<fv::Header>() {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // header_length: buffer must be large enough to hold the header.
        if (fv_header.header_length as usize) > buffer.len() {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // checksum: fv header must sum to zero (and must be multiple of 2 bytes)
        if fv_header.header_length & 0x01 != 0 {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        let header_slice = &buffer[..fv_header.header_length as usize];
        let sum: Wrapping<u16> =
            header_slice.chunks_exact(2).map(|x| Wrapping(u16::from_le_bytes(x.try_into().unwrap()))).sum();

        if sum != Wrapping(0u16) {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // revision: must be at least 2. Assumes that if later specs bump the rev they will maintain
        // backwards compat with existing header definition.
        if fv_header.revision < 2 {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // file_system_guid: must be EFI_FIRMWARE_FILE_SYSTEM2_GUID or EFI_FIRMWARE_FILE_SYSTEM3_GUID.
        if fv_header.file_system_guid != ffs::guid::EFI_FIRMWARE_FILE_SYSTEM2_GUID
            && fv_header.file_system_guid != ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID
        {
            Err(efi::Status::INVALID_PARAMETER)?;
        }

        // fv_length: must be large enough to hold the header.
        if fv_header.fv_length < fv_header.header_length as u64 {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // fv_length: must be less than or equal to fv_data buffer length
        if fv_header.fv_length > buffer.len() as u64 {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        //ext_header_offset: must be inside the fv
        if fv_header.ext_header_offset as u64 > fv_header.fv_length {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        //if ext_header is present, its size must fit inside the FV.
        let ext_header = {
            if fv_header.ext_header_offset != 0 {
                let ext_header_offset = fv_header.ext_header_offset as usize;
                if ext_header_offset + mem::size_of::<fv::ExtHeader>() > buffer.len() {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }

                //Safety: previous check ensures that fv_data is large enough to contain the ext_header
                let ext_header = unsafe { &*(buffer[ext_header_offset..].as_ptr() as *const fv::ExtHeader) };
                let ext_header_end = ext_header_offset + ext_header.ext_header_size as usize;
                if ext_header_end > buffer.len() {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                Some(FirmwareVolumeExtHeader { header: *ext_header, data: &buffer[ext_header_offset..ext_header_end] })
            } else {
                None
            }
        };

        //block map must fit within the fv header (which is checked above to guarantee it is within the fv_data buffer).
        let block_map = &buffer[mem::size_of::<fv::Header>()..fv_header.header_length as usize];

        //block map should be a multiple of 8 in size
        if block_map.len() & 0x7 != 0 {
            Err(efi::Status::VOLUME_CORRUPTED)?;
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
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        //remove the terminator.
        block_map.pop();

        //thre must be at least one valid entry in the block map.
        if block_map.is_empty() {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        //other entries in block map must be non-zero.
        if block_map.iter().any(|x| x == &fv::BlockMapEntry { num_blocks: 0, length: 0 }) {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        let data_offset = {
            if let Some(ext_header) = &ext_header {
                // if ext header exists, then data starts after ext header
                fv_header.ext_header_offset as usize + ext_header.header.ext_header_size as usize
            } else {
                // otherwise data starts after the fv_header.
                fv_header.header_length as usize
            }
        };

        let data_offset = align_up(data_offset as u64, 8) as usize;
        let erase_byte = if fv_header.attributes & Fvb2RawAttributes::ERASE_POLARITY != 0 { 0xff } else { 0 };

        Ok(Self { data: buffer, attributes: fv_header.attributes, block_map, ext_header, data_offset, erase_byte })
    }

    /// Instantiate a new FirmwareVolume from a base address.
    ///
    /// ## Safety
    /// Caller must ensure that base_address is the address of the start of a firmware volume.
    ///
    /// Contents of the FirmwareVolume will be cached in this instance.
    pub unsafe fn new_from_address(base_address: u64) -> Result<Self, efi::Status> {
        // SAFETY: per function safety contract, base_address is the start of a firmware volume. The firmware volume
        // header signature is checked for a new firmware volume is returned.
        let fv_header = unsafe { &*(base_address as *const fv::Header) };
        if fv_header.signature != u32::from_le_bytes(*b"_FVH") {
            // base_address is not the start of a firmware volume.
            return Err(efi::Status::VOLUME_CORRUPTED);
        }

        // SAFETY: The slice creation is considered safe if the base address for the FV header has a valid
        // firmware volume header signature.
        let fv_buffer = unsafe { slice::from_raw_parts(base_address as *const u8, fv_header.fv_length as usize) };
        Self::new(fv_buffer)
    }

    /// Returns the block map for the FV
    pub fn block_map(&self) -> &Vec<fv::BlockMapEntry> {
        &self.block_map
    }

    /// Returns the GUID name of the FV, if any.
    pub fn fv_name(&self) -> Option<efi::Guid> {
        self.ext_header.as_ref().map(|ext_header| ext_header.header.fv_name)
    }

    /// Returns an iterator of the files in this FV.
    pub fn file_iter(&self) -> impl Iterator<Item = Result<File<'a>, efi::Status>> {
        FvFileIterator::new(&self.data[self.data_offset..], self.erase_byte)
    }

    /// returns the (linear block offset from FV base, block_size, remaining_blocks) given an LBA.
    pub fn lba_info(&self, lba: u32) -> Result<(u32, u32, u32), efi::Status> {
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
            return Err(efi::Status::INVALID_PARAMETER); //lba out of range.
        }

        let remaining_blocks = total_blocks - lba;
        Ok((offset + lba * block_size, block_size, remaining_blocks))
    }

    /// Returns the attributes for the FirmwareVolume
    pub fn attributes(&self) -> EfiFvbAttributes2 {
        self.attributes
    }

    /// Returns the size in bytes of the FV data + header.
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }

    /// Returns the FV data.
    pub fn data(&self) -> &[u8] {
        self.data
    }
}

impl fmt::Debug for FirmwareVolume<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FirmwareVolume")
            .field("attributes", &self.attributes)
            .field("block_map", &self.block_map)
            .field("ext_header", &self.ext_header)
            .field("data_offset", &self.data_offset)
            .field("erase_byte", &self.erase_byte)
            .field("data.len()", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// File access support
///
/// Provides access to file contents.
///
/// ## Example
///```
/// # use std::{env, fs, path::Path, error::Error};
/// use patina_pi::fw_fs::FirmwareVolume;
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
/// # let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
/// let fv = FirmwareVolume::new(&fv_bytes).expect("Firmware Volume Corrupt");
/// for file in fv.file_iter() {
///   println!("{:#x?}", file);
/// }
/// # Ok(())
/// # }
///```
#[derive(Clone)]
pub struct File<'a> {
    data: &'a [u8],
    name: efi::Guid,
    file_type: u8,
    attributes: u8,
    header_size: usize,
    size: u64,
}

impl<'a> File<'a> {
    /// Instantiates a new File by parsing the given buffer.
    ///
    /// The normal way to obtain a File instance would be through the [`FirmwareVolume::file_iter()`] method, but
    /// a constructor is provided here to enable independent instantiation of a file.
    pub fn new(buffer: &'a [u8]) -> Result<Self, efi::Status> {
        // verify that buffer has enough storage for a file header.
        if buffer.len() < mem::size_of::<file::Header>() {
            Err(efi::Status::INVALID_PARAMETER)?;
        }

        //Safety: buffer is large enough to contain the header, so can cast to a ref.
        let file_header = unsafe { &*(buffer.as_ptr() as *const file::Header) };

        // determine size and data offset
        let (header_size, size) = {
            let header_size = mem::size_of::<file::Header>();
            if (file_header.attributes & LARGE_FILE) == 0 {
                //standard header with 24-bit size
                let mut size_vec = file_header.size.to_vec();
                size_vec.push(0);
                let size = u32::from_le_bytes(size_vec.try_into().unwrap());
                (header_size, size as u64)
            } else {
                //extended header with 64-bit size
                let extended_size_length = mem::size_of::<u64>();
                if buffer[header_size..].len() < extended_size_length {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                let size =
                    u64::from_le_bytes(buffer[header_size..header_size + extended_size_length].try_into().unwrap());
                (header_size + extended_size_length, size as u64)
            }
        };

        // Verify that the total size of the file fits within the buffer.
        if size as usize > buffer.len() {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        // Interpreting the state field requires knowledge of the EFI_FVB_ERASE_POLARITY from the FV header, which is not
        // available here unless the constructor API is modified to specify it. So it is inferred based on the state of
        // the reserved bits in the EFI_FFS_FILE_STATE which spec requires to be set to EFI_FVB_ERASE_POLARITY.
        // This implementation does not support FV modification, so the only valid state is EFI_FILE_DATA_VALID.
        if (file_header.state & 0x80) == 0 {
            //erase polarity = 0. Verify DATA_VALID is set, and no higher-order bits are set.
            if file_header.state & 0xFC != ffs::file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(efi::Status::VOLUME_CORRUPTED)?;
            }
        } else {
            //erase polarity = 1. Verify DATA_VALID is clear, and no higher-order bits are clear.
            if (!file_header.state) & 0xFC != ffs::file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(efi::Status::VOLUME_CORRUPTED)?;
            }
        }

        //Verify the header checksum.
        let header_sum: Wrapping<u8> = buffer[..header_size].iter().map(|&x| Wrapping(x)).sum();
        // integrity_check_file and state are assumed to be zero for checksum, so subtract them here.
        let header_sum = header_sum.wrapping_sub(&Wrapping(file_header.integrity_check_file));
        let header_sum = header_sum.wrapping_sub(&Wrapping(file_header.state));
        if header_sum != Wrapping(0u8) {
            Err(efi::Status::VOLUME_CORRUPTED)?;
        }

        //Verify the file data checksum.
        if file_header.attributes & ffs::attributes::raw::CHECKSUM != 0 {
            let data_sum: Wrapping<u8> = buffer[header_size..size as usize].iter().map(|&x| Wrapping(x)).sum();
            if data_sum != Wrapping(0u8) {
                Err(efi::Status::VOLUME_CORRUPTED)?;
            }
        } else {
            // Verify that the checksum is initialized to 0xAA per spec requirements when CHECKSUM attribute is cleared.
            if file_header.integrity_check_file != 0xAA {
                Err(efi::Status::VOLUME_CORRUPTED)?;
            }
        }

        Ok(Self {
            data: &buffer[..size as usize],
            name: file_header.name,
            file_type: file_header.file_type,
            attributes: file_header.attributes,
            header_size,
            size,
        })
    }

    /// Returns the file type.
    pub fn file_type(&self) -> Option<FfsFileType> {
        match self.file_type {
            FfsFileRawType::RAW => Some(FfsFileType::Raw),
            FfsFileRawType::FREEFORM => Some(FfsFileType::FreeForm),
            FfsFileRawType::SECURITY_CORE => Some(FfsFileType::SecurityCore),
            FfsFileRawType::PEI_CORE => Some(FfsFileType::PeiCore),
            FfsFileRawType::DXE_CORE => Some(FfsFileType::DxeCore),
            FfsFileRawType::PEIM => Some(FfsFileType::Peim),
            FfsFileRawType::DRIVER => Some(FfsFileType::Driver),
            FfsFileRawType::COMBINED_PEIM_DRIVER => Some(FfsFileType::CombinedPeimDriver),
            FfsFileRawType::APPLICATION => Some(FfsFileType::Application),
            FfsFileRawType::MM => Some(FfsFileType::Mm),
            FfsFileRawType::FIRMWARE_VOLUME_IMAGE => Some(FfsFileType::FirmwareVolumeImage),
            FfsFileRawType::COMBINED_MM_DXE => Some(FfsFileType::CombinedMmDxe),
            FfsFileRawType::MM_CORE => Some(FfsFileType::MmCore),
            FfsFileRawType::MM_STANDALONE => Some(FfsFileType::MmStandalone),
            FfsFileRawType::MM_CORE_STANDALONE => Some(FfsFileType::MmCoreStandalone),
            FfsFileRawType::OEM_MIN..=FfsFileRawType::OEM_MAX => Some(FfsFileType::OemMin),
            FfsFileRawType::DEBUG_MIN..=FfsFileRawType::DEBUG_MAX => Some(FfsFileType::DebugMin),
            FfsFileRawType::FFS_PAD => Some(FfsFileType::FfsPad),
            FfsFileRawType::FFS_MIN..=FfsFileRawType::FFS_MAX => Some(FfsFileType::FfsUnknown),
            _ => None,
        }
    }

    /// Returns the file type as a raw u8.
    pub fn file_type_raw(&self) -> u8 {
        self.file_type
    }

    /// Returns the FV attributes for the file.
    pub fn fv_attributes(&self) -> EfiFvFileAttributes {
        let attributes = self.attributes;
        let data_alignment = (attributes & FfsRawAttribute::DATA_ALIGNMENT) >> 3;
        // decode alignment per Table 3.3 in PI spec 1.8 Part III.
        let mut file_attributes: u32 = match (
            data_alignment,
            (attributes & FfsRawAttribute::DATA_ALIGNMENT_2) == FfsRawAttribute::DATA_ALIGNMENT_2,
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
        if attributes & FfsRawAttribute::FIXED != 0 {
            file_attributes |= FvFileRawAttribute::FIXED;
        }
        file_attributes as EfiFvFileAttributes
    }

    /// Returns the file attributes as a raw u8
    pub fn attributes_raw(&self) -> u8 {
        self.attributes
    }

    /// Returns the file name GUID.
    pub fn name(&self) -> efi::Guid {
        self.name
    }

    /// Returns the size in bytes of the whole file, including the header.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the raw data from the file (without extracting any sections), not including the header.
    pub fn content(&self) -> &[u8] {
        &self.data[self.header_size..self.size as usize]
    }

    /// Returns the raw data for the file, including the header.
    pub fn data(&self) -> &[u8] {
        self.data
    }

    // Returns an iterator over the sections of this file (without extracting encapsulation sections).
    pub fn section_iter(&self) -> impl Iterator<Item = Result<Section, efi::Status>> + '_ {
        self.section_iter_with_extractor(&NullSectionExtractor {})
    }

    // Returns an iterator over the sections of this file, extracting encapsulation sections with the given extractor.
    pub fn section_iter_with_extractor<'b>(
        &'b self,
        extractor: &'b dyn SectionExtractor,
    ) -> impl Iterator<Item = Result<Section, efi::Status>> + 'b {
        FileSectionIterator::new(&self.data[self.header_size..self.size as usize], extractor)
    }
}

impl fmt::Debug for File<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("name", &self.name)
            .field("file_type", &self.file_type)
            .field("attributes", &self.attributes)
            .field("header_size", &self.header_size)
            .field("size", &self.size)
            .field("data.len()", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// Section Metadata
///
/// Describes the meta data in the section header (if any - most section types do not have metadata).
#[derive(Debug, Clone)]
pub enum SectionMetaData {
    None,
    Compression(FfsSectionHeader::Compression),
    GuidDefined(FfsSectionHeader::GuidDefined, Box<[u8]>),
    Version(FfsSectionHeader::Version),
    FreeformSubtypeGuid(FfsSectionHeader::FreeformSubtypeGuid),
}

/// Section access support
///
/// Provides access to section contents.
///
/// ## Example
///```
/// # use std::{env, fs, path::Path, error::Error};
/// use patina_pi::fw_fs::FirmwareVolume;
/// # fn main() -> Result<(), Box<dyn Error>> {
/// # let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
/// # let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
/// let fv = FirmwareVolume::new(&fv_bytes).expect("Firmware Volume Corrupt");
/// for file in fv.file_iter() {
///   let file = file.unwrap();
///   for section in file.section_iter() {
///     println!("{:#x?}", section);
///   }
/// }
/// # Ok(())
/// # }
///```
#[derive(Clone)]
pub struct Section {
    section_type: u8,
    meta_data: SectionMetaData,
    data: Box<[u8]>,
    section_size: usize,
}

impl Section {
    /// Instantiates a new Section by parsing the given buffer.
    ///
    /// The normal way to obtain a Section instance would be through the [`File::section_iter()`] method, but
    /// a constructor is provided here to enable independent instantiation of a section.
    pub fn new(buffer: &[u8]) -> Result<Self, efi::Status> {
        // verify that buffer has enough storage for a section header.
        if buffer.len() < mem::size_of::<section::Header>() {
            Err(efi::Status::INVALID_PARAMETER)?;
        }

        //Safety: buffer is large enough to contain the header, so can cast to a ref.
        let section_header = unsafe { &*(buffer.as_ptr() as *const section::Header) };

        //determine section_size and start of section content based on whether extended size field is present.
        let header_end = mem::size_of::<section::Header>();
        let (section_size, content_offset) = {
            if section_header.size.iter().all(|&x| x == 0xff) {
                //extended header - confirm there is space for extended size
                if buffer.len() < header_end + mem::size_of::<u32>() {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                let size =
                    u32::from_le_bytes(buffer[header_end..header_end + mem::size_of::<u32>()].try_into().unwrap());
                (size as usize, header_end + mem::size_of::<u32>())
            } else {
                //standard header
                let mut size_vec = section_header.size.to_vec();
                size_vec.push(0);
                let size = u32::from_le_bytes(size_vec.try_into().unwrap());
                (size as usize, header_end)
            }
        };

        let (meta_data, data) = match section_header.section_type {
            FfsSectionRawType::encapsulated::COMPRESSION => {
                let compression_header_size = mem::size_of::<section::header::Compression>();
                //verify that buffer has enough storage for a compression header.
                if buffer.len() < content_offset + compression_header_size {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                //Safety: buffer is large enough to hold compression header
                let compression_header =
                    unsafe { &*(buffer[content_offset..].as_ptr() as *const section::header::Compression) };
                let data: Box<[u8]> = Box::from(&buffer[content_offset + compression_header_size..section_size]);
                (SectionMetaData::Compression(*compression_header), data)
            }
            FfsSectionRawType::encapsulated::GUID_DEFINED => {
                let guid_defined_header_size = mem::size_of::<section::header::GuidDefined>();
                //verify that buffer has enough storage for a guid_defined header.
                if buffer.len() < content_offset + guid_defined_header_size {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                //Safety: buffer is large enough to hold guid_defined header
                let guid_defined =
                    unsafe { &*(buffer[content_offset..].as_ptr() as *const section::header::GuidDefined) };

                //verify that buffer has enough storage for guid-specific fields.
                let data_offset = guid_defined.data_offset as usize;
                if buffer.len() < data_offset {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }

                let guid_specific_header_fields: Box<[u8]> =
                    Box::from(&buffer[content_offset + guid_defined_header_size..data_offset]);
                let data: Box<[u8]> = Box::from(&buffer[data_offset..section_size]);

                (SectionMetaData::GuidDefined(*guid_defined, guid_specific_header_fields), data)
            }
            FfsSectionRawType::VERSION => {
                let version_header_size = mem::size_of::<section::header::Version>();
                //verify that buffer has enough storage for a version header.
                if buffer.len() < content_offset + version_header_size {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                //Safety: buffer is large enough to hold version header
                let version_header =
                    unsafe { &*(buffer[content_offset..].as_ptr() as *const section::header::Version) };
                let data: Box<[u8]> = Box::from(&buffer[content_offset + version_header_size..section_size]);
                (SectionMetaData::Version(*version_header), data)
            }
            FfsSectionRawType::FREEFORM_SUBTYPE_GUID => {
                let freeform_header_size = mem::size_of::<section::header::FreeformSubtypeGuid>();
                //verify that buffer has enough storage for a freeform header.
                if buffer.len() < content_offset + freeform_header_size {
                    Err(efi::Status::VOLUME_CORRUPTED)?;
                }
                //Safety: buffer is large enough to hold freeform header
                let freeform_header =
                    unsafe { &*(buffer[content_offset..].as_ptr() as *const section::header::FreeformSubtypeGuid) };
                let data: Box<[u8]> = Box::from(&buffer[content_offset + freeform_header_size..section_size]);
                (SectionMetaData::FreeformSubtypeGuid(*freeform_header), data)
            }
            FfsSectionRawType::OEM_MIN..=FfsSectionRawType::FFS_MAX => {
                //these section types do not have a defined header. So set metadata to none, and set data to the entire section buffer.
                let data: Box<[u8]> = Box::from(buffer);
                (SectionMetaData::None, data)
            }
            _ => {
                let data: Box<[u8]> = Box::from(&buffer[content_offset..section_size]);
                (SectionMetaData::None, data)
            }
        };

        Ok(Self { section_type: section_header.section_type, meta_data, data, section_size })
    }

    /// Returns the section type.
    pub fn section_type(&self) -> Option<FfsSectionType> {
        match self.section_type {
            FfsSectionRawType::encapsulated::COMPRESSION => Some(FfsSectionType::Compression),
            FfsSectionRawType::encapsulated::GUID_DEFINED => Some(FfsSectionType::GuidDefined),
            FfsSectionRawType::encapsulated::DISPOSABLE => Some(FfsSectionType::Disposable),
            FfsSectionRawType::PE32 => Some(FfsSectionType::Pe32),
            FfsSectionRawType::PIC => Some(FfsSectionType::Pic),
            FfsSectionRawType::TE => Some(FfsSectionType::Te),
            FfsSectionRawType::DXE_DEPEX => Some(FfsSectionType::DxeDepex),
            FfsSectionRawType::VERSION => Some(FfsSectionType::Version),
            FfsSectionRawType::USER_INTERFACE => Some(FfsSectionType::UserInterface),
            FfsSectionRawType::COMPATIBILITY16 => Some(FfsSectionType::Compatibility16),
            FfsSectionRawType::FIRMWARE_VOLUME_IMAGE => Some(FfsSectionType::FirmwareVolumeImage),
            FfsSectionRawType::FREEFORM_SUBTYPE_GUID => Some(FfsSectionType::FreeformSubtypeGuid),
            FfsSectionRawType::RAW => Some(FfsSectionType::Raw),
            FfsSectionRawType::PEI_DEPEX => Some(FfsSectionType::PeiDepex),
            FfsSectionRawType::MM_DEPEX => Some(FfsSectionType::MmDepex),
            _ => None,
        }
    }

    /// Returns the section type as a raw u8.
    pub fn section_type_raw(&self) -> u8 {
        self.section_type
    }

    /// Indicates whether this section is an encapsulation section (i.e. can be expended with a SectionExtractor).
    pub fn is_encapsulation(&self) -> bool {
        self.section_type() == Some(FfsSectionType::Compression)
            || self.section_type() == Some(FfsSectionType::GuidDefined)
    }

    /// Returns the section metadata.
    pub fn meta_data(&self) -> &SectionMetaData {
        &self.meta_data
    }

    /// Returns the section data.
    pub fn section_data(&self) -> &[u8] {
        &self.data
    }
    pub fn section_size(&self) -> usize {
        self.section_size
    }
}

impl fmt::Debug for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Section")
            .field("section_type", &self.section_type)
            .field("meta_data", &self.meta_data)
            .field("data.len()", &self.data.len())
            .finish_non_exhaustive()
    }
}

struct FvFileIterator<'a> {
    buffer: &'a [u8],
    erase_byte: u8,
    next_offset: usize,
    error: bool,
}

impl<'a> FvFileIterator<'a> {
    pub fn new(buffer: &'a [u8], erase_byte: u8) -> Self {
        FvFileIterator { buffer, erase_byte, next_offset: 0, error: false }
    }
}

impl<'a> Iterator for FvFileIterator<'a> {
    type Item = Result<File<'a>, efi::Status>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.error {
            return None;
        }
        if self.next_offset > self.buffer.len() {
            return None;
        }
        if self.buffer[self.next_offset..].len() < mem::size_of::<file::Header>() {
            return None;
        }
        if self.buffer[self.next_offset..self.next_offset + mem::size_of::<file::Header>()]
            .iter()
            .all(|&x| x == self.erase_byte)
        {
            return None;
        }
        let result = File::new(&self.buffer[self.next_offset..]);
        if let Ok(ref file) = result {
            // per the PI spec, "Given a file F, the next file FvHeader is located at the next 8-byte aligned firmware volume
            // offset following the last byte the file F"
            self.next_offset = align_up(self.next_offset as u64 + file.size(), 8) as usize;
        } else {
            self.error = true;
        }

        Some(result)
    }
}

struct FileSectionIterator<'a> {
    buffer: &'a [u8],
    extractor: &'a dyn SectionExtractor,
    next_offset: usize,
    error: bool,
    pending_extracted_sections: VecDeque<Result<Section, efi::Status>>,
}

impl<'a> FileSectionIterator<'a> {
    pub fn new(buffer: &'a [u8], extractor: &'a dyn SectionExtractor) -> Self {
        FileSectionIterator {
            buffer,
            extractor,
            next_offset: 0,
            error: false,
            pending_extracted_sections: VecDeque::new(),
        }
    }
}

impl Iterator for FileSectionIterator<'_> {
    type Item = Result<Section, efi::Status>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.error {
            return None;
        }

        if let Some(result) = self.pending_extracted_sections.pop_front() {
            if result.is_err() {
                self.error = true;
            }
            return Some(result);
        }

        if self.next_offset > self.buffer.len() {
            return None;
        }

        if self.buffer[self.next_offset..].len() < mem::size_of::<ffs::section::Header>() {
            return None;
        }
        let result = Section::new(&self.buffer[self.next_offset..]);
        if let Ok(ref section) = result {
            if section.is_encapsulation() {
                // attempt to extract the encapsulated section.
                match self.extractor.extract(section) {
                    Ok(extracted_buffer) => {
                        for section in FileSectionIterator::new(&extracted_buffer, self.extractor) {
                            self.pending_extracted_sections.push_back(section);
                        }
                    }
                    Err(err) => {
                        // on error, push the error on pending sections. This encapsulation section will be returned, and on the
                        // next iteration, the error will be returned.
                        self.pending_extracted_sections.push_back(Err(err));
                    }
                }
            }
            self.next_offset += align_up(section.section_size() as u64, 4) as usize;
        } else {
            self.error = true;
        }
        Some(result)
    }
}

#[cfg(test)]
mod unit_tests {
    use std::{
        collections::HashMap,
        env,
        error::Error,
        fs::{self, File},
        path::Path,
    };

    use core::{mem, sync::atomic::AtomicBool};
    use r_efi::efi;
    use serde::Deserialize;
    use uuid::Uuid;

    use crate::fw_fs::{SectionMetaData, guid};

    use super::{FfsSectionType, FirmwareVolume, NullSectionExtractor, Section, SectionExtractor, fv};

    #[derive(Debug, Deserialize)]
    struct TargetValues {
        total_number_of_files: u32,
        files_to_test: HashMap<String, FfsFileTargetValues>,
    }

    #[derive(Debug, Deserialize)]
    struct FfsFileTargetValues {
        file_type: u8,
        attributes: u8,
        size: u64,
        number_of_sections: usize,
        sections: HashMap<usize, FfsSectionTargetValues>,
    }

    #[derive(Debug, Deserialize)]
    struct FfsSectionTargetValues {
        section_type: Option<FfsSectionType>,
        size: u64,
        text: Option<String>,
    }

    fn stringify(error: efi::Status) -> String {
        format!("efi error: {:x?}", error).to_string()
    }

    fn test_firmware_volume_worker(
        fv: FirmwareVolume,
        mut expected_values: TargetValues,
        extractor: &dyn SectionExtractor,
    ) -> Result<(), Box<dyn Error>> {
        let mut count = 0;
        for ffs_file in fv.file_iter() {
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
                let sections: Result<Vec<Section>, efi::Status> =
                    ffs_file.section_iter_with_extractor(extractor).collect();
                let sections = sections.map_err(stringify)?;
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
                            section.section_type(),
                            "[{file_name}, section: {idx}] Error with the section Type"
                        );
                        assert_eq!(
                            target.size,
                            section.section_data().len() as u64,
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

    fn extract_text_from_section(section: &Section) -> Option<String> {
        if section.section_type() == Some(FfsSectionType::UserInterface) {
            let display_name_chars: Vec<u16> =
                section.section_data().chunks(2).map(|x| u16::from_le_bytes(x.try_into().unwrap())).collect();
            Some(String::from_utf16_lossy(&display_name_chars).trim_end_matches(char::from(0)).to_string())
        } else {
            None
        }
    }

    #[test]
    fn test_firmware_volume() -> Result<(), Box<dyn Error>> {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv = FirmwareVolume::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("DXEFV_expected_values.yml"))?)?;

        test_firmware_volume_worker(fv, expected_values, &NullSectionExtractor {})
    }

    #[test]
    fn test_giant_firmware_volume() -> Result<(), Box<dyn Error>> {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
        let fv = FirmwareVolume::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("GIGANTOR_expected_values.yml"))?)?;

        test_firmware_volume_worker(fv, expected_values, &NullSectionExtractor {})
    }

    #[test]
    fn test_section_extraction() -> Result<(), Box<dyn Error>> {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("FVMAIN_COMPACT.Fv"))?;

        let expected_values = serde_yaml::from_reader::<File, TargetValues>(File::open(
            root.join("FVMAIN_COMPACT_expected_values.yml"),
        )?)?;

        struct TestExtractor {
            invoked: AtomicBool,
        }

        impl SectionExtractor for TestExtractor {
            fn extract(&self, section: &Section) -> Result<Box<[u8]>, efi::Status> {
                let SectionMetaData::GuidDefined(metadata, _guid_specific) = section.meta_data() else {
                    panic!("Unexpected section metadata");
                };
                assert_eq!(metadata.section_definition_guid, guid::BROTLI_SECTION);
                self.invoked.store(true, core::sync::atomic::Ordering::SeqCst);
                Ok(Box::new([0u8; 0]))
            }
        }

        let test_extractor = TestExtractor { invoked: AtomicBool::new(false) };

        let fv = FirmwareVolume::new(&fv_bytes).unwrap();

        test_firmware_volume_worker(fv, expected_values, &test_extractor)?;

        assert!(test_extractor.invoked.load(core::sync::atomic::Ordering::SeqCst));

        Ok(())
    }

    #[test]
    fn test_malformed_firmware_volume() -> Result<(), Box<dyn Error>> {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        // bogus signature.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).signature ^= 0xdeadbeef;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus header_length.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).header_length = 0;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus checksum.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).checksum ^= 0xbeef;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus revision.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).revision = 1;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus filesystem guid.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).file_system_guid = efi::Guid::from_bytes(&[0xa5; 16]);
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus fv length.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).fv_length = 0;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        // bogus ext header offset.
        let mut fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_header = fv_bytes.as_mut_ptr() as *mut fv::Header;
        unsafe {
            (*fv_header).fv_length = ((*fv_header).ext_header_offset - 1) as u64;
        };
        assert_eq!(FirmwareVolume::new(&fv_bytes).unwrap_err(), efi::Status::VOLUME_CORRUPTED);

        Ok(())
    }

    #[test]
    fn zero_size_block_map_gives_same_offset_as_no_block_map() {
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
            assert_eq!((*a_ptr).block_map.as_ptr(), a_ptr.offset(1) as *const fv::BlockMapEntry);
        }
    }

    struct ExampleSectionExtractor {}
    impl SectionExtractor for ExampleSectionExtractor {
        fn extract(&self, section: &Section) -> Result<Box<[u8]>, efi::Status> {
            println!("Encapsulated section: {:?}", section);
            Ok(Box::new([0u8; 0])) //A real section extractor would provide the extracted buffer on return.
        }
    }

    #[test]
    fn section_extract_should_extract() -> Result<(), Box<dyn Error>> {
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");
        let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
        let fv = FirmwareVolume::new(&fv_bytes).expect("Firmware Volume Corrupt");
        for file in fv.file_iter() {
            let file = file.map_err(|_| "parse error".to_string())?;
            for (idx, section) in file.section_iter_with_extractor(&ExampleSectionExtractor {}).enumerate() {
                let section = section.map_err(|_| "parse error".to_string())?;
                println!("file: {:?}, section: {:?} type: {:?}", file.name(), idx, section.section_type());
            }
        }
        Ok(())
    }

    #[test]
    fn section_should_have_correct_metadata() -> Result<(), Box<dyn Error>> {
        let empty_pe32: [u8; 4] = [0x04, 0x00, 0x00, 0x10];
        let section = Section::new(&empty_pe32).unwrap();
        assert!(matches!(section.meta_data(), SectionMetaData::None));

        let empty_compression: [u8; 0x11] =
            [0x11, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let section = Section::new(&empty_compression).unwrap();
        match section.meta_data() {
            SectionMetaData::Compression(header) => {
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
        let section = Section::new(&empty_guid_defined).unwrap();
        match section.meta_data() {
            SectionMetaData::GuidDefined(header, guid_data) => {
                assert_eq!(
                    header.section_definition_guid,
                    efi::Guid::from_bytes(&[
                        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF
                    ])
                );
                assert_eq!(header.data_offset, 0x1C);
                assert_eq!(header.attributes, 0x3412);
                assert_eq!(guid_data.to_vec(), &[0x00u8, 0x01, 0x02, 0x03]);
                assert_eq!(section.section_data(), &[0x04, 0x15, 0x19, 0x80]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_version: [u8; 14] =
            [0x0E, 0x00, 0x00, 0x14, 0x00, 0x00, 0x31, 0x00, 0x2E, 0x00, 0x30, 0x00, 0x00, 0x00];
        let section = Section::new(&empty_version).unwrap();
        match section.meta_data() {
            SectionMetaData::Version(version) => {
                assert_eq!(version.build_number, 0);
                assert_eq!(section.section_data(), &[0x31, 0x00, 0x2E, 0x00, 0x30, 0x00, 0x00, 0x00]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_freeform_subtype: [u8; 24] = [
            0x18, 0x00, 0x00, 0x18, //Header
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, //GUID
            0x04, 0x15, 0x19, 0x80, //Data
        ];
        let section = Section::new(&empty_freeform_subtype).unwrap();
        match section.meta_data() {
            SectionMetaData::FreeformSubtypeGuid(ffst_header) => {
                assert_eq!(
                    ffst_header.sub_type_guid,
                    efi::Guid::from_bytes(&[
                        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF
                    ])
                );
                assert_eq!(section.section_data(), &[0x04, 0x15, 0x19, 0x80]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        Ok(())
    }
}
