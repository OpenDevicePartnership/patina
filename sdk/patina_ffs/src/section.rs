use alloc::{boxed::Box, vec, vec::Vec};
use mu_pi::fw_fs::ffs::{self, section};
use patina_sdk::{base::align_up, boot_services::c_ptr::CPtr};

use core::{fmt, iter, mem, ptr, slice::from_raw_parts};

use crate::FirmwareFileSystemError;

pub trait SectionExtractor {
    fn extract(&self, section: &Section) -> Result<Vec<u8>, FirmwareFileSystemError>;
}

pub trait SectionComposer {
    fn compose(&self, section: &Section) -> Result<(SectionHeader, Vec<u8>), FirmwareFileSystemError>;
}

#[derive(Debug, Clone)]
pub enum SectionHeader {
    Pad(u32),
    Standard(section::EfiSectionType, u32),
    Compression(section::header::Compression, u32),
    GuidDefined(section::header::GuidDefined, Vec<u8>, u32),
    Version(section::header::Version, u32),
    FreeFormSubtypeGuid(section::header::FreeformSubtypeGuid, u32),
}

impl SectionHeader {
    pub fn content_offset(&self) -> usize {
        self.serialize().len()
    }

    pub fn total_section_size(&self) -> usize {
        self.content_offset() + self.content_size()
    }

    pub fn set_content_size(&mut self, size: usize) -> Result<(), FirmwareFileSystemError> {
        match self {
            SectionHeader::Pad(content_size)
            | SectionHeader::Standard(_, content_size)
            | SectionHeader::Compression(_, content_size)
            | SectionHeader::GuidDefined(_, _, content_size)
            | SectionHeader::Version(_, content_size)
            | SectionHeader::FreeFormSubtypeGuid(_, content_size) => {
                *content_size = size.try_into().map_err(|_| FirmwareFileSystemError::InvalidParameter)?;
                Ok(())
            }
        }
    }

    pub fn content_size(&self) -> usize {
        match self {
            SectionHeader::Pad(content_size)
            | SectionHeader::Standard(_, content_size)
            | SectionHeader::Compression(_, content_size)
            | SectionHeader::GuidDefined(_, _, content_size)
            | SectionHeader::Version(_, content_size)
            | SectionHeader::FreeFormSubtypeGuid(_, content_size) => *content_size as usize,
        }
    }

    pub fn section_type_raw(&self) -> u8 {
        match self {
            SectionHeader::Pad(_) => ffs::section::raw_type::FFS_PAD,
            SectionHeader::Standard(raw_type, _) => *raw_type,
            SectionHeader::Compression(_, _) => ffs::section::raw_type::encapsulated::COMPRESSION,
            SectionHeader::GuidDefined(_, _, _) => ffs::section::raw_type::encapsulated::GUID_DEFINED,
            SectionHeader::Version(_, _) => ffs::section::raw_type::VERSION,
            SectionHeader::FreeFormSubtypeGuid(_, _) => ffs::section::raw_type::FREEFORM_SUBTYPE_GUID,
        }
    }
    pub fn section_type(&self) -> Option<ffs::section::Type> {
        match self {
            SectionHeader::Pad(_) => None,
            SectionHeader::Standard(section_type_raw, _) => match *section_type_raw {
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
            SectionHeader::Compression(_, _) => Some(ffs::section::Type::Compression),
            SectionHeader::GuidDefined(_, _, _) => Some(ffs::section::Type::GuidDefined),
            SectionHeader::Version(_, _) => Some(ffs::section::Type::Version),
            SectionHeader::FreeFormSubtypeGuid(_, _) => Some(ffs::section::Type::FreeformSubtypeGuid),
        }
    }
    pub fn serialize(&self) -> Vec<u8> {
        let (header_data, content_size) = match self {
            SectionHeader::Pad(_content_size) => return Vec::new(),
            SectionHeader::Standard(_, content_size) => (vec![0u8; 0], *content_size),
            SectionHeader::Compression(compression, content_size) => {
                //safety: compression is repr(C)
                let compression_slice =
                    unsafe { from_raw_parts(compression.as_ptr() as *const u8, mem::size_of_val(compression)) };
                (compression_slice.to_vec(), *content_size)
            }
            SectionHeader::GuidDefined(guid_defined, items, context_size) => {
                //safety: guid_defined is repr(C)
                let mut guid_defined_vec = unsafe {
                    from_raw_parts(guid_defined.as_ptr() as *const u8, mem::size_of_val(guid_defined)).to_vec()
                };
                guid_defined_vec.extend(items);
                (guid_defined_vec, *context_size)
            }
            SectionHeader::Version(version, content_size) => {
                //safety: version is repr(C)
                let version_slice = unsafe { from_raw_parts(version.as_ptr() as *const u8, mem::size_of_val(version)) };
                (version_slice.to_vec(), *content_size)
            }
            SectionHeader::FreeFormSubtypeGuid(freeform_subtype_guid, content_size) => {
                //safety: freeform_subtype_guid is repr(C)
                let freeform_slice = unsafe {
                    from_raw_parts(freeform_subtype_guid.as_ptr() as *const u8, mem::size_of_val(freeform_subtype_guid))
                };
                (freeform_slice.to_vec(), *content_size)
            }
        };

        let mut section_header = ffs::section::Header { section_type: self.section_type_raw(), size: [0xffu8; 3] };

        let section_size = mem::size_of_val(&section_header) + header_data.len() + (content_size as usize);

        if section_size < 0x1000000 {
            section_header.size = (section_size as u32).to_le_bytes()[0..3].try_into().unwrap();
        }

        //safety: header is repr(C)
        let mut section_vec = unsafe {
            from_raw_parts(&raw const section_header as *const u8, mem::size_of_val(&section_header)).to_vec()
        };

        //add ext size if req.
        if section_size >= 0x1000000 {
            section_vec.extend((section_size as u32 + 4).to_le_bytes());
        }

        section_vec.extend(header_data);

        section_vec
    }
}

#[derive(Clone)]
struct LeafSectionData {
    data: Vec<u8>,
}

impl fmt::Debug for LeafSectionData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeafSectionData").field("data ({:#x} bytes)", &self.data.len()).finish()
    }
}

#[derive(Clone)]
struct EncapsulationSectionData {
    sub_sections: Vec<Section>,
    data: Vec<u8>,
    extracted: bool,
}

impl fmt::Debug for EncapsulationSectionData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncapsulationSectionData")
            .field("sub_sections", &self.sub_sections)
            .field("data ({:#x} bytes)", &self.data.len())
            .field("extracted", &self.extracted)
            .finish()
    }
}

