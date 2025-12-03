//! Serialization and deserialization logic for Firmware Volume (FV) structures.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use crate::pi::{
    fw_fs::{
        FfsSectionHeader::{NOT_COMPRESSED, STANDARD_COMPRESSION},
        FfsSectionType, FirmwareVolume, SectionMetaData,
        guid::{
            BROTLI_SECTION, CRC32_SECTION, LZMA_F86_SECTION, LZMA_PARALLEL_SECTION, LZMA_SECTION,
            TIANO_DECOMPRESS_SECTION,
        },
    },
    serializable::{format_guid, hex_format},
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use r_efi::efi;
use serde::{Deserialize, Serialize};

// This is the serialized version of the FV list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FirmwareVolumeSerDe {
    pub fv_name: String,
    #[serde(with = "hex_format")]
    pub fv_length: usize,
    #[serde(with = "hex_format")]
    pub fv_base_address: u64,
    #[serde(with = "hex_format")]
    pub fv_attributes: u32,
    pub files: Vec<FirmwareFileSerDe>,
}

// This is the serialized version of the file list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FirmwareFileSerDe {
    pub name: String, // GUID
    pub file_type: String,
    #[serde(with = "hex_format")]
    pub length: usize,
    // pub base_address: u64,
    #[serde(with = "hex_format")]
    pub attributes: u32,
    pub sections: Vec<FirmwareSectionSerDe>,
}

// This is the serialized version of the section list.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FirmwareSectionSerDe {
    pub section_type: String,
    #[serde(with = "hex_format")]
    pub length: usize,
    pub compression_type: String,
    // pub attributes: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pe_info: Option<PeHeaderInfo>,
}

// Serialized wrapper for PE-related fields.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Copy)]
pub struct PeHeaderInfo {
    pub section_alignment: u32,
    pub machine: u16,
    pub subsystem: u16,
}

impl From<FirmwareVolume<'_>> for FirmwareVolumeSerDe {
    fn from(fv: FirmwareVolume) -> Self {
        // Get the FV name, length, base address, and attributes
        let fv_name = format_guid(fv.fv_name().unwrap_or(efi::Guid::from_bytes(&[0; 16])));
        let fv_length = fv.size() as usize;
        let fv_attributes = fv.attributes();
        let files = fv
            .file_iter()
            .filter_map(|file| {
                // Iterate over the Files in the FV
                let Ok(file) = file else {
                    return None;
                };
                let file_name = format_guid(file.name());
                let file_length = file.size() as usize;
                let file_attributes = file.attributes_raw() as u32;
                let file_type =
                    file.file_type().map(|ft| format!("{:#x?}", ft)).unwrap_or_else(|| "Invalid".to_string());
                let sections = file
                    .section_iter()
                    .filter_map(|section| {
                        // Iterate over the section in file
                        let Ok(section) = section else {
                            return None;
                        };
                        let section_length = section.section_size();
                        let section_type_str = section
                            .section_type()
                            .map(|st| format!("{:#x?}", st))
                            .unwrap_or_else(|| "Invalid".to_string());
                        let section_compression_type = match section.meta_data() {
                            SectionMetaData::Compression(compression) => match compression.compression_type {
                                NOT_COMPRESSED => "uncompressed".to_string(),
                                STANDARD_COMPRESSION => "Standard Uefi compressed".to_string(),
                                _ => format!("{:#x?}", compression.compression_type),
                            },
                            SectionMetaData::GuidDefined(guid, _) => match guid.section_definition_guid {
                                BROTLI_SECTION => "Brotli Compressed".to_string(),
                                CRC32_SECTION => "CRC32 Compressed".to_string(),
                                LZMA_SECTION => "LZMA Compressed".to_string(),
                                LZMA_F86_SECTION => "LZMA F86 Compressed".to_string(),
                                LZMA_PARALLEL_SECTION => "LZMA Parallel Compressed".to_string(),
                                TIANO_DECOMPRESS_SECTION => "Tiano Compressed".to_string(),
                                _ => format_guid(guid.section_definition_guid),
                            },
                            _ => "uncompressed".to_string(),
                        };

                        if let Some(section_type) = section.section_type()
                            && section_type == FfsSectionType::Pe32
                        {
                            // If parsing fails or the header is missing PE data (in the coff.optional headers), we treat it as a non-PE section (skip the `pe_info`).
                            let pe = goblin::pe::PE::parse(section.section_data());
                            if let Ok(pe_parsed) = pe
                                && let Some(optional_header) = pe_parsed.header.optional_header
                            {
                                let alignment = optional_header.windows_fields.section_alignment;
                                let machine = pe_parsed.header.coff_header.machine;
                                let subsystem = optional_header.windows_fields.subsystem;
                                return Some(FirmwareSectionSerDe {
                                    section_type: section_type_str,
                                    length: section_length,
                                    compression_type: section_compression_type,
                                    pe_info: Some(PeHeaderInfo { section_alignment: alignment, machine, subsystem }),
                                });
                            }
                        }

                        Some(FirmwareSectionSerDe {
                            section_type: section_type_str,
                            length: section_length,
                            compression_type: section_compression_type,
                            pe_info: None,
                        })
                    })
                    .collect::<Vec<_>>();

                Some(FirmwareFileSerDe {
                    name: file_name,
                    length: file_length,
                    attributes: file_attributes,
                    sections,
                    file_type,
                })
            })
            .collect::<Vec<_>>();

        FirmwareVolumeSerDe { fv_name, fv_length, fv_attributes, files, fv_base_address: 0 /* filled outside */ }
    }
}
