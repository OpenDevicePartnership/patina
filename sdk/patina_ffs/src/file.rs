//! Firmware File System (FFS) file parsing and composition.
//!
//! This module provides:
//! - `FileRef`: a zero-copy, read-only view over a serialized FFS file backed by a byte slice.
//! - `File`: an owned, mutable builder for creating and serializing FFS files from sections.
//!
//! Both types help inspect file metadata (name GUID, type, attributes), traverse sections, and
//! serialize with correct headers, checksums, and alignment.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use mu_pi::fw_fs::{
    ffs::{self, attributes, file},
    fv,
};

use crate::{
    FirmwareFileSystemError,
    section::{Section, SectionComposer, SectionExtractor, SectionIterator},
};

use alloc::vec::Vec;
use core::{fmt, iter, mem, ptr, slice::from_raw_parts};
use r_efi::efi;

/// Zero-copy view over a Firmware File System (FFS) file backed by a byte slice.
///
/// Parsing via [`FileRef::new`] validates the header, state, and (optionally) the
/// data checksum. The lifetime `'a` ties the instance to the provided buffer.
/// Accessors expose file metadata (name, type, attributes), content, and sections.
#[derive(Clone)]
pub struct FileRef<'a> {
    data: &'a [u8],
    header: file::Header,
    erase_polarity: bool,
    size: usize,
    content_offset: usize,
}

