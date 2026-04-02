//! USB descriptor structures and constants.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

/// USB interface class for HID devices.
pub const CLASS_HID: u8 = 3;
/// USB interface subclass for boot devices.
pub const SUBCLASS_BOOT: u8 = 1;

/// HID boot protocol mode.
pub const BOOT_PROTOCOL: u8 = 0;
/// HID report protocol mode.
pub const REPORT_PROTOCOL: u8 = 1;

/// USB descriptor type for configuration.
pub const USB_DESC_TYPE_CONFIG: u8 = 2;
/// USB descriptor type for interface.
pub const USB_DESC_TYPE_INTERFACE: u8 = 4;
/// USB descriptor type for HID.
pub const USB_DESC_TYPE_HID: u8 = 0x21;
/// USB descriptor type for HID report.
pub const USB_DESC_TYPE_REPORT: u8 = 0x22;

/// USB endpoint transfer type mask.
pub const USB_ENDPOINT_XFER_TYPE_MASK: u8 = 0x03;
/// USB endpoint type: interrupt.
pub const USB_ENDPOINT_INTERRUPT: u8 = 0x03;
/// USB endpoint direction: IN.
pub const USB_ENDPOINT_DIR_IN: u8 = 0x80;

/// No USB error.
pub const EFI_USB_NOERROR: u32 = 0;
/// USB stall error.
pub const EFI_USB_ERR_STALL: u32 = 0x04;

/// Delay before re-submitting after a USB interrupt transfer error (100ms in 100ns units).
pub const EFI_USB_INTERRUPT_DELAY: u64 = 10_000_000;

/// USB HID class-specific request: GET_REPORT.
pub const USB_HID_GET_REPORT_REQUEST: u8 = 0x01;
/// USB HID class-specific request: SET_REPORT.
pub const USB_HID_SET_REPORT_REQUEST: u8 = 0x09;
/// USB HID class-specific request: SET_PROTOCOL.
pub const USB_HID_SET_PROTOCOL_REQUEST: u8 = 0x0B;

/// USB request type: class, interface, host-to-device.
pub const USB_REQ_TYPE_CLASS_INTERFACE_OUT: u8 = 0x21;
/// USB request type: class, interface, device-to-host.
pub const USB_REQ_TYPE_CLASS_INTERFACE_IN: u8 = 0xA1;
/// USB request type: standard, endpoint, host-to-device.
pub const USB_REQ_TYPE_STANDARD_ENDPOINT_OUT: u8 = 0x02;

/// USB standard request: CLEAR_FEATURE.
pub const USB_REQ_CLEAR_FEATURE: u8 = 0x01;
/// USB feature selector: ENDPOINT_HALT.
pub const USB_FEATURE_ENDPOINT_HALT: u16 = 0;

/// USB standard request: GET_DESCRIPTOR.
pub const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
/// USB request type: standard, device, device-to-host.
pub const USB_REQ_TYPE_STANDARD_DEVICE_IN: u8 = 0x80;

/// Timeout for USB control transfers (in milliseconds).
pub const USB_TRANSFER_TIMEOUT_MS: u32 = 3000;

/// USB interface descriptor.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct EfiUsbInterfaceDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub num_endpoints: u8,
    pub interface_class: u8,
    pub interface_sub_class: u8,
    pub interface_protocol: u8,
    pub interface: u8,
}

/// USB endpoint descriptor.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct EfiUsbEndpointDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub endpoint_address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

/// USB configuration descriptor.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct EfiUsbConfigDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub total_length: u16,
    pub num_interfaces: u8,
    pub configuration_value: u8,
    pub configuration: u8,
    pub attributes: u8,
    pub max_power: u8,
}

/// Common header for USB descriptors.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct UsbDescHead {
    pub len: u8,
    pub desc_type: u8,
}

/// HID class descriptor entry (type + length pair).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C, packed)]
pub struct HidClassDescriptor {
    pub descriptor_type: u8,
    pub descriptor_length: u16,
}

/// USB HID descriptor.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct EfiUsbHidDescriptor {
    pub length: u8,
    pub descriptor_type: u8,
    pub bcd_hid: u16,
    pub country_code: u8,
    pub num_descriptors: u8,
    // Followed by variable-length array of HidClassDescriptor.
    // First entry is inline; use hid_class_desc() for access.
}

impl EfiUsbHidDescriptor {
    /// Returns a slice of HID class descriptor entries.
    ///
    /// # Safety
    ///
    /// The caller must ensure `self` points to a buffer at least `self.length`
    /// bytes long, and that `self.num_descriptors` is valid.
    pub unsafe fn hid_class_desc(&self) -> &[HidClassDescriptor] {
        // SAFETY: Caller guarantees the buffer is large enough.
        let base = unsafe { (self as *const Self as *const u8).add(size_of::<Self>()) as *const HidClassDescriptor };
        // SAFETY: base is valid for num_descriptors entries as guaranteed by the caller.
        unsafe { core::slice::from_raw_parts(base, self.num_descriptors as usize) }
    }
}
