//! Serializable HOB Definitions and Utilities
//!
//! Defines serializable representations of various HOB types and utilities for parsing these HOBs.
//! These representations can be serialized to and deserialized from JSON format using `serde`.
//! For more information on the standard HOB format, see `hob.rs`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::cmp::Ordering;

use crate::pi::hob::Hob;
use crate::pi::serializable::hex_format;
use crate::pi::{serializable::Interval, serializable::format_guid};
use alloc::string::String;
use serde::{Deserialize, Serialize};

/// Serializable representation of the different HOB types.
/// For more information on the usage and representation of these HOBs, see `hob.rs`.
///
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HobSerDe {
    Handoff {
        version: u32,
        #[serde(with = "hex_format")]
        memory_top: u64,
        #[serde(with = "hex_format")]
        memory_bottom: u64,
        #[serde(with = "hex_format")]
        free_memory_top: u64,
        #[serde(with = "hex_format")]
        free_memory_bottom: u64,
        #[serde(with = "hex_format")]
        end_of_hob_list: u64,
    },
    MemoryAllocation {
        alloc_descriptor: MemAllocDescriptorSerDe,
    },
    ResourceDescriptor(ResourceDescriptorSerDe),
    ResourceDescriptorV2 {
        v1: ResourceDescriptorSerDe,
        attributes: u64,
    },
    GuidExtension {
        name: String,
    },
    FirmwareVolume {
        #[serde(with = "hex_format")]
        base_address: u64,
        length: u64,
    },
    Cpu {
        size_of_memory_space: u8,
        size_of_io_space: u8,
    },
    UnknownHob,
}

/// Serializable representation of the memory allocation descriptor inside a Memory Allocation HOB.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MemAllocDescriptorSerDe {
    /// Name (as a GUID string).
    pub name: String,
    /// Start address of the memory region.
    #[serde(with = "hex_format")]
    pub memory_base_address: u64,
    /// Length of the memory region in bytes.
    pub memory_length: u64,
    /// Type of memory (as defined in `r_efi::System::MemoryType`).
    pub memory_type: u32,
}

impl Interval for MemAllocDescriptorSerDe {
    fn start(&self) -> u64 {
        self.memory_base_address
    }

    fn end(&self) -> u64 {
        self.memory_base_address + self.memory_length
    }

    /// Merge two memory descriptors into one(including non overlapping
    /// intervals).
    fn merge(&self, other: &Self) -> Self {
        Self {
            name: self.name.clone(),
            memory_type: self.memory_type,
            memory_base_address: core::cmp::min(self.start(), other.start()),
            memory_length: core::cmp::max(self.end(), other.end()) - core::cmp::min(self.start(), other.start()),
        }
    }
}

/// Serializable representation of the resource descriptor inside a Resource Descriptor HOB.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ResourceDescriptorSerDe {
    /// Owner (as a GUID string).
    pub owner: String,
    /// Type of resource (as defined in `EFI_RESOURCE_TYPE`).
    pub resource_type: u32,
    /// Attributes of the resource in hex format (as defined in `EFI_RESOURCE_ATTRIBUTE_TYPE`).
    #[serde(with = "hex_format")]
    pub resource_attribute: u32,
    /// Start address of the resource.
    #[serde(with = "hex_format")]
    pub physical_start: u64,
    /// Length of the resource in bytes.
    #[serde(with = "hex_format")]
    pub resource_length: u64,
}

impl Interval for ResourceDescriptorSerDe {
    fn start(&self) -> u64 {
        self.physical_start
    }

    fn end(&self) -> u64 {
        self.physical_start + self.resource_length
    }

    /// Merge two resource descriptors into one(including non overlapping
    /// intervals).
    fn merge(&self, other: &Self) -> Self {
        Self {
            owner: self.owner.clone(),
            resource_type: self.resource_type,
            resource_attribute: self.resource_attribute,
            physical_start: core::cmp::min(self.start(), other.start()),
            resource_length: core::cmp::max(self.end(), other.end()) - core::cmp::min(self.start(), other.start()),
        }
    }
}

impl Ord for ResourceDescriptorSerDe {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.physical_start.cmp(&other.physical_start) {
            Ordering::Equal => self.resource_length.cmp(&other.resource_length),
            other => other,
        }
    }
}