impl<'a> FileRef<'a> {
    /// Parse a byte slice as an FFS file and validate header/state/checksums.
    ///
    /// Returns a zero-copy view over the provided buffer.
    ///
    /// Errors
    /// - [`FirmwareFileSystemError::InvalidHeader`]: malformed header or size.
    /// - [`FirmwareFileSystemError::InvalidState`]: file state not DATA_VALID.
    /// - [`FirmwareFileSystemError::DataCorrupt`]: data checksum mismatch.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use patina_ffs::file::{File, FileRef};
    /// use r_efi::efi;
    /// use mu_pi::fw_fs::ffs;
    /// use patina_ffs::section::{Section, SectionHeader};
    ///
    /// // Build a file and then parse the resulting bytes back.
    /// let guid = efi::Guid::from_bytes(&[0u8; 16]);
    /// let mut file = File::new(guid, 0x07);
    /// let data = b"hello".to_vec();
    /// let section = Section::new_from_header_with_data(
    ///     SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///     data,
    /// ).unwrap();
    /// file.sections_mut().push(section);
    /// let bytes = file.serialize().unwrap();
    /// let file_ref = FileRef::new(&bytes).unwrap();
    /// assert_eq!(file_ref.file_type_raw(), 0x07);
    /// ```
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
                let mut size = [00u8; 4];
                size[0..3].copy_from_slice(&header.size);
                let size = u32::from_le_bytes(size) as usize;
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
        let erase_polarity;
        if (header.state & 0x80) == 0 {
            //erase polarity = 0. Verify DATA_VALID is set, and no higher-order bits are set.
            erase_polarity = false;
            if header.state & 0xFC != file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(FirmwareFileSystemError::InvalidState)?;
            }
        } else {
            //erase polarity = 1. Verify DATA_VALID is clear, and no higher-order bits are clear.
            erase_polarity = true;
            if (!header.state) & 0xFC != file::raw::state::DATA_VALID {
                //file is not in EFI_FILE_DATA_VALID state.
                Err(FirmwareFileSystemError::InvalidState)?;
            }
        }

        // Verify the file header checksum.
        let sum = buffer[..content_offset].iter().fold(0u8, |sum, val| sum.wrapping_add(*val));
        let sum = sum.wrapping_sub(header.state);
        let sum = sum.wrapping_sub(header.integrity_check_file);
        if sum != 0 {
            Err(FirmwareFileSystemError::InvalidHeader)?;
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
        Ok(Self { data: &buffer[..size], header, erase_polarity, size, content_offset })
    }

    /// Total serialized size of the file in bytes (header + content).
    pub fn size(&self) -> usize {
        self.size
    }

    /// The file name GUID from the FFS header.
    pub fn name(&self) -> efi::Guid {
        self.header.name
    }

    /// The raw FFS file type byte.
    pub fn file_type_raw(&self) -> u8 {
        self.header.file_type
    }

    /// The file payload bytes (sections area), excluding the header.
    pub fn content(&self) -> &[u8] {
        &self.data[self.content_offset..]
    }

    /// Byte offset from the start of the file to the beginning of content.
    pub fn content_offset(&self) -> usize {
        self.content_offset
    }

    /// The complete file bytes, including header and content.
    pub fn data(&self) -> &[u8] {
        self.data
    }

    /// Erase polarity encoded in the header; `true` for erase=1, `false` for erase=0.
    pub fn erase_polarity(&self) -> bool {
        self.erase_polarity
    }

    /// Decode the file's attributes into `EFI_FV_FILE_ATTRIBUTES` per PI spec.
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

    /// The raw attributes byte from the FFS header.
    pub fn attributes_raw(&self) -> u8 {
        self.header.attributes
    }

    /// Parse and collect all sections contained in this file.
    ///
    /// Returns a flattened list of sections; nested containers are expanded.
    pub fn sections(&self) -> Result<Vec<Section>, FirmwareFileSystemError> {
        let sections = SectionIterator::new(&self.data[self.content_offset..])
            .collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;
        Ok(sections.iter().flat_map(|x| x.sections().cloned().collect::<Vec<_>>()).collect())
    }

    /// Parse sections and run the provided extractor on each extracted section.
    ///
    /// Useful for decoding compressed or encapsulated sections during traversal.
    ///
    /// ##Examples
    ///
    /// ```rust no_run
    /// use mu_pi::fw_fs::ffs;
    /// use r_efi::efi;
    /// use patina_ffs::file::{File, FileRef};
    /// use patina_ffs::FirmwareFileSystemError;
    /// use patina_ffs::section::{Section, SectionHeader, SectionExtractor};
    ///
    /// // A dummy extractor that declines to handle any section types.
    /// struct Dummy;
    /// impl SectionExtractor for Dummy {
    ///     fn extract(&self, _section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError> {
    ///         Err(FirmwareFileSystemError::Unsupported)
    ///     }
    /// }
    ///
    /// // Build a simple file containing a single RAW section and parse it back.
    /// let guid = efi::Guid::from_bytes(&[0u8; 16]);
    /// let mut file = File::new(guid, 0x07);
    /// let data = b"hello".to_vec();
    /// let section = Section::new_from_header_with_data(
    ///     SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///     data,
    /// ).unwrap();
    /// file.sections_mut().push(section);
    /// let bytes = file.serialize().unwrap();
    ///
    /// let file_ref = FileRef::new(&bytes).unwrap();
    /// let sections = file_ref.sections_with_extractor(&Dummy).unwrap();
    /// assert!(!sections.is_empty());
    /// ```
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

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileRef")
            .field("data (bytes)", &self.data.len())
            .field("header", &self.header)
            .field("erase_polarity", &self.erase_polarity)
            .field("size", &self.size)
            .field("content_offset", &self.content_offset)
            .finish()
    }
}

/// Owned, mutable representation of an FFS file for composition and serialization.
///
/// Use this to build files programmatically from sections, control attributes
/// (e.g., data checksum, large-file), select erase polarity, and then
/// [`serialize`](Self::serialize) into a well-formed byte stream.
pub struct File {
    name: efi::Guid,
    file_type_raw: u8,
    attributes: u8,
    erase_polarity: bool,
    sections: Vec<Section>,
}

impl File {
    /// Create a new, empty FFS file builder with the given name and type.
    pub fn new(name: efi::Guid, file_type_raw: u8) -> Self {
        Self { name, file_type_raw, attributes: 0, erase_polarity: true, sections: Vec::new() }
    }

