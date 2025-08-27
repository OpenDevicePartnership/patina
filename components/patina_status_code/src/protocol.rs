use r_efi::efi;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EfiStatusCodeHeader {
    pub header_size: u16,
    pub size: u16,
    pub data_type: efi::Guid,
}

pub const REGISTER_UNREGISTER_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0x86212936, 0xe76, 0x41c8, 0xa0, 0x3a, &[0x2a, 0xf2, 0xfc, 0x1c, 0x39, 0xe2]);

// Register and unregister

// sherry: u probably need a global :(
