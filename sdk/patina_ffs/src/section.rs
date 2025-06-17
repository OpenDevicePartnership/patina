use alloc::{boxed::Box, vec, vec::Vec};
use mu_pi::fw_fs::{ffs::section, EfiSectionType, FfsSectionHeader, FfsSectionRawType};

use core::{iter, mem};

use crate::FirmwareFileSystemError;

pub trait SectionExtractor {
    fn extract(&self, section: &Section) -> Result<Vec<Section>, FirmwareFileSystemError>;
}

pub trait SectionComposer {
    fn compose(&self, section: &mut Section) -> Result<(), FirmwareFileSystemError>;
}

#[derive(Debug, Clone)]
pub enum SectionMetaData {
    Standard(EfiSectionType),
    Compression(FfsSectionHeader::Compression),
    GuidDefined(FfsSectionHeader::GuidDefined, Vec<u8>),
    Version(FfsSectionHeader::Version),
    FreeFormSubtypeGuid(FfsSectionHeader::FreeformSubtypeGuid),
}
#[derive(Clone)]
pub struct Section {
    meta: SectionMetaData,
    data: Vec<u8>,
    content_offset: usize,

    subsections: Vec<Section>,
    composed: bool,
    extracted: bool,
}

impl Section {
    pub fn new(buffer: &[u8]) -> Result<Self, FirmwareFileSystemError> {
        // Verify that the buffer has enough storage for a section header.
        if buffer.len() < mem::size_of::<section::Header>() {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // Safety: buffer is large enough to contain the header, so can cast it.
        let section_header = unsafe { &*(buffer.as_ptr() as *const section::Header) };

        // Determine section size and start of section content
        let (section_size, section_data_offset) = {
            if section_header.size.iter().all(|&x| x == 0xff) {
                // size field is all 0xFF - this indicates extended header.
                let ext_header_size = mem::size_of::<section::header::CommonSectionHeaderExtended>();
                if buffer.len() < ext_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to contain extended header, so can cast it.
                let ext_header = unsafe { &*(buffer.as_ptr() as *const section::header::CommonSectionHeaderExtended) };
                (ext_header.extended_size as usize, ext_header_size)
            } else {
                //standard header.
                let mut size = vec![0x00u8; 4];
                size[0..2].copy_from_slice(&section_header.size);
                let size = u32::from_le_bytes(size.try_into().unwrap()) as usize;
                (size, core::mem::size_of::<section::Header>())
            }
        };

        // Verify that the buffer has enough space for the entire section.
        if buffer.len() < section_size {
            Err(FirmwareFileSystemError::InvalidHeader)?;
        }

        // For spec-defined section types, validate the section-specific headers.
        let (content_offset, meta) = match section_header.section_type {
            FfsSectionRawType::encapsulated::COMPRESSION => {
                let compression_header_size = mem::size_of::<section::header::Compression>();
                // verify that the buffer is large enough to hold the compresion header.
                if buffer.len() < section_data_offset + compression_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the compression header.
                let compression_header =
                    unsafe { *(buffer[section_data_offset..].as_ptr() as *const section::header::Compression) };
                (section_data_offset + compression_header_size, SectionMetaData::Compression(compression_header))
            }
            FfsSectionRawType::encapsulated::GUID_DEFINED => {
                // verify that the buffer is large enough to hold the GuidDefined header.
                let guid_header_size = mem::size_of::<section::header::GuidDefined>();
                if buffer.len() < section_data_offset + guid_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the GuidDefined header.
                let guid_defined_header =
                    unsafe { *(buffer[section_data_offset..].as_ptr() as *const section::header::GuidDefined) };

                // Verify that buffer has enough storage for guid-specific fields.
                let data_offset = guid_defined_header.data_offset as usize;
                if buffer.len() < data_offset {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }

                let guid_specific_data = buffer[section_data_offset + guid_header_size..data_offset].to_vec();

                (data_offset, SectionMetaData::GuidDefined(guid_defined_header, guid_specific_data))
            }
            FfsSectionRawType::VERSION => {
                let version_header_size = mem::size_of::<section::header::Version>();
                // verify that the buffer is large enough to hold the Version header.
                if buffer.len() < section_data_offset + version_header_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the version header.
                let version_header =
                    unsafe { *(buffer[section_data_offset..].as_ptr() as *const section::header::Version) };
                (section_data_offset + version_header_size, SectionMetaData::Version(version_header))
            }
            FfsSectionRawType::FREEFORM_SUBTYPE_GUID => {
                // verify that the buffer is large enough to hold the FreeformSubtypeGuid header.
                let freeform_subtype_size = mem::size_of::<section::header::FreeformSubtypeGuid>();
                if buffer.len() < section_data_offset + freeform_subtype_size {
                    Err(FirmwareFileSystemError::InvalidHeader)?;
                }
                // Safety: buffer is large enough to hold the freeform header type
                let freeform_header =
                    unsafe { *(buffer[section_data_offset..].as_ptr() as *const section::header::FreeformSubtypeGuid) };
                (section_data_offset + freeform_subtype_size, SectionMetaData::FreeFormSubtypeGuid(freeform_header))
            }
            _ => (section_data_offset, SectionMetaData::Standard(section_header.section_type)), //for all other types, the content immediately follows the standard header.
        };

        Ok(Section {
            meta,
            data: buffer.to_vec(),
            content_offset,
            subsections: Vec::new(),
            composed: true,
            extracted: false,
        })
    }

    pub fn metadata(&self) -> &SectionMetaData {
        &self.meta
    }

    pub fn try_as_slice(&self) -> Result<&[u8], FirmwareFileSystemError> {
        if self.composed {
            Ok(self.data.as_slice())
        } else {
            Err(FirmwareFileSystemError::NotComposed)
        }
    }

    pub fn try_content_as_slice(&self) -> Result<&[u8], FirmwareFileSystemError> {
        self.try_as_slice().map(|x| &x[self.content_offset..])
    }

    pub fn is_composed(&self) -> bool {
        self.composed
    }

    pub fn is_extracted(&self) -> bool {
        self.extracted
    }
}

impl<'a> Section {
    pub fn sections(&'a self) -> Box<dyn Iterator<Item = &'a Section> + 'a> {
        Box::new(iter::once(self).chain(self.subsections.iter().flat_map(|x| x.sections())))
    }
}