    /// Serialize the file into a valid FFS byte stream.
    ///
    /// Produces the correct header variant (standard or large), includes
    /// required padding between sections, and computes header/data checksums.
    /// Errors from section serialization are propagated.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use mu_pi::fw_fs::ffs;
    /// use r_efi::efi;
    /// use patina_ffs::file::File;
    /// use patina_ffs::section::{Section, SectionHeader};
    ///
    /// let guid = efi::Guid::from_bytes(&[0u8; 16]);
    /// let mut file = File::new(guid, 0x07);
    /// file.set_data_checksum(true);
    ///
    /// // Add a simple RAW section with content "hello".
    /// let data = b"hello".to_vec();
    /// let section = Section::new_from_header_with_data(
    ///     SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///     data,
    /// ).unwrap();
    /// file.sections_mut().push(section);
    /// let bytes = file.serialize().unwrap();
    /// assert!(!bytes.is_empty());
    /// ```
    pub fn serialize(&self) -> Result<Vec<u8>, FirmwareFileSystemError> {
        let mut content = Vec::new();

        let mut section_iter = self.sections.iter().peekable();

        while let Some(section) = &section_iter.next() {
            content.extend_from_slice(&section.serialize()?);
            if section_iter.peek().is_some() {
                //pad to next 4-byte aligned length, since sections start at 4-byte aligned offsets. No padding is added
                //after the last section.
                if content.len() % 4 != 0 {
                    let pad_length = 4 - (content.len() % 4);
                    //Per PI 1.8A volume 3 section 2.2.4, pad byte is always zero.
                    content.extend(iter::repeat_n(0u8, pad_length));
                }
            }
        }

        let mut header = {
            if ((self.attributes & attributes::raw::LARGE_FILE) != 0)
                || content.len() > 0xffffff - mem::size_of::<ffs::file::Header>()
            {
                self.serialize_header2(&content)
            } else {
                self.serialize_header(&content)
            }
        };

        header.extend(content);
        Ok(header)
    }

    // Generate a serialized FFS file header for small files that uses ffs::file::Header2
    fn serialize_header2(&self, content: &[u8]) -> Vec<u8> {
        let mut file_header = ffs::file::Header2 {
            header: ffs::file::Header {
                name: self.name,
                integrity_check_header: 0,
                integrity_check_file: 0,
                file_type: self.file_type_raw,
                attributes: self.attributes | attributes::raw::LARGE_FILE,
                size: [0u8; 3],
                state: 0,
            },
            extended_size: 0,
        };
        file_header.extended_size = (mem::size_of_val(&file_header) + content.len()) as u64;

        // calculate checksum (excludes state and integrity_check_file, set to zero)
        // safety: file_header is repr(C), safe to represent as byte slice for checksum
        let header_slice =
            unsafe { from_raw_parts(&raw const file_header as *const u8, mem::size_of_val(&file_header)) };
        let sum = header_slice.iter().fold(0u8, |sum, value| sum.wrapping_add(*value));
        file_header.header.integrity_check_header = 0u8.wrapping_sub(sum);

        // calculate file data check
        if self.is_data_checksum() {
            let sum = content.iter().fold(0u8, |sum, value| sum.wrapping_add(*value));
            file_header.header.integrity_check_file = 0u8.wrapping_sub(sum);
        } else {
            file_header.header.integrity_check_file = 0xaau8;
        }

        file_header.header.state = ffs::file::raw::state::HEADER_CONSTRUCTION
            | ffs::file::raw::state::HEADER_VALID
            | ffs::file::raw::state::DATA_VALID;
        if self.erase_polarity {
            file_header.header.state = !file_header.header.state;
        }

        let header_slice =
            unsafe { from_raw_parts(&raw const file_header as *const u8, mem::size_of_val(&file_header)) };
        header_slice.to_vec()
    }

    // Generate a serialized FFS file header for small files that uses ffs::file::Header.
    fn serialize_header(&self, content: &[u8]) -> Vec<u8> {
        let mut file_header = ffs::file::Header {
            name: self.name,
            integrity_check_header: 0,
            integrity_check_file: 0,
            file_type: self.file_type_raw,
            attributes: self.attributes,
            size: [0u8; 3],
            state: 0,
        };
        let size = mem::size_of_val(&file_header) + content.len();
        file_header.size.copy_from_slice(&size.to_le_bytes()[0..3]);

        // calculate checksum (excludes state and integrity_check_file, set to zero)
        // safety: file_header is repr(C), safe to represent as byte slice for checksum
        let header_slice =
            unsafe { from_raw_parts(&raw const file_header as *const u8, mem::size_of_val(&file_header)) };
        let sum = header_slice.iter().fold(0u8, |sum, value| sum.wrapping_add(*value));
        file_header.integrity_check_header = 0u8.wrapping_sub(sum);

        // calculate file data check
        if self.is_data_checksum() {
            let sum = content.iter().fold(0u8, |sum, value| sum.wrapping_add(*value));
            file_header.integrity_check_file = 0u8.wrapping_sub(sum);
        } else {
            file_header.integrity_check_file = 0xaau8;
        }

        file_header.state = ffs::file::raw::state::HEADER_CONSTRUCTION
            | ffs::file::raw::state::HEADER_VALID
            | ffs::file::raw::state::DATA_VALID;
        if self.erase_polarity {
            file_header.state = !file_header.state;
        }

        let header_slice =
            unsafe { from_raw_parts(&raw const file_header as *const u8, mem::size_of_val(&file_header)) };
        header_slice.to_vec()
    }

