//! USB descriptor reading for HID devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use alloc::vec;
use core::{ffi::c_void, mem::size_of};

use r_efi::efi;

use crate::{device::UsbHidDescriptors, usb_requests};
use patina::uefi_protocol::usb_io::{EfiUsbDataDirection, EfiUsbDeviceRequest, EfiUsbIoProtocol, types::*};

/// Owned wrapper around the variable-length USB HID descriptor.
///
/// The backing `Vec<u8>` holds the complete descriptor bytes (fixed header +
/// trailing `HidClassDescriptor` entries). Automatically freed on drop.
#[derive(Debug)]
struct HidDescriptor {
    data: alloc::vec::Vec<u8>,
}

impl HidDescriptor {
    fn header(&self) -> &EfiUsbHidDescriptor {
        // SAFETY: data was copied from a valid HID descriptor at least size_of::<EfiUsbHidDescriptor>() bytes.
        unsafe { &*(self.data.as_ptr() as *const EfiUsbHidDescriptor) }
    }

    fn class_descriptors(&self) -> &[HidClassDescriptor] {
        let count = self.header().num_descriptors as usize;
        // SAFETY: data was copied from a valid HID descriptor with trailing class descriptor entries.
        unsafe {
            let base = self.data.as_ptr().add(size_of::<EfiUsbHidDescriptor>()) as *const HidClassDescriptor;
            core::slice::from_raw_parts(base, count)
        }
    }
}

/// Reads interface, endpoint, and HID descriptors from the device.
///
/// Returns a [`UsbHidDescriptors`] containing the interface, interrupt-in
/// endpoint, and report descriptors. Returns an error if the interrupt-in
/// endpoint is not found.
pub fn read_descriptors(usb_io: &EfiUsbIoProtocol) -> Result<UsbHidDescriptors, efi::Status> {
    let usb_io_ptr = usb_io as *const EfiUsbIoProtocol;

    let mut interface_descriptor = EfiUsbInterfaceDescriptor::default();
    // SAFETY: usb_io and interface_descriptor are valid.
    let status = unsafe { (usb_io.usb_get_interface_descriptor)(usb_io_ptr, &mut interface_descriptor) };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }

    log::trace!(
        "USB HID: interface class: 0x{:x}, subclass: 0x{:x}, protocol: 0x{:x}",
        interface_descriptor.interface_class,
        interface_descriptor.interface_sub_class,
        interface_descriptor.interface_protocol,
    );

    let mut int_in_endpoint_descriptor = EfiUsbEndpointDescriptor::default();
    for index in 0..interface_descriptor.num_endpoints {
        let mut endpoint = EfiUsbEndpointDescriptor::default();
        // SAFETY: usb_io and endpoint descriptor are valid.
        let status = unsafe { (usb_io.usb_get_endpoint_descriptor)(usb_io_ptr, index, &mut endpoint) };
        if status != efi::Status::SUCCESS {
            return Err(status);
        }

        if (endpoint.attributes & USB_ENDPOINT_XFER_TYPE_MASK) == USB_ENDPOINT_INTERRUPT
            && (endpoint.endpoint_address & USB_ENDPOINT_DIR_IN) != 0
        {
            int_in_endpoint_descriptor = endpoint;
            break;
        }
    }

    // Interrupt-in endpoint must be found.
    if int_in_endpoint_descriptor.length == 0 {
        return Err(efi::Status::DEVICE_ERROR);
    }

    let hid_descriptor = get_full_hid_descriptor(usb_io, &interface_descriptor)?;
    let report_descriptor = read_report_descriptor(usb_io, &interface_descriptor, &hid_descriptor)?;

    Ok(UsbHidDescriptors { interface_descriptor, int_in_endpoint_descriptor, report_descriptor })
}

