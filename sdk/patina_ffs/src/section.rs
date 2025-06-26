use alloc::{boxed::Box, format, vec, vec::Vec};
use mu_pi::fw_fs::ffs::{self, section};
use patina_sdk::base::align_up;

use core::{fmt, iter, mem, ptr};

use crate::FirmwareFileSystemError;

pub trait SectionExtractor {
    fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError>;
}

pub trait SectionComposer {
    fn compose(&self, section: &Section) -> Result<(SectionMetaData, Vec<u8>), FirmwareFileSystemError>;
}

#[derive(Debug, Clone)]
pub enum SectionMetaData {
    Standard(section::EfiSectionType, usize),
    Compression(section::header::Compression, usize),
    GuidDefined(section::header::GuidDefined, Vec<u8>, usize),
    Version(section::header::Version, usize),
    FreeFormSubtypeGuid(section::header::FreeformSubtypeGuid, usize),
}

impl SectionMetaData {
    pub fn content_offset(&self) -> usize {
        match self {
            SectionMetaData::Standard(_, offset)
            | SectionMetaData::Compression(_, offset)
            | SectionMetaData::GuidDefined(_, _, offset)
            | SectionMetaData::Version(_, offset)
            | SectionMetaData::FreeFormSubtypeGuid(_, offset) => *offset,
        }
    }
}

#[derive(Clone)]
enum SectionData {
    None,
    Composed(Vec<u8>),
    Extracted(Vec<Section>),
    Both(Vec<u8>, Vec<Section>),
}