    /// Set the erase polarity to encode in the header state bits.
    ///
    /// `true` => erase=1 (bits set), `false` => erase=0 (bits clear).
    pub fn set_erase_polarity(&mut self, erase_polarity: bool) {
        self.erase_polarity = erase_polarity;
    }

    /// Enable or disable the data checksum attribute for the file contents.
    pub fn set_data_checksum(&mut self, checksum: bool) {
        if checksum {
            self.attributes |= attributes::raw::CHECKSUM;
        } else {
            self.attributes &= !attributes::raw::CHECKSUM;
        }
    }

    /// Returns `true` if the file has the data checksum attribute set.
    pub fn is_data_checksum(&self) -> bool {
        self.attributes & attributes::raw::CHECKSUM != 0
    }

    /// Compute the header size (offset to content) for the current sections/attributes.
    ///
    /// Uses section lengths to decide whether a large-file header is required.
    pub fn content_offset(&self) -> Result<usize, FirmwareFileSystemError> {
        if self.attributes & attributes::raw::LARGE_FILE != 0 {
            Ok(mem::size_of::<ffs::file::Header2>())
        } else {
            let mut section_iter = self.sections.iter().peekable();
            let mut content_len = 0;
            while let Some(section) = &section_iter.next() {
                let section_len = section.serialize()?.len();
                content_len += section_len;
                if section_iter.peek().is_some() {
                    //pad to next 4-byte aligned length, since sections start at 4-byte aligned offsets. No padding is added
                    //after the last section.
                    let rem = section_len % 4;
                    if rem != 0 {
                        //Per PI 1.8A volume 3 section 2.2.4, pad byte is always zero.
                        content_len += 4 - rem;
                    }
                }
            }
            if content_len + mem::size_of::<ffs::file::Header>() > 0xffffff {
                Ok(mem::size_of::<ffs::file::Header2>())
            } else {
                Ok(mem::size_of::<ffs::file::Header>())
            }
        }
    }

    /// Run the provided extractor over all sections in-place.
    pub fn extract(&mut self, extractor: &dyn SectionExtractor) -> Result<(), FirmwareFileSystemError> {
        for section in self.sections.iter_mut() {
            section.extract(extractor)?;
        }
        Ok(())
    }

    /// Compose sections with the given composer and then serialize the file.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use mu_pi::fw_fs::ffs;
    /// use r_efi::efi;
    /// use patina_ffs::file::File;
    /// use patina_ffs::section::{Section, SectionHeader, SectionComposer};
    /// use patina_ffs::section::{SectionHeader as SH};
    /// use patina_ffs::FirmwareFileSystemError;
    ///
    /// // A trivial composer that passes through existing headers and data.
    /// struct Passthrough;
    /// impl SectionComposer for Passthrough {
    ///     fn compose(&self, section: &Section) -> Result<(SH, Vec<u8>), FirmwareFileSystemError> {
    ///         Ok((section.header().clone(), section.try_content_as_slice()?.to_vec()))
    ///     }
    /// }
    ///
    /// let guid = efi::Guid::from_bytes(&[0u8; 16]);
    /// let mut file = File::new(guid, 0x07);
    /// let data = b"hello".to_vec();
    /// file.sections_mut().push(Section::new_from_header_with_data(
    ///     SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///     data,
    /// ).unwrap());
    /// let _bytes = file.serialize_with_composer(&Passthrough).unwrap();
    /// ```
    pub fn serialize_with_composer(
        &mut self,
        composer: &dyn SectionComposer,
    ) -> Result<Vec<u8>, FirmwareFileSystemError> {
        self.compose(composer)?;
        self.serialize()
    }

