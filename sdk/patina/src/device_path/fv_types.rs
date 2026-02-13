//! FV-related Device Path struct implementations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::mem;
use r_efi::efi;

/// Describes a memory-mapped device path node.
#[repr(C)]
pub struct MemMapDevicePath {
    /// Standard UEFI device path header.
    pub header: efi::protocols::device_path::Protocol,
    /// EFI_MEMORY_TYPE value.
    pub memory_type: u32,
    /// Starting address of the memory region.
    pub starting_address: u64,
    /// Ending address of the memory region.
    pub ending_address: u64,
}

impl MemMapDevicePath {
    /// Creates a new MemMapDevicePath with the given memory type and address range.
    pub fn new(memory_type: u32, starting_address: u64, ending_address: u64) -> Self {
        MemMapDevicePath {
            header: efi::protocols::device_path::Protocol {
                r#type: efi::protocols::device_path::TYPE_HARDWARE,
                sub_type: efi::protocols::device_path::Hardware::SUBTYPE_MMAP,
                length: [
                    (mem::size_of::<MemMapDevicePath>() & 0xff) as u8,
                    ((mem::size_of::<MemMapDevicePath>() >> 8) & 0xff) as u8,
                ],
            },
            memory_type,
            starting_address,
            ending_address,
        }
    }
}

/// Describes a memory-mapped firmware volume device path node. The memory type field is defined in the UEFI specification, and the starting and ending address fields specify the range of the firmware volume in memory.
#[repr(C)]
pub struct FvMemMapDevicePath {
    /// Memory-mapped device path node describing the firmware volume.
    pub mem_map_device_path: MemMapDevicePath,
    /// Device path terminator.
    pub end_dev_path: efi::protocols::device_path::End,
}

impl FvMemMapDevicePath {
    /// Creates a new FvMemMapDevicePath with the given memory type and address range.
    pub fn new(memory_type: u32, starting_address: u64, ending_address: u64) -> Self {
        FvMemMapDevicePath {
            mem_map_device_path: MemMapDevicePath::new(memory_type, starting_address, ending_address),
            end_dev_path: efi::protocols::device_path::End {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_END,
                    sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                    length: [
                        (mem::size_of::<efi::protocols::device_path::End>() & 0xff) as u8,
                        ((mem::size_of::<efi::protocols::device_path::End>() >> 8) & 0xff) as u8,
                    ],
                },
            },
        }
    }
}

/// Describes a firmware volume device path node.
#[repr(C)]
pub struct MediaFwVolDevicePath {
    /// Standard UEFI device path header with type MEDIA and sub-type SUBTYPE_PIWG_FIRMWARE_VOLUME.
    pub header: efi::protocols::device_path::Protocol,
    /// Firmware volume name.
    pub name: efi::Guid,
}

impl MediaFwVolDevicePath {
    /// Creates a new MediaFwVolDevicePath with the given firmware volume name.
    pub fn new(name: efi::Guid, subtype: MediaFwDevicePathSubtype) -> Self {
        MediaFwVolDevicePath {
            header: efi::protocols::device_path::Protocol {
                r#type: efi::protocols::device_path::TYPE_MEDIA,
                sub_type: subtype as u8,
                length: [
                    (mem::size_of::<MediaFwVolDevicePath>() & 0xff) as u8,
                    ((mem::size_of::<MediaFwVolDevicePath>() >> 8) & 0xff) as u8,
                ],
            },
            name,
        }
    }
}

/// Sub-types for Media Firmware Volume Device Paths, as defined in the UEFI specification.
/// Can either be an FV or a file path within an FV.
#[repr(u8)]
pub enum MediaFwDevicePathSubtype {
    /// Firmware volume device path node.
    FirmwareVolume = efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_VOLUME,
    /// Firmware file path device path node.
    FirmwareFile = efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE,
}

/// Describes a firmware volume or file path within a firmware volume, with a device path header and an end node.
#[repr(C)]
pub struct FvPiWgDevicePath {
    /// Firmware volume or file path device path node.
    pub fv_dev_path: MediaFwVolDevicePath,
    /// Device path terminator.
    pub end_dev_path: efi::protocols::device_path::End,
}

impl FvPiWgDevicePath {
    /// Instantiate a new FvPiWgDevicePath for a Firmware Volume.
    pub fn new_fv(fv_name: efi::Guid) -> Self {
        Self::new_worker(fv_name, MediaFwDevicePathSubtype::FirmwareVolume)
    }

    /// Instantiate a new FvPiWgDevicePath for a Firmware File.
    pub fn new_file(file_name: efi::Guid) -> Self {
        Self::new_worker(file_name, MediaFwDevicePathSubtype::FirmwareFile)
    }

    /// Instantiate a new FvPiWgDevicePath with the given sub-type.
    pub fn new_worker(name: efi::Guid, sub_type: MediaFwDevicePathSubtype) -> Self {
        FvPiWgDevicePath {
            fv_dev_path: MediaFwVolDevicePath::new(name, sub_type),
            end_dev_path: efi::protocols::device_path::End {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_END,
                    sub_type: efi::protocols::device_path::End::SUBTYPE_ENTIRE,
                    length: [
                        (mem::size_of::<efi::protocols::device_path::End>() & 0xff) as u8,
                        ((mem::size_of::<efi::protocols::device_path::End>() >> 8) & 0xff) as u8,
                    ],
                },
            },
        }
    }
}