impl fmt::Debug for SectionData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Composed(arg0) => f.debug_tuple("Composed").field(&format!("{:#x} bytes", arg0.len())).finish(),
            Self::Extracted(arg0) => f.debug_tuple("Extracted").field(arg0).finish(),
            Self::Both(arg0, arg1) => {
                f.debug_tuple("Both").field(&format!("{:#x} bytes", arg0.len())).field(arg1).finish()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Section {
    meta: SectionMetaData,
    data: SectionData,
}

impl Section {
    pub fn new_from_meta(meta: SectionMetaData) -> Self {
        Self { meta, data: SectionData::None }
    }

    pub fn new_from_buffer(buffer: &[u8]) -> Result<Self, FirmwareFileSystemError> {
        // Verify that the buffer has enough storage for a section header.
        if buffer.len() < mem::size_of::<section::Header>() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Safety: buffer is large enough to contain the header.
        let section_header = unsafe { ptr::read_unaligned(buffer.as_ptr() as *const section::Header) };

        // Determine section size and start of section content
        let (section_size, section_data_offset) = {
            if section_header.size.iter().all(|&x| x == 0xff) {
                // size field is all 0xFF - this indicates extended header.
                let ext_header_size = mem::size_of::<section::header::CommonSectionHeaderExtended>();
                if buffer.len() < ext_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to contain extended header.
                let ext_header = unsafe {
                    ptr::read_unaligned(buffer.as_ptr() as *const section::header::CommonSectionHeaderExtended)
                };
                (ext_header.extended_size as usize, ext_header_size)
            } else {
                //standard header.
                let mut size = vec![0x00u8; 4];
                size[0..3].copy_from_slice(&section_header.size);
                let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
                (size, core::mem::size_of::<section::Header>())
            }
        };

        // Verify that the buffer has enough space for the entire section.
        if buffer.len() < section_size {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // For spec-defined section types, validate the section-specific headers.
        let meta = match section_header.section_type {
            section::raw_type::encapsulated::COMPRESSION => {
                let compression_header_size = mem::size_of::<section::header::Compression>();
                // verify that the buffer is large enough to hold the compresion header.
                if buffer.len() < section_data_offset + compression_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the compression header.
                let compression_header = unsafe {
                    ptr::read_unaligned(buffer[section_data_offset..].as_ptr() as *const section::header::Compression)
                };
                SectionMetaData::Compression(compression_header, section_data_offset + compression_header_size)
            }
            section::raw_type::encapsulated::GUID_DEFINED => {
                // verify that the buffer is large enough to hold the GuidDefined header.
                let guid_header_size = mem::size_of::<section::header::GuidDefined>();
                if buffer.len() < section_data_offset + guid_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the GuidDefined header.
                let guid_defined_header = unsafe {
                    ptr::read_unaligned(buffer[section_data_offset..].as_ptr() as *const section::header::GuidDefined)
                };

                // Verify that buffer has enough storage for guid-specific fields.
                let data_offset = guid_defined_header.data_offset as usize;
                if buffer.len() < data_offset {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }

                let guid_specific_data = buffer[section_data_offset + guid_header_size..data_offset].to_vec();

                SectionMetaData::GuidDefined(guid_defined_header, guid_specific_data, data_offset)
            }
            section::raw_type::VERSION => {
                let version_header_size = mem::size_of::<section::header::Version>();
                // verify that the buffer is large enough to hold the Version header.
                if buffer.len() < section_data_offset + version_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the version header.
                let version_header = unsafe {
                    ptr::read_unaligned(buffer[section_data_offset..].as_ptr() as *const section::header::Version)
                };
                SectionMetaData::Version(version_header, section_data_offset + version_header_size)
            }
            section::raw_type::FREEFORM_SUBTYPE_GUID => {
                // verify that the buffer is large enough to hold the FreeformSubtypeGuid header.
                let freeform_subtype_size = mem::size_of::<section::header::FreeformSubtypeGuid>();
                if buffer.len() < section_data_offset + freeform_subtype_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the freeform header type
                let freeform_header = unsafe {
                    ptr::read_unaligned(
                        buffer[section_data_offset..].as_ptr() as *const section::header::FreeformSubtypeGuid
                    )
                };
                SectionMetaData::FreeFormSubtypeGuid(freeform_header, section_data_offset + freeform_subtype_size)
            }
            _ => SectionMetaData::Standard(section_header.section_type, section_data_offset), //for all other types, the content immediately follows the standard header.
        };

        Ok(Section { meta, data: SectionData::Composed(buffer[..section_size].to_vec()) })
    }

    pub fn new_from_meta_with_sections(meta: SectionMetaData, sections: Vec<Section>) -> Self {
        Self { meta, data: SectionData::Extracted(sections) }
    }

    pub fn metadata(&self) -> &SectionMetaData {
        &self.meta
    }

    pub fn size(&self) -> Result<usize, FirmwareFileSystemError> {
        match &self.data {
            SectionData::None | SectionData::Extracted(_) => Err(FirmwareFileSystemError::NotComposed),
            SectionData::Composed(data) | SectionData::Both(data, _) => Ok(data.len()),
        }
    }

    pub fn section_type_raw(&self) -> u8 {
        match &self.meta {
            SectionMetaData::Standard(raw_type, _) => *raw_type,
            SectionMetaData::Compression(_, _) => ffs::section::raw_type::encapsulated::COMPRESSION,
            SectionMetaData::GuidDefined(_, _, _) => ffs::section::raw_type::encapsulated::GUID_DEFINED,
            SectionMetaData::Version(_, _) => ffs::section::raw_type::VERSION,
            SectionMetaData::FreeFormSubtypeGuid(_, _) => ffs::section::raw_type::FREEFORM_SUBTYPE_GUID,
        }
    }
    pub fn section_type(&self) -> Option<ffs::section::Type> {
        match &self.meta {
            SectionMetaData::Standard(section_type_raw, _) => match *section_type_raw {
                ffs::section::raw_type::encapsulated::DISPOSABLE => Some(ffs::section::Type::Disposable),
                ffs::section::raw_type::PE32 => Some(ffs::section::Type::Pe32),
                ffs::section::raw_type::PIC => Some(ffs::section::Type::Pic),
                ffs::section::raw_type::TE => Some(ffs::section::Type::Te),
                ffs::section::raw_type::DXE_DEPEX => Some(ffs::section::Type::DxeDepex),
                ffs::section::raw_type::USER_INTERFACE => Some(ffs::section::Type::UserInterface),
                ffs::section::raw_type::COMPATIBILITY16 => Some(ffs::section::Type::Compatibility16),
                ffs::section::raw_type::FIRMWARE_VOLUME_IMAGE => Some(ffs::section::Type::FirmwareVolumeImage),
                ffs::section::raw_type::RAW => Some(ffs::section::Type::Raw),
                ffs::section::raw_type::PEI_DEPEX => Some(ffs::section::Type::PeiDepex),
                ffs::section::raw_type::MM_DEPEX => Some(ffs::section::Type::MmDepex),
                _ => None,
            },
            SectionMetaData::Compression(_, _) => Some(ffs::section::Type::Compression),
            SectionMetaData::GuidDefined(_, _, _) => Some(ffs::section::Type::GuidDefined),
            SectionMetaData::Version(_, _) => Some(ffs::section::Type::Version),
            SectionMetaData::FreeFormSubtypeGuid(_, _) => Some(ffs::section::Type::FreeformSubtypeGuid),
        }
    }

    pub fn compose(&mut self, composer: &dyn SectionComposer) -> Result<(), FirmwareFileSystemError> {
        let sections = match &mut self.data {
            SectionData::None | SectionData::Composed(_) => return Ok(()), //nothing to do
            SectionData::Extracted(sections) => sections,
            SectionData::Both(_, sections) => sections,
        };

        for section in sections {
            section.compose(composer)?;
        }

        let (meta, content) = composer.compose(self)?;
        let old_data = mem::replace(&mut self.data, SectionData::None);

        self.data = match old_data {
            SectionData::None | SectionData::Composed(_) => unreachable!(), // returned above.
            SectionData::Extracted(sections) | SectionData::Both(_, sections) => SectionData::Both(content, sections),
        };
        self.meta = meta;

        Ok(())
    }

    pub fn extract(&mut self, extractor: &dyn SectionExtractor) -> Result<(), FirmwareFileSystemError> {
        let extracted_data = match self.data {
            SectionData::None | SectionData::Extracted(_) => return Ok(()), //nothing to do.
            SectionData::Composed(_) | SectionData::Both(_, _) => {
                match extractor.extract(self) {
                    Err(FirmwareFileSystemError::Unsupported) => Vec::new(), //unsupported section, so no subsections.
                    result => result?,
                }
            }
        };

        let mut sections: Vec<Section> =
            SectionIterator::new(&extracted_data).collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;
        for section in sections.iter_mut() {
            section.extract(extractor)?;
        }

        let old_data = mem::replace(&mut self.data, SectionData::None);
        self.data = match old_data {
            SectionData::None | SectionData::Extracted(_) => unreachable!(), // returned above.
            SectionData::Composed(content) | SectionData::Both(content, _) => SectionData::Both(content, sections),
        };
        Ok(())
    }

    pub fn try_as_slice(&self) -> Result<&[u8], FirmwareFileSystemError> {
        match &self.data {
            SectionData::None | SectionData::Extracted(_) => Err(FirmwareFileSystemError::NotComposed),
            SectionData::Composed(data) | SectionData::Both(data, _) => Ok(data),
        }
    }

    pub fn try_content_as_slice(&self) -> Result<&[u8], FirmwareFileSystemError> {
        let content_offset = self.meta.content_offset();
        match &self.data {
            SectionData::None | SectionData::Extracted(_) => Err(FirmwareFileSystemError::NotComposed),
            SectionData::Composed(data) | SectionData::Both(data, _) => Ok(&data[content_offset..]),
        }
    }

    pub fn try_into_boxed_slice(self) -> Result<Box<[u8]>, FirmwareFileSystemError> {
        match self.data {
            SectionData::None | SectionData::Extracted(_) => Err(FirmwareFileSystemError::NotComposed),
            SectionData::Composed(data) | SectionData::Both(data, _) => Ok(data.into_boxed_slice()),
        }
    }

    pub fn into_sections(self) -> Box<dyn Iterator<Item = Section>> {
        match self.data {
            SectionData::None => Box::new(iter::empty()),
            SectionData::Composed(_) => Box::new(iter::once(self)),
            SectionData::Extracted(sections) => Box::new(sections.into_iter().flat_map(|x| x.into_sections())),
            SectionData::Both(data, sections) => {
                let current = Self { meta: self.meta, data: SectionData::Composed(data) };
                Box::new(iter::once(current).chain(sections.into_iter().flat_map(|x| x.clone().into_sections())))
            }
        }
    }

    pub fn sections(&self) -> Box<dyn Iterator<Item = &Section> + '_> {
        match &self.data {
            SectionData::None => Box::new(iter::empty()),
            SectionData::Composed(_) => Box::new(iter::once(self)),
            SectionData::Extracted(sections) => Box::new(sections.iter().flat_map(|x| x.sections())),
            SectionData::Both(_, sections) => {
                Box::new(iter::once(self).chain(sections.iter().flat_map(|x| x.sections())))
            }
        }
    }
}
pub struct SectionIterator<'a> {
    data: &'a [u8],
    next_offset: usize,
    error: bool,
}

impl<'a> SectionIterator<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, next_offset: 0, error: false }
    }
}

impl Iterator for SectionIterator<'_> {
    type Item = Result<Section, FirmwareFileSystemError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.error {
            return None;
        }

        if self.next_offset >= self.data.len() {
            return None;
        }

        let result = Section::new_from_buffer(&self.data[self.next_offset..]);
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