    /// Compose all sections using the provided composer.
    ///
    /// ## Examples
    ///
    /// ```rust no_run
    /// use mu_pi::fw_fs::ffs;
    /// use r_efi::efi;
    /// use patina_ffs::file::File;
    /// use patina_ffs::section::{Section, SectionHeader, SectionComposer};
    /// use patina_ffs::section::SectionHeader as SH;
    /// use patina_ffs::FirmwareFileSystemError;
    ///
    /// struct Passthrough;
    /// impl SectionComposer for Passthrough {
    ///     fn compose(&self, section: &Section) -> Result<(SH, Vec<u8>), FirmwareFileSystemError> {
    ///         Ok((section.header().clone(), section.try_content_as_slice()?.to_vec()))
    ///     }
    /// }
    ///
    /// let guid = efi::Guid::from_bytes(&[0u8; 16]);
    /// let mut file = File::new(guid, 0x07);
    /// let data = b"hello".to_vec();
    /// file.sections_mut().push(Section::new_from_header_with_data(
    ///     SectionHeader::Standard(ffs::section::raw_type::RAW, data.len() as u32),
    ///     data,
    /// ).unwrap());
    /// file.compose(&Passthrough).unwrap();
    /// ```
    pub fn compose(&mut self, composer: &dyn SectionComposer) -> Result<(), FirmwareFileSystemError> {
        for section in self.sections.iter_mut() {
            section.compose(composer)?;
        }
        Ok(())
    }

    /// Read-only access to the list of top-level sections in this file.
    ///
    /// Note: This returns only the file's immediate sections. For flattened traversal,
    /// use [`File::section_iter`], and for nested access per item see [`Section::sub_sections`].
    pub fn sections(&self) -> &Vec<Section> {
        &self.sections
    }

    /// Mutable access to the list of top-level sections in this file.
    ///
    /// Note: This does not provide flattened mutable traversal. To mutate nested sections,
    /// iterate the returned slice and call [`Section::sub_sections_mut`] on each as needed.
    pub fn sections_mut(&mut self) -> &mut Vec<Section> {
        &mut self.sections
    }

    /// Iterate over all (flattened) sections in this file.
    pub fn section_iter(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().flat_map(|x| x.sections())
    }

    /// Iterate over top-level sections in this file mutably.
    ///
    /// Note: This yields only the top-level sections. To traverse nested
    /// sections mutably, call [`Section::sub_sections_mut`] on each item.
    pub fn section_iter_mut(&mut self) -> impl Iterator<Item = &mut Section> {
        self.sections.iter_mut()
    }

    /// The file name GUID set for this file.
    pub fn name(&self) -> efi::Guid {
        self.name
    }

    /// The raw FFS file type byte set for this file.
    pub fn file_type_raw(&self) -> u8 {
        self.file_type_raw
    }

    /// The raw attributes byte currently set for this file.
    pub fn attributes_raw(&self) -> u8 {
        self.attributes
    }
}

impl TryFrom<FileRef<'_>> for File {
    type Error = FirmwareFileSystemError;

    fn try_from(src: FileRef<'_>) -> Result<Self, Self::Error> {
        Ok(Self {
            name: src.name(),
            file_type_raw: src.file_type_raw(),
            attributes: src.attributes_raw(),
            erase_polarity: src.erase_polarity(),
            sections: src.sections()?,
        })
    }
}

impl TryFrom<(FileRef<'_>, &dyn SectionExtractor)> for File {
    type Error = FirmwareFileSystemError;

    fn try_from(src: (FileRef<'_>, &dyn SectionExtractor)) -> Result<Self, Self::Error> {
        let (src, extractor) = src;
        let mut sections = src.sections()?;
        for section in sections.iter_mut() {
            section.extract(extractor)?
        }
        Ok(Self {
            name: src.name(),
            file_type_raw: src.file_type_raw(),
            attributes: src.attributes_raw(),
            erase_polarity: src.erase_polarity(),
            sections,
        })
    }
}
