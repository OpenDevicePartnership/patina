use alloc::vec::Vec;
use core::{
    fmt, iter, mem, ptr,
    slice::{self, from_raw_parts},
};
use patina_sdk::base::align_up;
use r_efi::efi;

use mu_pi::fw_fs::{
    ffs::{self, file},
    fv::{self, BlockMapEntry},
    fvb,
};

use crate::{
    file::{File, FileRef},
    section::SectionExtractor,
    FirmwareFileSystemError,
};

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
    /// Caller must ensure that base_address is the address of the start of a firmware volume.
    /// Caller must ensure that the lifetime of the buffer at base_address is longer than the
    /// returned VolumeRef.
    ///
    pub unsafe fn new_from_address(base_address: u64) -> Result<Self, FirmwareFileSystemError> {
        let fv_header = ptr::read_unaligned(base_address as *const fv::Header);
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

    pub fn ext_header(&self) -> Option<(fv::ExtHeader, Vec<u8>)> {
        self.ext_header.map(|ext_header| {
            let ext_header_data_start = self.fv_header.ext_header_offset as usize + mem::size_of_val(&ext_header);
            let ext_header_end = ext_header_data_start + ext_header.ext_header_size as usize;
            let header_data = self.data[ext_header_data_start..ext_header_end].to_vec();
            (ext_header, header_data)
        })
    }

    pub fn fv_name(&self) -> Option<efi::Guid> {
        self.ext_header().map(|x| x.0.fv_name)
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
        FileRefIter::new(&self.data[self.content_offset..], self.erase_byte()).filter(|x| {
            //Per PI spec 1.8A, V3, section 2.1.4.1.8: "Standard firmware file system services will not return the
            //handle of any PAD files, nor will they permit explicit creation of such files."
            //Pad files are ignored on read, and will be inserted on serialziation as needed to honor alignment
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

pub struct Volume {
    file_system_guid: efi::Guid,
    attributes: fvb::attributes::EfiFvbAttributes2,
    ext_header: Option<(fv::ExtHeader, Vec<u8>)>,
    block_map: Vec<BlockMapEntry>,
    files: Vec<File>,
}

impl Volume {
    pub fn new(block_map: Vec<BlockMapEntry>) -> Self {
        Self {
            file_system_guid: ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID,
            attributes: 0,
            ext_header: None,
            block_map,
            files: Vec::new(),
        }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, FirmwareFileSystemError> {
        let pad_byte =
            if (self.attributes & fvb::attributes::raw::fvb2::ERASE_POLARITY) != 0 { 0xffu8 } else { 0x00u8 };

        let large_file_support = self.file_system_guid == ffs::guid::EFI_FIRMWARE_FILE_SYSTEM3_GUID;

        //Serialize the file list into a content vector.
        let mut content = Vec::new();
        for file in &self.files {
            let file_buffer = &file.serialize()?;

            // Check if the file is too big for the filesystem format.
            if file_buffer.len() >= fv::FFS_V2_MAX_FILE_SIZE && !large_file_support {
                Err(FirmwareFileSystemError::Unsupported)?;
            }
            content.extend_from_slice(file_buffer);

            //pad to next 8-byte aligned length, since files start at 8-byte aligned offsets.
            let pad_length = 8 - (content.len() % 8);

            content.extend(iter::repeat(pad_byte).take(pad_length));
        }

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
            fv_buffer
                .extend_from_slice(unsafe { from_raw_parts(&raw const block as *const u8, mem::size_of_val(block)) });
        }

        // add the ext_header, if present
        let ext_header_offset = if let Some((ext_header, data)) = &self.ext_header {
            let offset = fv_buffer.len();

            fv_buffer.extend_from_slice(unsafe {
                from_raw_parts(&raw const ext_header as *const u8, mem::size_of_val(ext_header))
            });

            fv_buffer.extend(data);

            offset
        } else {
            0
        };

        let header_len = fv_buffer.len();
        // add padding to ensure first file is 8-byte aligned.
        let padding_len = 8 - (fv_buffer.len() % 8);
        fv_buffer.extend(iter::repeat(pad_byte).take(padding_len));

        //add content
        fv_buffer.extend(content);

        // calculate/patch the various header fields that need knowledge of buffer.
        fv_header.fv_length = fv_buffer.len().try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
        fv_header.header_length = header_len.try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
        fv_header.ext_header_offset =
            ext_header_offset.try_into().map_err(|_| FirmwareFileSystemError::InvalidHeader)?;
        let checksum = fv_buffer[..header_len]
            .chunks_exact(2)
            .fold(0u16, |sum, value| sum.wrapping_add(u16::from_le_bytes(value.try_into().unwrap())));
        fv_header.checksum = 0u16.wrapping_sub(checksum);

        //re-write the updated fv_header into the front of the fv_buffer.
        fv_buffer[..mem::size_of_val(&fv_header)]
            .copy_from_slice(unsafe { from_raw_parts(&raw mut fv_header as *mut u8, mem::size_of_val(&fv_header)) });

        Ok(fv_buffer)
    }
}

impl TryFrom<VolumeRef<'_>> for Volume {
    type Error = FirmwareFileSystemError;

    fn try_from(src: VolumeRef<'_>) -> Result<Self, Self::Error> {
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
        })
    }
}

impl TryFrom<(VolumeRef<'_>, &dyn SectionExtractor)> for Volume {
    type Error = FirmwareFileSystemError;

    fn try_from(src: (VolumeRef<'_>, &dyn SectionExtractor)) -> Result<Self, Self::Error> {
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
        })
    }
}