#[derive(Clone, Debug)]
enum SectionData {
    Leaf(LeafSectionData),
    Encapsulation(EncapsulationSectionData),
}

#[derive(Debug, Clone)]
pub struct Section {
    header: SectionHeader,
    data: SectionData,
    dirty: bool,
}

impl Section {
    pub fn new_from_header_with_data(header: SectionHeader, data: Vec<u8>) -> Result<Self, FirmwareFileSystemError> {
        //Pad sections need special handling due to having no section header.
        if let SectionHeader::Pad(_) = header {
            Ok(Self { header, data: SectionData::Leaf(LeafSectionData { data }), dirty: false })
        } else {
            let mut buffer = header.serialize();
            buffer.extend(data);
            Self::new_from_buffer(&buffer)
        }
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
        let (header, content_offset) = match section_header.section_type {
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
                let content_size: u32 = (section_size - (section_data_offset + compression_header_size))
                    .try_into()
                    .map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
                (
                    SectionHeader::Compression(compression_header, content_size),
                    section_data_offset + compression_header_size,
                )
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
                let content_size: u32 =
                    (section_size - data_offset).try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
                (SectionHeader::GuidDefined(guid_defined_header, guid_specific_data, content_size), data_offset)
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
                let content_size: u32 = (section_size - (section_data_offset + version_header_size))
                    .try_into()
                    .map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
                (SectionHeader::Version(version_header, content_size), section_data_offset + version_header_size)
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
                let content_size: u32 = (section_size - (section_data_offset + freeform_subtype_size))
                    .try_into()
                    .map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
                (
                    SectionHeader::FreeFormSubtypeGuid(freeform_header, content_size),
                    section_data_offset + freeform_subtype_size,
                )
            }
            _ => {
                let content_size: u32 = (section_size - section_data_offset)
                    .try_into()
                    .map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
                (SectionHeader::Standard(section_header.section_type, content_size), section_data_offset)
                //for all other types, the content immediately follows the standard header.
            }
        };

        let section_data = match header {
            SectionHeader::Compression(_, _) | SectionHeader::GuidDefined(_, _, _) => {
                SectionData::Encapsulation(EncapsulationSectionData {
                    sub_sections: Vec::new(),
                    data: buffer[content_offset..section_size].to_vec(),
                    extracted: false,
                })
            }
            _ => SectionData::Leaf(LeafSectionData { data: buffer[content_offset..section_size].to_vec() }),
        };

