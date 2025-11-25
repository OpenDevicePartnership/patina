//! A section extractor that provides UEFI decompression functionality with an additional custom extractor
//! implementation.
extern crate alloc;

use alloc::vec;
use mu_rust_helpers::uefi_decompress::{DecompressionAlgorithm, decompress_into_with_algo};
use patina::pi::fw_fs::{self, ffs};
use patina_ffs::{
    FirmwareFileSystemError,
    section::{SectionExtractor, SectionHeader},
};

/// Section extractor that provides UEFI decompression, with an optional additional [SectionExtractor] implementation.
#[derive(Default)]
pub(super) struct CoreExtractor<E: SectionExtractor>(E);

impl<E: SectionExtractor> CoreExtractor<E> {
    /// Creates a new [CoreExtractor] with the specified additional extractor.
    pub const fn new(e: E) -> Self {
        Self(e)
    }

    /// Attempts to decompress the section using UEFI decompression algorithms.
    fn uefi_decompress_extract(
        section: &patina_ffs::section::Section,
    ) -> Result<vec::Vec<u8>, FirmwareFileSystemError> {
        let (src, algo) = match section.header() {
            SectionHeader::GuidDefined(guid_header, _, _)
                if guid_header.section_definition_guid == fw_fs::guid::TIANO_DECOMPRESS_SECTION =>
            {
                (section.try_content_as_slice()?, DecompressionAlgorithm::TianoDecompress)
            }
            SectionHeader::Compression(compression_header, _) => {
                match compression_header.compression_type {
                    ffs::section::header::NOT_COMPRESSED => return Ok(section.try_content_as_slice()?.to_vec()), //not compressed, so just return section data
                    ffs::section::header::STANDARD_COMPRESSION => {
                        (section.try_content_as_slice()?, DecompressionAlgorithm::UefiDecompress)
                    }
                    _ => Err(FirmwareFileSystemError::Unsupported)?,
                }
            }
            _ => return Err(FirmwareFileSystemError::Unsupported),
        };

        //sanity check the src data
        if src.len() < 8 {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }

        let compressed_size =
            u32::from_le_bytes(src[0..4].try_into().map_err(|_| FirmwareFileSystemError::DataCorrupt)?) as usize;
        if compressed_size > src.len() {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }

        // allocate a buffer to hold the decompressed data
        let decompressed_size =
            u32::from_le_bytes(src[4..8].try_into().map_err(|_| FirmwareFileSystemError::DataCorrupt)?) as usize;
        let mut decompressed_buffer = vec![0u8; decompressed_size];

        // execute decompress
        decompress_into_with_algo(src, &mut decompressed_buffer, algo)
            .map_err(|_err| FirmwareFileSystemError::DataCorrupt)?;
        Ok(decompressed_buffer)
    }
}

impl<E: SectionExtractor> SectionExtractor for CoreExtractor<E> {
    fn extract(&self, section: &patina_ffs::section::Section) -> Result<vec::Vec<u8>, FirmwareFileSystemError> {
        match Self::uefi_decompress_extract(section) {
            Err(FirmwareFileSystemError::Unsupported) => (),
            Err(err) => return Err(err),
            Ok(buffer) => return Ok(buffer),
        }
        self.0.extract(section)
    }
}