/// Reads the report descriptor from the device using the HID descriptor's
/// class descriptor entries to determine the length.
fn read_report_descriptor(
    usb_io: &EfiUsbIoProtocol,
    interface_descriptor: &EfiUsbInterfaceDescriptor,
    hid_descriptor: &HidDescriptor,
) -> Result<alloc::vec::Vec<u8>, efi::Status> {
    let report_entry = hid_descriptor.class_descriptors().iter().find(|d| d.descriptor_type == USB_DESC_TYPE_REPORT);
    let descriptor_length = match report_entry {
        Some(entry) => entry.descriptor_length as usize,
        None => return Err(efi::Status::NOT_FOUND),
    };

    let mut buffer = vec![0u8; descriptor_length];

    usb_requests::usb_get_report_descriptor(
        usb_io,
        interface_descriptor.interface_number,
        descriptor_length as u16,
        buffer.as_mut_ptr(),
    )?;

    Ok(buffer)
}

/// Retrieves the full HID descriptor for the given interface by parsing the
/// configuration descriptor.
fn get_full_hid_descriptor(
    usb_io: &EfiUsbIoProtocol,
    interface_descriptor: &EfiUsbInterfaceDescriptor,
) -> Result<HidDescriptor, efi::Status> {
    let mut config_desc = EfiUsbConfigDescriptor::default();
    // SAFETY: usb_io and config_desc are valid.
    let status = unsafe { (usb_io.usb_get_config_descriptor)(usb_io as *const EfiUsbIoProtocol, &mut config_desc) };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }

    let total_length = config_desc.total_length as usize;
    let mut buffer = vec![0u8; total_length];

    // Read the full configuration descriptor using GET_DESCRIPTOR control transfer.
    let descriptor_value =
        (USB_DESC_TYPE_CONFIG as u16) << 8 | (config_desc.configuration_value.wrapping_sub(1)) as u16;
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_STANDARD_DEVICE_IN,
        request: USB_REQ_GET_DESCRIPTOR,
        value: descriptor_value,
        index: 0,
        length: total_length as u16,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request, buffer, and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::DataIn,
            USB_TRANSFER_TIMEOUT_MS,
            buffer.as_mut_ptr() as *mut c_void,
            total_length,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }

    find_hid_descriptor_in_config(&buffer, interface_descriptor)
}

