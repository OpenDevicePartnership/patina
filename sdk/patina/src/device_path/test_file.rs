// FV related device path nodes.

use core::mem;
use r_efi::efi;

#[repr(C)]
pub struct MemMapDevicePath {
    pub header: efi::protocols::device_path::Protocol,
    pub memory_type: u32,
    pub starting_address: u64,
    pub ending_address: u64,
}

#[repr(C)]
pub struct FvMemMapDevicePath {
    pub mem_map_device_path: MemMapDevicePath,
    pub end_dev_path: efi::protocols::device_path::End,
}

#[repr(C)]
pub struct MediaFwVolDevicePath {
    pub header: efi::protocols::device_path::Protocol,
    pub name: efi::Guid,
}

#[repr(C)]
pub struct FvPiWgDevicePath {
    pub fv_dev_path: MediaFwVolDevicePath,
    pub end_dev_path: efi::protocols::device_path::End,
}

impl FvPiWgDevicePath {
    // instantiate a new FvPiWgDevicePath for a Firmware Volume
    pub fn new_fv(fv_name: efi::Guid) -> Self {
        Self::new_worker(fv_name, efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_VOLUME)
    }
    // instantiate a new FvPiWgDevicePath for a Firmware File
    pub fn new_file(file_name: efi::Guid) -> Self {
        Self::new_worker(file_name, efi::protocols::device_path::Media::SUBTYPE_PIWG_FIRMWARE_FILE)
    }
    // instantiate a new FvPiWgDevicePath with the given sub-type
    pub fn new_worker(name: efi::Guid, sub_type: u8) -> Self {
        FvPiWgDevicePath {
            fv_dev_path: MediaFwVolDevicePath {
                header: efi::protocols::device_path::Protocol {
                    r#type: efi::protocols::device_path::TYPE_MEDIA,
                    sub_type,
                    length: [
                        (mem::size_of::<MediaFwVolDevicePath>() & 0xff) as u8,
                        ((mem::size_of::<MediaFwVolDevicePath>() >> 8) & 0xff) as u8,
                    ],
                },
                name,
            },
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