        Ok(Section { header, data: section_data, dirty: false })
    }

    pub fn header(&self) -> &SectionHeader {
        &self.header
    }

    pub fn encapsulation(&self) -> bool {
        matches!(self.data, SectionData::Encapsulation(_))
    }

    pub fn dirty(&self) -> bool {
        if let SectionData::Encapsulation(data) = &self.data {
            if data.extracted {
                self.dirty || data.sub_sections.iter().any(|x| x.dirty())
            } else {
                self.dirty
            }
        } else {
            self.dirty
        }
    }

    pub fn size(&self) -> Result<usize, FirmwareFileSystemError> {
        if self.dirty() {
            Err(FirmwareFileSystemError::NotComposed)?;
        }
        Ok(self.header.total_section_size())
    }

    pub fn section_type_raw(&self) -> u8 {
        self.header.section_type_raw()
    }

    pub fn section_type(&self) -> Option<ffs::section::Type> {
        self.header.section_type()
    }

    pub fn set_section_data(&mut self, data: Vec<u8>) -> Result<(), FirmwareFileSystemError> {
        if let SectionData::Leaf(leaf) = &mut self.data {
            leaf.data = data;
            self.header.set_content_size(leaf.data.len())?;
            self.dirty = true;
            Ok(())
        } else {
            Err(FirmwareFileSystemError::NotLeaf)
        }
    }

    pub fn compose(&mut self, composer: &dyn SectionComposer) -> Result<(), FirmwareFileSystemError> {
        match &mut self.data {
            SectionData::Encapsulation(encapsulation) => {
                for section in encapsulation.sub_sections.iter_mut() {
                    section.compose(composer)?;
                }
            }
            SectionData::Leaf(_) => (),
        }

        let (header, content) = composer.compose(self)?;
        self.header = header;
        self.header.set_content_size(content.len())?;

        match &mut self.data {
            SectionData::Encapsulation(encapsulation) => {
                encapsulation.data = content;
            }
            SectionData::Leaf(leaf) => {
                leaf.data = content;
            }
        }

        self.dirty = false;
        Ok(())
    }

    pub fn extract(&mut self, extractor: &dyn SectionExtractor) -> Result<(), FirmwareFileSystemError> {
        match &self.data {
            SectionData::Encapsulation(x) if !x.extracted => (),
            _ => return Ok(()), //nothing to do.
        };

        let extracted_data = match extractor.extract(self) {
            Err(FirmwareFileSystemError::Unsupported) => Vec::new(),
            result => result?,
        };

        let mut sections: Vec<Section> =
            SectionIterator::new(&extracted_data).collect::<Result<Vec<_>, FirmwareFileSystemError>>()?;

        for section in sections.iter_mut() {
            section.extract(extractor)?;
        }

        match &mut self.data {
            SectionData::Encapsulation(encapsulation) => {
                encapsulation.sub_sections = sections;
                encapsulation.extracted = true;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    pub fn serialize(&self) -> Result<Vec<u8>, FirmwareFileSystemError> {
        if self.dirty() {
            Err(FirmwareFileSystemError::NotComposed)?;
        }
        let mut data = self.header.serialize();
        data.extend(match &self.data {
            SectionData::Encapsulation(encapsulation) => &encapsulation.data,
            SectionData::Leaf(leaf) => &leaf.data,
        });
        Ok(data)
    }

    pub fn try_content_as_slice(&self) -> Result<&[u8], FirmwareFileSystemError> {
        if self.dirty() {
            Err(FirmwareFileSystemError::NotComposed)?;
        }
        match &self.data {
            SectionData::Encapsulation(encapsulation) => Ok(&encapsulation.data),
            SectionData::Leaf(leaf) => Ok(&leaf.data),
        }
    }

    pub fn into_sections(self) -> impl Iterator<Item = Section> {
        let sub_sections = match &self.data {
            SectionData::Encapsulation(encapsulation) => encapsulation.sub_sections.clone(),
            SectionData::Leaf(_) => vec![],
        };
        iter::once(self).chain(sub_sections)
    }

    pub fn sections(&self) -> Box<dyn Iterator<Item = &Section> + '_> {
        match &self.data {
            SectionData::Encapsulation(encapsulation) => {
                let sub_sections = encapsulation.sub_sections.iter();
                Box::new(iter::once(self).chain(sub_sections))
            }
            SectionData::Leaf(_leaf) => Box::new(iter::once(self)),
        }
    }

    pub fn sub_sections_mut(&mut self) -> Box<dyn Iterator<Item = &mut Section> + '_> {
        match &mut self.data {
            SectionData::Encapsulation(encapsulation) => Box::new(encapsulation.sub_sections.iter_mut()),
            SectionData::Leaf(_leaf) => Box::new(iter::empty()),
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