#[cfg(test)]
mod test {
    use core::{mem, sync::atomic::AtomicBool};
    use log::{self, Level, LevelFilter, Metadata, Record};
    use mu_pi::fw_fs::{self, ffs, fv};
    use r_efi::efi;
    use serde::Deserialize;
    use std::{
        collections::HashMap,
        env,
        error::Error,
        fs::{self, File},
        path::Path,
    };
    use uuid::Uuid;

    use crate::{
        section::{Section, SectionExtractor, SectionMetaData},
        volume::{Volume, VolumeRef},
        FirmwareFileSystemError,
    };

    #[derive(Debug, Deserialize)]
    struct TargetValues {
        total_number_of_files: u32,
        files_to_test: HashMap<String, FfsFileTargetValues>,
    }

    #[derive(Debug, Deserialize)]
    struct FfsFileTargetValues {
        file_type: u8,
        attributes: u8,
        size: usize,
        number_of_sections: usize,
        sections: HashMap<usize, FfsSectionTargetValues>,
    }

    #[derive(Debug, Deserialize)]
    struct FfsSectionTargetValues {
        section_type: Option<ffs::section::EfiSectionType>,
        size: usize,
        text: Option<String>,
    }

    struct NullExtractror {}
    impl SectionExtractor for NullExtractror {
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

    fn test_firmware_volume_worker(
        fv: VolumeRef,
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

    #[test]
    fn test_firmware_volume() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv = VolumeRef::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("DXEFV_expected_values.yml"))?)?;

        test_firmware_volume_worker(fv, expected_values, &NullExtractror {})
    }

    #[test]
    fn test_giant_firmware_volume() -> Result<(), Box<dyn Error>> {
        set_logger();
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let fv_bytes = fs::read(root.join("GIGANTOR.Fv"))?;
        let fv = VolumeRef::new(&fv_bytes).unwrap();

        let expected_values =
            serde_yaml::from_reader::<File, TargetValues>(File::open(root.join("GIGANTOR_expected_values.yml"))?)?;

        test_firmware_volume_worker(fv, expected_values, &NullExtractror {})
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
                let SectionMetaData::GuidDefined(metadata, _, _) = section.metadata() else {
                    panic!("Unexpected section metadata");
                };
                assert_eq!(metadata.section_definition_guid, fw_fs::guid::BROTLI_SECTION);
                self.invoked.store(true, core::sync::atomic::Ordering::SeqCst);
                Err(FirmwareFileSystemError::Unsupported)
            }
        }

        let test_extractor = TestExtractor { invoked: AtomicBool::new(false) };

        let fv = VolumeRef::new(&fv_bytes).unwrap();

        test_firmware_volume_worker(fv, expected_values, &test_extractor)?;

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
            assert_eq!((&(*a_ptr).block_map).as_ptr(), a_ptr.offset(1) as *const fv::BlockMapEntry);
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
        assert!(matches!(section.metadata(), SectionMetaData::Standard(ffs::section::raw_type::PE32, _)));

        let empty_compression: [u8; 0x11] =
            [0x11, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let section = Section::new_from_buffer(&empty_compression).unwrap();
        match section.metadata() {
            SectionMetaData::Compression(header, _) => {
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
        match section.metadata() {
            SectionMetaData::GuidDefined(header, guid_data, _) => {
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
        match section.metadata() {
            SectionMetaData::Version(version, _) => {
                assert_eq!(version.build_number, 0);
                assert_eq!(section.try_content_as_slice().unwrap(), &[0x31, 0x00, 0x2E, 0x00, 0x30, 0x00, 0x00, 0x00]);
            }
            otherwise_bad => panic!("invalid section: {:x?}", otherwise_bad),
        }

        let empty_freeform_subtype: [u8; 024] = [
            0x18, 0x00, 0x00, 0x18, //Header
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, //GUID
            0x04, 0x15, 0x19, 0x80, //Data
        ];
        let section = Section::new_from_buffer(&empty_freeform_subtype).unwrap();
        match section.metadata() {
            SectionMetaData::FreeFormSubtypeGuid(ffst_header, _) => {
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
        let root = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("test_resources");

        let original_fv_bytes = fs::read(root.join("DXEFV.Fv"))?;
        let fv_ref = VolumeRef::new(&original_fv_bytes).map_err(stringify)?;

        let _fv: Volume = fv_ref.try_into().map_err(stringify)?;

        Ok(())
    }
}