/// Searches the configuration descriptor buffer for the HID descriptor that
/// immediately follows the matching interface descriptor.
fn find_hid_descriptor_in_config(
    buffer: &[u8],
    interface_descriptor: &EfiUsbInterfaceDescriptor,
) -> Result<HidDescriptor, efi::Status> {
    let mut cursor: usize = 0;

    while cursor + size_of::<UsbDescHead>() <= buffer.len() {
        // SAFETY: bounds check above ensures UsbDescHead fits at cursor.
        let header = unsafe { &*(buffer.as_ptr().add(cursor) as *const UsbDescHead) };

        if header.len == 0 {
            log::error!("USB HID: descriptor length is 0 at offset {cursor}");
            break;
        }

        if header.desc_type == USB_DESC_TYPE_INTERFACE {
            // SAFETY: We've checked desc_type; the interface descriptor is at cursor.
            let iface = unsafe { &*(buffer.as_ptr().add(cursor) as *const EfiUsbInterfaceDescriptor) };
            if iface.interface_number == interface_descriptor.interface_number
                && iface.alternate_setting == interface_descriptor.alternate_setting
            {
                // The HID descriptor must immediately follow the interface descriptor.
                let next_offset = cursor + header.len as usize;
                if next_offset + size_of::<UsbDescHead>() <= buffer.len() {
                    // SAFETY: bounds check above ensures UsbDescHead fits at next_offset.
                    let next_header = unsafe { &*(buffer.as_ptr().add(next_offset) as *const UsbDescHead) };
                    if next_header.desc_type == USB_DESC_TYPE_HID {
                        let len = next_header.len as usize;
                        return Ok(HidDescriptor { data: buffer[next_offset..next_offset + len].to_vec() });
                    }
                }
                // HID descriptor not found at expected position.
                break;
            }
        }

        cursor += header.len as usize;
    }

    Err(efi::Status::UNSUPPORTED)
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::{vec, vec::Vec};
    use core::{cell::Cell, ffi::c_void};

    // ---- Descriptor byte builders ----

    fn interface_bytes(number: u8, alt: u8, num_endpoints: u8) -> Vec<u8> {
        vec![9, USB_DESC_TYPE_INTERFACE, number, alt, num_endpoints, CLASS_HID, 0, 0, 0]
    }

    fn hid_bytes(report_desc_len: u16) -> Vec<u8> {
        let total = (size_of::<EfiUsbHidDescriptor>() + size_of::<HidClassDescriptor>()) as u8;
        vec![
            total,
            USB_DESC_TYPE_HID,
            0x11,
            0x01, // bcd_hid
            0,    // country_code
            1,    // num_descriptors
            USB_DESC_TYPE_REPORT,
            (report_desc_len & 0xFF) as u8,
            (report_desc_len >> 8) as u8,
        ]
    }

    fn endpoint_bytes(address: u8, attributes: u8) -> Vec<u8> {
        vec![7, 5, address, attributes, 8, 0, 10]
    }

    fn config_header_bytes(total_length: u16) -> Vec<u8> {
        vec![9, USB_DESC_TYPE_CONFIG, (total_length & 0xFF) as u8, (total_length >> 8) as u8, 1, 1, 0, 0x80, 50]
    }

    /// Builds a complete config descriptor buffer with correct total_length header.
    fn build_config_buffer(descs: &[&[u8]]) -> Vec<u8> {
        let payload_len: usize = descs.iter().map(|d| d.len()).sum();
        let total_length = (9 + payload_len) as u16;
        let mut buffer = config_header_bytes(total_length);
        for desc in descs {
            buffer.extend_from_slice(desc);
        }
        buffer
    }

    fn concat(slices: &[&[u8]]) -> Vec<u8> {
        slices.iter().flat_map(|s| s.iter().copied()).collect()
    }

    fn make_interface(number: u8, alt: u8, num_endpoints: u8) -> EfiUsbInterfaceDescriptor {
        EfiUsbInterfaceDescriptor {
            length: 9,
            descriptor_type: USB_DESC_TYPE_INTERFACE,
            interface_number: number,
            alternate_setting: alt,
            num_endpoints,
            interface_class: CLASS_HID,
            ..Default::default()
        }
    }

    fn make_endpoint(address: u8, attributes: u8) -> EfiUsbEndpointDescriptor {
        EfiUsbEndpointDescriptor {
            length: 7,
            descriptor_type: 5,
            endpoint_address: address,
            attributes,
            max_packet_size: 8,
            interval: 10,
        }
    }

    // ---- HidDescriptor tests ----

    #[test]
    fn hid_descriptor_header_returns_correct_fields() {
        let data = hid_bytes(64);
        let hid = HidDescriptor { data };
        let header = *hid.header();
        assert_eq!(header.descriptor_type, USB_DESC_TYPE_HID);
        let bcd = header.bcd_hid;
        assert_eq!(bcd, 0x0111);
        assert_eq!(header.country_code, 0);
        assert_eq!(header.num_descriptors, 1);
    }

    #[test]
    fn hid_descriptor_class_descriptors_returns_entries() {
        let data = hid_bytes(256);
        let hid = HidDescriptor { data };
        let class_descs = hid.class_descriptors();
        assert_eq!(class_descs.len(), 1);
        let entry = class_descs[0];
        assert_eq!(entry.descriptor_type, USB_DESC_TYPE_REPORT);
        let len = entry.descriptor_length;
        assert_eq!(len, 256);
    }

    // ---- find_hid_descriptor_in_config tests ----

    #[test]
    fn find_hid_desc_succeeds_for_matching_interface() {
        let iface = make_interface(0, 0, 1);
        let buffer =
            concat(&[&interface_bytes(0, 0, 1), &hid_bytes(64), &endpoint_bytes(0x81, USB_ENDPOINT_INTERRUPT)]);
        let result = find_hid_descriptor_in_config(&buffer, &iface).unwrap();
        let header = *result.header();
        assert_eq!(header.descriptor_type, USB_DESC_TYPE_HID);
        assert_eq!(header.num_descriptors, 1);
        let entry = result.class_descriptors()[0];
        assert_eq!(entry.descriptor_type, USB_DESC_TYPE_REPORT);
        let len = entry.descriptor_length;
        assert_eq!(len, 64);
    }

    #[test]
    fn find_hid_desc_fails_for_wrong_interface_number() {
        let iface = make_interface(1, 0, 1);
        let buffer = concat(&[&interface_bytes(0, 0, 1), &hid_bytes(64)]);
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_fails_for_wrong_alternate_setting() {
        let iface = make_interface(0, 1, 1);
        let buffer = concat(&[&interface_bytes(0, 0, 1), &hid_bytes(64)]);
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_fails_when_non_hid_follows_interface() {
        let iface = make_interface(0, 0, 1);
        let buffer = concat(&[&interface_bytes(0, 0, 1), &endpoint_bytes(0x81, USB_ENDPOINT_INTERRUPT)]);
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_fails_on_empty_buffer() {
        let iface = make_interface(0, 0, 1);
        assert_eq!(find_hid_descriptor_in_config(&[], &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_handles_zero_length_descriptor() {
        let iface = make_interface(0, 0, 1);
        let buffer = vec![0, USB_DESC_TYPE_INTERFACE];
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_fails_when_buffer_truncated_after_interface() {
        let iface = make_interface(0, 0, 1);
        let buffer = interface_bytes(0, 0, 1);
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn find_hid_desc_selects_correct_interface_among_multiple() {
        let iface = make_interface(1, 0, 1);
        let buffer = concat(&[
            &interface_bytes(0, 0, 1),
            &hid_bytes(32),
            &endpoint_bytes(0x81, USB_ENDPOINT_INTERRUPT),
            &interface_bytes(1, 0, 1),
            &hid_bytes(128),
            &endpoint_bytes(0x82, USB_ENDPOINT_INTERRUPT),
        ]);
        let result = find_hid_descriptor_in_config(&buffer, &iface).unwrap();
        let entry = result.class_descriptors()[0];
        let len = entry.descriptor_length;
        assert_eq!(len, 128);
    }

    #[test]
    fn find_hid_desc_buffer_too_small_for_header() {
        let iface = make_interface(0, 0, 1);
        let buffer = vec![9]; // 1 byte, can't fit UsbDescHead (2 bytes)
        assert_eq!(find_hid_descriptor_in_config(&buffer, &iface).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    // ---- Mock EfiUsbIoProtocol ----

    /// Test wrapper containing an `EfiUsbIoProtocol` as the first field so that
    /// extern "efiapi" mock functions can recover the mock data via the `this`
    /// pointer (same containing-record pattern used by production code).
    #[repr(C)]
    struct MockUsbIo {
        protocol: EfiUsbIoProtocol,
        interface_desc: EfiUsbInterfaceDescriptor,
        interface_status: efi::Status,
        endpoints: Vec<EfiUsbEndpointDescriptor>,
        config_desc: EfiUsbConfigDescriptor,
        config_status: efi::Status,
        config_buffer: Vec<u8>,
        report_descriptor: Vec<u8>,
        control_call_count: Cell<usize>,
        control_statuses: Vec<efi::Status>,
    }

    impl MockUsbIo {
        /// # Safety
        /// `this` must point to the `protocol` field of a valid `MockUsbIo`.
        unsafe fn from_this(this: *const EfiUsbIoProtocol) -> &'static Self {
            // SAFETY: MockUsbIo is #[repr(C)] with protocol as first field.
            unsafe { &*(this as *const MockUsbIo) }
        }
    }

    extern "efiapi" fn mock_get_interface_descriptor(
        this: *const EfiUsbIoProtocol,
        desc: *mut EfiUsbInterfaceDescriptor,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        if mock.interface_status == efi::Status::SUCCESS {
            // SAFETY: desc is a valid output pointer from the caller.
            unsafe {
                *desc = mock.interface_desc;
            }
        }
        mock.interface_status
    }

    extern "efiapi" fn mock_get_endpoint_descriptor(
        this: *const EfiUsbIoProtocol,
        index: u8,
        desc: *mut EfiUsbEndpointDescriptor,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        let idx = index as usize;
        if idx < mock.endpoints.len() {
            // SAFETY: desc is a valid output pointer from the caller.
            unsafe {
                *desc = mock.endpoints[idx];
            }
            efi::Status::SUCCESS
        } else {
            efi::Status::INVALID_PARAMETER
        }
    }

    extern "efiapi" fn mock_get_config_descriptor(
        this: *const EfiUsbIoProtocol,
        desc: *mut EfiUsbConfigDescriptor,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        if mock.config_status == efi::Status::SUCCESS {
            // SAFETY: desc is a valid output pointer from the caller.
            unsafe {
                *desc = mock.config_desc;
            }
        }
        mock.config_status
    }

    extern "efiapi" fn mock_control_transfer(
        this: *const EfiUsbIoProtocol,
        _request: *const EfiUsbDeviceRequest,
        _direction: EfiUsbDataDirection,
        _timeout: u32,
        data: *mut c_void,
        data_length: usize,
        _status: *mut u32,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        let call_idx = mock.control_call_count.get();
        mock.control_call_count.set(call_idx + 1);

        let status = mock.control_statuses.get(call_idx).copied().unwrap_or(efi::Status::SUCCESS);
        if status != efi::Status::SUCCESS {
            return status;
        }

        // Call 0: config descriptor read; Call 1: report descriptor read.
        let source = if call_idx == 0 { &mock.config_buffer } else { &mock.report_descriptor };
        let copy_len = data_length.min(source.len());
        if copy_len > 0 && !data.is_null() {
            // SAFETY: data is a valid buffer of data_length bytes, source is at least copy_len bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(source.as_ptr(), data as *mut u8, copy_len);
            }
        }
        efi::Status::SUCCESS
    }

    fn build_mock(
        interface: EfiUsbInterfaceDescriptor,
        endpoints: Vec<EfiUsbEndpointDescriptor>,
        config_buffer: Vec<u8>,
        report_descriptor: Vec<u8>,
    ) -> MockUsbIo {
        let total_length = config_buffer.len() as u16;
        let mut protocol = EfiUsbIoProtocol::stub();
        protocol.usb_control_transfer = mock_control_transfer;
        protocol.usb_get_config_descriptor = mock_get_config_descriptor;
        protocol.usb_get_interface_descriptor = mock_get_interface_descriptor;
        protocol.usb_get_endpoint_descriptor = mock_get_endpoint_descriptor;
        MockUsbIo {
            protocol,
            interface_desc: interface,
            interface_status: efi::Status::SUCCESS,
            endpoints,
            config_desc: EfiUsbConfigDescriptor {
                length: 9,
                descriptor_type: USB_DESC_TYPE_CONFIG,
                total_length,
                num_interfaces: 1,
                configuration_value: 1,
                ..Default::default()
            },
            config_status: efi::Status::SUCCESS,
            config_buffer,
            report_descriptor,
            control_call_count: Cell::new(0),
            control_statuses: vec![],
        }
    }

    /// Builds a standard config buffer with one HID interface + interrupt-in endpoint.
    fn standard_config_buffer(interface_number: u8, report_desc_len: u16) -> Vec<u8> {
        build_config_buffer(&[
            &interface_bytes(interface_number, 0, 1),
            &hid_bytes(report_desc_len),
            &endpoint_bytes(0x81, USB_ENDPOINT_INTERRUPT),
        ])
    }

    // ---- read_descriptors integration tests ----

    #[test]
    fn read_descriptors_succeeds() {
        let report_data = vec![0x05, 0x01, 0x09, 0x06];
        let mock = build_mock(
            make_interface(0, 0, 1),
            vec![make_endpoint(0x81, USB_ENDPOINT_INTERRUPT)],
            standard_config_buffer(0, report_data.len() as u16),
            report_data.clone(),
        );

        let result = read_descriptors(&mock.protocol).unwrap();
        assert_eq!(result.interface_descriptor.interface_class, CLASS_HID);
        assert_eq!(result.int_in_endpoint_descriptor.endpoint_address, 0x81);
        assert_eq!(result.report_descriptor, report_data);
    }

    #[test]
    fn read_descriptors_fails_on_interface_error() {
        let mut mock = build_mock(make_interface(0, 0, 1), vec![], vec![], vec![]);
        mock.interface_status = efi::Status::DEVICE_ERROR;

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_when_no_interrupt_in_endpoint() {
        // Endpoint is interrupt OUT, not IN.
        let mock =
            build_mock(make_interface(0, 0, 1), vec![make_endpoint(0x01, USB_ENDPOINT_INTERRUPT)], vec![], vec![]);

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_when_no_endpoints() {
        let mock = build_mock(make_interface(0, 0, 0), vec![], vec![], vec![]);

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_on_config_descriptor_error() {
        let mut mock =
            build_mock(make_interface(0, 0, 1), vec![make_endpoint(0x81, USB_ENDPOINT_INTERRUPT)], vec![], vec![]);
        mock.config_status = efi::Status::DEVICE_ERROR;

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_on_config_transfer_error() {
        let mut mock = build_mock(
            make_interface(0, 0, 1),
            vec![make_endpoint(0x81, USB_ENDPOINT_INTERRUPT)],
            standard_config_buffer(0, 4),
            vec![],
        );
        mock.control_statuses = vec![efi::Status::DEVICE_ERROR];

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_on_report_descriptor_transfer_error() {
        let mut mock = build_mock(
            make_interface(0, 0, 1),
            vec![make_endpoint(0x81, USB_ENDPOINT_INTERRUPT)],
            standard_config_buffer(0, 4),
            vec![],
        );
        // First control transfer (config read) succeeds, second (report read) fails.
        mock.control_statuses = vec![efi::Status::SUCCESS, efi::Status::DEVICE_ERROR];

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_descriptors_fails_when_hid_descriptor_not_in_config() {
        // Config buffer has interface but no HID descriptor after it.
        let config_buffer =
            build_config_buffer(&[&interface_bytes(0, 0, 1), &endpoint_bytes(0x81, USB_ENDPOINT_INTERRUPT)]);
        let mock = build_mock(
            make_interface(0, 0, 1),
            vec![make_endpoint(0x81, USB_ENDPOINT_INTERRUPT)],
            config_buffer,
            vec![],
        );

        assert_eq!(read_descriptors(&mock.protocol).unwrap_err(), efi::Status::UNSUPPORTED);
    }

    #[test]
    fn read_descriptors_skips_non_interrupt_endpoints() {
        // First endpoint is bulk OUT, second is interrupt IN.
        let mock = build_mock(
            make_interface(0, 0, 2),
            vec![
                make_endpoint(0x02, 0x02),                   // bulk OUT
                make_endpoint(0x81, USB_ENDPOINT_INTERRUPT), // interrupt IN
            ],
            standard_config_buffer(0, 4),
            vec![0x05, 0x01, 0x09, 0x06],
        );

        let result = read_descriptors(&mock.protocol).unwrap();
        assert_eq!(result.int_in_endpoint_descriptor.endpoint_address, 0x81);
    }
}
