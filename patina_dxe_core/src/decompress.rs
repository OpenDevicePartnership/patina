//! A module for core UEFI decompression functionality.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
extern crate alloc;

use alloc::boxed::Box;
use patina::{
    boot_services::BootServices,
    component::{IntoComponent, Storage},
    error::EfiError,
    uefi_protocol::decompress,
};

use alloc::vec;

use mu_rust_helpers::uefi_decompress::{DecompressionAlgorithm, decompress_into_with_algo};
use patina::component::prelude::Service;
use patina_ffs::{
    FirmwareFileSystemError,
    section::{SectionExtractor, SectionHeader},
};
use patina_pi::fw_fs::{self, ffs};

/// Component to install the UEFI Decompress Protocol.
#[derive(IntoComponent, Default)]
pub(crate) struct DecompressProtocolInstaller;

impl DecompressProtocolInstaller {
    fn entry_point(self, storage: &mut Storage) -> patina::error::Result<()> {
        let protocol = Box::new(decompress::EfiDecompressProtocol::new());

        match storage.boot_services().install_protocol_interface(None, protocol) {
            Ok(_) => Ok(()),
            Err(err) => EfiError::status_to_result(err),
        }
    }
}

/// Section extractor that provides UEFI decompression, with an optional additional [SectionExtractor] implementation.
#[derive(Default)]
pub struct CoreExtractor(Option<Service<dyn SectionExtractor>>);

impl CoreExtractor {
    /// Creates a new [CoreExtractor] with no additional extractor.
    pub const fn new() -> Self {
        Self(None)
    }

    /// Sets an additional [SectionExtractor] to be used if UEFI decompression does not apply.
    pub fn set_extractor(&mut self, extractor: Service<dyn SectionExtractor>) -> &mut Self {
        self.0 = Some(extractor);
        self
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

impl SectionExtractor for CoreExtractor {
    fn extract(&self, section: &patina_ffs::section::Section) -> Result<vec::Vec<u8>, FirmwareFileSystemError> {
        match Self::uefi_decompress_extract(section) {
            Err(FirmwareFileSystemError::Unsupported) => (),
            Err(err) => return Err(err),
            Ok(buffer) => return Ok(buffer),
        }
        self.0.as_ref().map_or(Err(FirmwareFileSystemError::Unsupported), |extractor| extractor.extract(section))
    }
}
