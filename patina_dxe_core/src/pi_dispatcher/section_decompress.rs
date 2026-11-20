//! A section extractor that provides UEFI decompression functionality with an additional custom extractor
//! implementation.
extern crate alloc;

use alloc::vec;
use patina::{
    pi::fw_fs::{self, ffs},
    uefi::decompress::{DecompressionAlgorithm, decompress_into_with_algo},
};
use patina_ffs::{
    FirmwareFileSystemError,
    section::{SectionExtractor, SectionHeader},
};

/// Section extractor that provides UEFI decompression, with an optional additional [`SectionExtractor`] implementation.
#[derive(Default)]
pub(super) struct CoreExtractor<E: SectionExtractor>(E);

impl<E: SectionExtractor> CoreExtractor<E> {
    /// Creates a new [`CoreExtractor`] with the specified additional extractor.
    pub const fn new(e: E) -> Self {
        Self(e)
    }

    /// Attempts to decompress the section using UEFI decompression algorithms.
    fn uefi_decompress_extract(
        section: &patina_ffs::section::Section,
    ) -> Result<vec::Vec<u8>, FirmwareFileSystemError> {
        let (src, algo) = match section.header() {
            SectionHeader::GuidDefined(guid_header, _, _)
                if guid_header.section_definition_guid == fw_fs::guid::TIANO_DECOMPRESS_SECTION_GUID =>
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

        let compressed_size = u32::from_le_bytes(
            src.get(0..4)
                .ok_or(FirmwareFileSystemError::DataCorrupt)?
                .try_into()
                .map_err(|_| FirmwareFileSystemError::DataCorrupt)?,
        ) as usize;
        if compressed_size > src.len() {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }

        // allocate a buffer to hold the decompressed data
        let decompressed_size = u32::from_le_bytes(
            src.get(4..8)
                .ok_or(FirmwareFileSystemError::DataCorrupt)?
                .try_into()
                .map_err(|_| FirmwareFileSystemError::DataCorrupt)?,
        ) as usize;
        // same limit patina_ffs_extractors applies to LZMA and Brotli sections.
        if decompressed_size > patina::SIZE_512MB {
            Err(FirmwareFileSystemError::DataCorrupt)?;
        }
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina_ffs::section::Section;
    use patina_ffs_extractors::NullSectionExtractor;

    #[test]
    fn uefi_decompress_limits_output_size() {
        let section = |size: u32| {
            let header = ffs::section::header::Compression {
                uncompressed_length: size,
                compression_type: ffs::section::header::STANDARD_COMPRESSION,
            };
            // UEFI compressed data header: compressed size, then decompressed size.
            let data = [0, size].iter().flat_map(|x| x.to_le_bytes()).collect();
            Section::new_from_header_with_data(SectionHeader::Compression(header, 8), data).unwrap()
        };
        assert_eq!(CoreExtractor::<NullSectionExtractor>::uefi_decompress_extract(&section(0)), Ok(vec![]));
        assert_eq!(
            CoreExtractor::<NullSectionExtractor>::uefi_decompress_extract(&section(u32::MAX)),
            Err(FirmwareFileSystemError::DataCorrupt)
        );
    }
}