impl PartialOrd for ResourceDescriptorSerDe {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl From<&Hob<'_>> for HobSerDe {
    fn from(hob: &Hob) -> Self {
        match hob {
            Hob::Handoff(handoff) => Self::Handoff {
                version: handoff.version,
                memory_top: handoff.memory_top,
                memory_bottom: handoff.memory_bottom,
                free_memory_top: handoff.free_memory_top,
                free_memory_bottom: handoff.free_memory_bottom,
                end_of_hob_list: handoff.end_of_hob_list,
            },
            Hob::MemoryAllocation(mem_alloc) => Self::MemoryAllocation {
                alloc_descriptor: MemAllocDescriptorSerDe {
                    name: format_guid(mem_alloc.alloc_descriptor.name),
                    memory_base_address: mem_alloc.alloc_descriptor.memory_base_address,
                    memory_length: mem_alloc.alloc_descriptor.memory_length,
                    memory_type: mem_alloc.alloc_descriptor.memory_type,
                },
            },
            Hob::ResourceDescriptor(resource_desc) => Self::ResourceDescriptor(ResourceDescriptorSerDe {
                owner: format_guid(resource_desc.owner),
                resource_type: resource_desc.resource_type,
                resource_attribute: resource_desc.resource_attribute,
                physical_start: resource_desc.physical_start,
                resource_length: resource_desc.resource_length,
            }),
            Hob::ResourceDescriptorV2(resource_desc2) => Self::ResourceDescriptorV2 {
                v1: ResourceDescriptorSerDe {
                    owner: format_guid(resource_desc2.v1.owner),
                    resource_type: resource_desc2.v1.resource_type,
                    resource_attribute: resource_desc2.v1.resource_attribute,
                    physical_start: resource_desc2.v1.physical_start,
                    resource_length: resource_desc2.v1.resource_length,
                },
                attributes: resource_desc2.attributes,
            },
            Hob::GuidHob(guid_ext, _) => Self::GuidExtension { name: format_guid(guid_ext.name) },
            Hob::FirmwareVolume(fv) => Self::FirmwareVolume { base_address: fv.base_address, length: fv.length },
            Hob::Cpu(cpu) => {
                Self::Cpu { size_of_memory_space: cpu.size_of_memory_space, size_of_io_space: cpu.size_of_io_space }
            }
            _ => Self::UnknownHob {},
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::pi::{BootMode, hob};

    use super::*;
    use serde_json::{from_str, to_string_pretty};

    #[test]
    fn test_hoblist_deserialization() {
        let json_data = r#"
            [
                {
                    "type": "handoff",
                    "version": 1,
                    "memory_top": "0xDEADCFEE",
                    "memory_bottom": "0xDEADBEEF",
                    "free_memory_top": "0x100000",
                    "free_memory_bottom": "0x10000",
                    "end_of_hob_list": "0xFEEDFACE"
                },
                {
                    "type": "memory_allocation",
                    "alloc_descriptor": {
                    "name": "123e4567-e89b-12d3-a456-426614174000",
                    "memory_base_address": "0x1000",
                    "memory_length": 12345678,
                    "memory_type": 0
                    }
                },
                {
                    "type": "resource_descriptor",
                    "owner": "123e4567-e89b-12d3-a456-426614174000",
                    "resource_type": 1,
                    "resource_attribute": "0x2",
                    "physical_start": "0x2000",
                    "resource_length": "0x4000"
                },
                {
                    "type": "resource_descriptor_v2",
                    "v1": {
                    "owner": "123e4567-e89b-12d3-a456-426614174000",
                    "resource_type": 1,
                    "resource_attribute": "0x2",
                    "physical_start": "0x2000",
                    "resource_length": "0x4000"
                    },
                    "attributes": 42
                },
                {
                    "type": "guid_extension",
                    "name": "123e4567-e89b-12d3-a456-426614174000"
                },
                {
                    "type": "firmware_volume",
                    "base_address": "0x10000",
                    "length": 987654321
                },
                {
                    "type": "cpu",
                    "size_of_memory_space": 48,
                    "size_of_io_space": 16
                },
                {
                    "type": "unknown_hob"
                }
            ]
        "#;

        let hob_list: Vec<HobSerDe> = from_str(json_data).expect("Failed to deserialize");

        assert_eq!(hob_list.len(), 8);
        if let HobSerDe::Handoff {
            version,
            memory_top,
            memory_bottom,
            free_memory_top,
            free_memory_bottom,
            end_of_hob_list,
        } = &hob_list[0]
        {
            assert_eq!(*version, 1);
            assert_eq!(*memory_top, 3735932910);
            assert_eq!(*memory_bottom, 3735928559);
            assert_eq!(*free_memory_top, 1048576);
            assert_eq!(*free_memory_bottom, 65536);
            assert_eq!(*end_of_hob_list, 4277009102);
        } else {
            panic!("First element is not a Handoff HOB");
        }

        if let HobSerDe::MemoryAllocation { alloc_descriptor } = &hob_list[1] {
            assert_eq!(alloc_descriptor.name, "123e4567-e89b-12d3-a456-426614174000");
            assert_eq!(alloc_descriptor.memory_base_address, 4096);
            assert_eq!(alloc_descriptor.memory_length, 12345678);
            assert_eq!(alloc_descriptor.memory_type, 0);
        } else {
            panic!("Second element is not a MemoryAllocation HOB");
        }

        if let HobSerDe::ResourceDescriptor(resource_desc) = &hob_list[2] {
            assert_eq!(resource_desc.owner, "123e4567-e89b-12d3-a456-426614174000");
            assert_eq!(resource_desc.resource_type, 1);
            assert_eq!(resource_desc.resource_attribute, 2);
            assert_eq!(resource_desc.physical_start, 8192);
            assert_eq!(resource_desc.resource_length, 16384);
        } else {
            panic!("Third element is not a ResourceDescriptor HOB");
        }

        if let HobSerDe::ResourceDescriptorV2 { v1, attributes } = &hob_list[3] {
            assert_eq!(v1.owner, "123e4567-e89b-12d3-a456-426614174000");
            assert_eq!(v1.resource_type, 1);
            assert_eq!(v1.resource_attribute, 2);
            assert_eq!(v1.physical_start, 8192);
            assert_eq!(v1.resource_length, 16384);
            assert_eq!(*attributes, 42);
        } else {
            panic!("Fourth element is not a ResourceDescriptorV2 HOB");
        }

        if let HobSerDe::GuidExtension { name } = &hob_list[4] {
            assert_eq!(name, "123e4567-e89b-12d3-a456-426614174000");
        } else {
            panic!("Fifth element is not a GuidExtension HOB");
        }

        if let HobSerDe::FirmwareVolume { base_address, length } = &hob_list[5] {
            assert_eq!(*base_address, 65536);
            assert_eq!(*length, 987654321);
        } else {
            panic!("Sixth element is not a FirmwareVolume HOB");
        }

        if let HobSerDe::Cpu { size_of_memory_space, size_of_io_space } = &hob_list[6] {
            assert_eq!(*size_of_memory_space, 48);
            assert_eq!(*size_of_io_space, 16);
        } else {
            panic!("Seventh element is not a CPU HOB");
        }
    }

    #[test]
    fn test_hoblist_serialization() {
        let header = hob::header::Hob {
            r#type: hob::HANDOFF,
            length: size_of::<hob::PhaseHandoffInformationTable>() as u16,
            reserved: 0,
        };
        let handoff_hob = hob::PhaseHandoffInformationTable {
            header,
            version: 0x00010000,
            boot_mode: BootMode::BootWithFullConfiguration,
            memory_top: 0xdeadc0de,
            memory_bottom: 0xdeadbeef,
            free_memory_top: 104,
            free_memory_bottom: 255,
            end_of_hob_list: 0xdeaddeadc0dec0de,
        };

        let header = hob::header::Hob {
            r#type: hob::MEMORY_ALLOCATION,
            length: size_of::<hob::MemoryAllocation>() as u16,
            reserved: 0,
        };
        let alloc_descriptor = hob::header::MemoryAllocation {
            name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            memory_base_address: 0,
            memory_length: 0x0123456789abcdef,
            memory_type: 0,
            reserved: [0; 4],
        };
        let memory_alloc_hob = hob::MemoryAllocation { header, alloc_descriptor };

        let header = hob::header::Hob {
            r#type: hob::RESOURCE_DESCRIPTOR,
            length: size_of::<hob::ResourceDescriptor>() as u16,
            reserved: 0,
        };
        let resource_desc_hob = hob::ResourceDescriptor {
            header,
            owner: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0,
            resource_length: 0x0123456789abcdef,
        };

        let mut v1 = hob::ResourceDescriptor {
            header,
            owner: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            resource_type: hob::EFI_RESOURCE_SYSTEM_MEMORY,
            resource_attribute: hob::EFI_RESOURCE_ATTRIBUTE_PRESENT,
            physical_start: 0,
            resource_length: 0x0123456789abcdef,
        };
        v1.header.r#type = hob::RESOURCE_DESCRIPTOR2;
        v1.header.length = size_of::<hob::ResourceDescriptorV2>() as u16;
        let resource_desc2_hob = hob::ResourceDescriptorV2 { v1, attributes: 8 };

        let data = [1_u8, 2, 3, 4, 5, 6, 7, 8];
        let guid_hob = (
            hob::GuidHob {
                header: hob::header::Hob {
                    r#type: hob::GUID_EXTENSION,
                    length: (size_of::<hob::GuidHob>() + data.len()) as u16,
                    reserved: 0,
                },
                name: r_efi::efi::Guid::from_fields(1, 2, 3, 4, 5, &[6, 7, 8, 9, 10, 11]),
            },
            data,
        );

        let header = hob::header::Hob { r#type: hob::FV, length: size_of::<hob::FirmwareVolume>() as u16, reserved: 0 };
        let fv_hob = hob::FirmwareVolume { header, base_address: 0, length: 0x0123456789abcdef };

        let header = hob::header::Hob { r#type: hob::CPU, length: size_of::<hob::Cpu>() as u16, reserved: 0 };
        let cpu_hob = hob::Cpu { header, size_of_memory_space: 0, size_of_io_space: 0, reserved: [0; 6] };

        let hob_list = vec![
            Hob::Handoff(&handoff_hob),
            Hob::ResourceDescriptor(&resource_desc_hob),
            Hob::MemoryAllocation(&memory_alloc_hob),
            Hob::ResourceDescriptor(&resource_desc_hob),
            Hob::ResourceDescriptorV2(&resource_desc2_hob),
            Hob::GuidHob(&guid_hob.0, &data),
            Hob::FirmwareVolume(&fv_hob),
            Hob::Cpu(&cpu_hob),
        ];

        let serializable_list = hob_list.iter().map(HobSerDe::from).collect::<Vec<HobSerDe>>();
        let json = to_string_pretty(&serializable_list).expect("Serialization failed");

        assert!(json.contains(r#""type": "handoff""#), "Handoff HOB missing");
        assert!(json.contains(r#""memory_top": "0xdeadc0de""#), "Memory top value incorrect");
        assert!(json.contains(r#""memory_bottom": "0xdeadbeef""#), "Memory bottom value incorrect");

        assert!(json.contains(r#""type": "memory_allocation""#), "Memory Allocation HOB missing");
        assert!(json.contains(r#""memory_length": 81985529216486895"#), "Memory length incorrect");

        assert!(json.contains(r#""type": "resource_descriptor""#), "Resource Descriptor HOB missing");
        assert!(json.contains(r#""physical_start": "0x0""#), "Physical start missing");

        assert!(json.contains(r#""type": "resource_descriptor_v2""#), "Resource Descriptor V2 missing");
        assert!(json.contains(r#""attributes": 8"#), "Resource Descriptor V2 attributes incorrect");

        assert!(json.contains(r#""type": "guid_extension""#), "GUID Extension HOB missing");

        assert!(json.contains(r#""type": "firmware_volume""#), "Firmware Volume HOB missing");
        assert!(json.contains(r#""base_address": "0x0""#), "Firmware Volume base address incorrect");
        assert!(json.contains(r#""length": 81985529216486895"#), "Firmware Volume length incorrect");

        assert!(json.contains(r#""type": "cpu""#), "CPU HOB missing");
        assert!(json.contains(r#""size_of_memory_space": 0"#), "CPU memory space size incorrect");
        assert!(json.contains(r#""size_of_io_space": 0"#), "CPU IO space size incorrect");
    }
}
