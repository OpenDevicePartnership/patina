//! USB control transfer helpers for HID devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use core::ffi::c_void;

use r_efi::efi;

use patina::uefi_protocol::usb_io::{EfiUsbDataDirection, EfiUsbDeviceRequest, EfiUsbIoProtocol, types::*};

/// Sends a USB HID SET_PROTOCOL request to switch the device to report mode.
pub fn set_protocol_request(usb_io: &EfiUsbIoProtocol, interface_number: u8, protocol: u8) -> Result<(), efi::Status> {
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_CLASS_INTERFACE_OUT,
        request: USB_HID_SET_PROTOCOL_REQUEST,
        value: protocol as u16,
        index: interface_number as u16,
        length: 0,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::NoData,
            USB_TRANSFER_TIMEOUT_MS,
            core::ptr::null_mut(),
            0,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }
    Ok(())
}

/// Sends a USB HID GET_REPORT class-specific request.
pub fn usb_get_report_request(
    usb_io: &EfiUsbIoProtocol,
    interface_number: u8,
    report_id: u8,
    report_type: u8,
    report_len: u16,
    report: *mut u8,
) -> Result<(), efi::Status> {
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_CLASS_INTERFACE_IN,
        request: USB_HID_GET_REPORT_REQUEST,
        value: (report_type as u16) << 8 | report_id as u16,
        index: interface_number as u16,
        length: report_len,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request, report buffer, and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::DataIn,
            USB_TRANSFER_TIMEOUT_MS,
            report as *mut c_void,
            report_len as usize,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }
    Ok(())
}

/// Sends a USB HID SET_REPORT class-specific request.
pub fn usb_set_report_request(
    usb_io: &EfiUsbIoProtocol,
    interface_number: u8,
    report_id: u8,
    report_type: u8,
    report_len: u16,
    report: *const u8,
) -> Result<(), efi::Status> {
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_CLASS_INTERFACE_OUT,
        request: USB_HID_SET_REPORT_REQUEST,
        value: (report_type as u16) << 8 | report_id as u16,
        index: interface_number as u16,
        length: report_len,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request, report buffer, and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::DataOut,
            USB_TRANSFER_TIMEOUT_MS,
            report as *mut c_void,
            report_len as usize,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }
    Ok(())
}

/// Sends a USB CLEAR_FEATURE(ENDPOINT_HALT) request.
pub fn usb_clear_endpoint_halt(usb_io: &EfiUsbIoProtocol, endpoint_address: u8) -> Result<(), efi::Status> {
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_STANDARD_ENDPOINT_OUT,
        request: USB_REQ_CLEAR_FEATURE,
        value: USB_FEATURE_ENDPOINT_HALT,
        index: endpoint_address as u16,
        length: 0,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::NoData,
            USB_TRANSFER_TIMEOUT_MS,
            core::ptr::null_mut(),
            0,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }
    Ok(())
}

/// Reads the report descriptor from the device via GET_DESCRIPTOR.
pub fn usb_get_report_descriptor(
    usb_io: &EfiUsbIoProtocol,
    interface_number: u8,
    descriptor_length: u16,
    descriptor_buffer: *mut u8,
) -> Result<(), efi::Status> {
    let request = EfiUsbDeviceRequest {
        request_type: USB_REQ_TYPE_STANDARD_DEVICE_IN | 0x01, // Interface recipient
        request: USB_REQ_GET_DESCRIPTOR,
        value: (USB_DESC_TYPE_REPORT as u16) << 8,
        index: interface_number as u16,
        length: descriptor_length,
    };
    let mut transfer_status: u32 = 0;
    // SAFETY: usb_io is valid; request, descriptor buffer, and status pointers are valid.
    let status = unsafe {
        (usb_io.usb_control_transfer)(
            usb_io as *const EfiUsbIoProtocol,
            &request,
            EfiUsbDataDirection::DataIn,
            USB_TRANSFER_TIMEOUT_MS,
            descriptor_buffer as *mut c_void,
            descriptor_length as usize,
            &mut transfer_status,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use core::cell::Cell;

    // ---- Mock USB IO ----

    /// Captured parameters from the most recent `usb_control_transfer` call.
    #[derive(Clone, Copy, Default)]
    struct CapturedRequest {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
        direction: u32,
        data_length: usize,
    }

    /// Mock USB IO context. `protocol` must be the first field so the extern
    /// mock function can recover mock state from the `this` pointer.
    #[repr(C)]
    struct MockUsbIo {
        protocol: EfiUsbIoProtocol,
        status: efi::Status,
        captured: Cell<CapturedRequest>,
    }

    impl MockUsbIo {
        /// # Safety
        /// `this` must point to the `protocol` field of a valid `MockUsbIo`.
        unsafe fn from_this(this: *const EfiUsbIoProtocol) -> &'static Self {
            // SAFETY: MockUsbIo is #[repr(C)] with protocol as first field.
            unsafe { &*(this as *const MockUsbIo) }
        }
    }

    extern "efiapi" fn mock_control_transfer(
        this: *const EfiUsbIoProtocol,
        request: *const EfiUsbDeviceRequest,
        direction: EfiUsbDataDirection,
        _timeout: u32,
        _data: *mut c_void,
        data_length: usize,
        _status: *mut u32,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        // SAFETY: request is a valid pointer from the caller.
        let req = unsafe { &*request };
        mock.captured.set(CapturedRequest {
            request_type: req.request_type,
            request: req.request,
            value: req.value,
            index: req.index,
            length: req.length,
            direction: direction as u32,
            data_length,
        });
        mock.status
    }

    fn make_mock(status: efi::Status) -> MockUsbIo {
        let mut protocol = EfiUsbIoProtocol::stub();
        protocol.usb_control_transfer = mock_control_transfer;
        MockUsbIo { protocol, status, captured: Cell::new(CapturedRequest::default()) }
    }

    // ---- set_protocol_request tests ----

    #[test]
    fn set_protocol_request_builds_correct_request() {
        let mock = make_mock(efi::Status::SUCCESS);
        assert!(set_protocol_request(&mock.protocol, 2, REPORT_PROTOCOL).is_ok());
        let cap = mock.captured.get();
        assert_eq!(cap.request_type, USB_REQ_TYPE_CLASS_INTERFACE_OUT);
        assert_eq!(cap.request, USB_HID_SET_PROTOCOL_REQUEST);
        assert_eq!(cap.value, REPORT_PROTOCOL as u16);
        assert_eq!(cap.index, 2);
        assert_eq!(cap.length, 0);
        assert_eq!(cap.direction, EfiUsbDataDirection::NoData as u32);
        assert_eq!(cap.data_length, 0);
    }

    #[test]
    fn set_protocol_request_returns_error_on_failure() {
        let mock = make_mock(efi::Status::DEVICE_ERROR);
        assert_eq!(set_protocol_request(&mock.protocol, 0, 0), Err(efi::Status::DEVICE_ERROR));
    }

    // ---- usb_get_report_request tests ----

    #[test]
    fn get_report_request_builds_correct_request() {
        let mock = make_mock(efi::Status::SUCCESS);
        let mut buffer = [0u8; 16];
        assert!(usb_get_report_request(&mock.protocol, 1, 0x03, 0x01, 16, buffer.as_mut_ptr()).is_ok());
        let cap = mock.captured.get();
        assert_eq!(cap.request_type, USB_REQ_TYPE_CLASS_INTERFACE_IN);
        assert_eq!(cap.request, USB_HID_GET_REPORT_REQUEST);
        assert_eq!(cap.value, (0x01u16 << 8) | 0x03);
        assert_eq!(cap.index, 1);
        assert_eq!(cap.length, 16);
        assert_eq!(cap.direction, EfiUsbDataDirection::DataIn as u32);
        assert_eq!(cap.data_length, 16);
    }

    #[test]
    fn get_report_request_returns_error_on_failure() {
        let mock = make_mock(efi::Status::DEVICE_ERROR);
        let mut buffer = [0u8; 8];
        assert_eq!(
            usb_get_report_request(&mock.protocol, 0, 0, 0x01, 8, buffer.as_mut_ptr()),
            Err(efi::Status::DEVICE_ERROR),
        );
    }

    // ---- usb_set_report_request tests ----

    #[test]
    fn set_report_request_builds_correct_request() {
        let mock = make_mock(efi::Status::SUCCESS);
        let report = [0xAAu8; 4];
        assert!(usb_set_report_request(&mock.protocol, 0, 0x01, 0x02, 4, report.as_ptr()).is_ok());
        let cap = mock.captured.get();
        assert_eq!(cap.request_type, USB_REQ_TYPE_CLASS_INTERFACE_OUT);
        assert_eq!(cap.request, USB_HID_SET_REPORT_REQUEST);
        assert_eq!(cap.value, (0x02u16 << 8) | 0x01);
        assert_eq!(cap.index, 0);
        assert_eq!(cap.length, 4);
        assert_eq!(cap.direction, EfiUsbDataDirection::DataOut as u32);
        assert_eq!(cap.data_length, 4);
    }

    #[test]
    fn set_report_request_returns_error_on_failure() {
        let mock = make_mock(efi::Status::DEVICE_ERROR);
        let report = [0u8; 4];
        assert_eq!(
            usb_set_report_request(&mock.protocol, 0, 0, 0x02, 4, report.as_ptr()),
            Err(efi::Status::DEVICE_ERROR),
        );
    }

    // ---- usb_clear_endpoint_halt tests ----

    #[test]
    fn clear_endpoint_halt_builds_correct_request() {
        let mock = make_mock(efi::Status::SUCCESS);
        assert!(usb_clear_endpoint_halt(&mock.protocol, 0x81).is_ok());
        let cap = mock.captured.get();
        assert_eq!(cap.request_type, USB_REQ_TYPE_STANDARD_ENDPOINT_OUT);
        assert_eq!(cap.request, USB_REQ_CLEAR_FEATURE);
        assert_eq!(cap.value, USB_FEATURE_ENDPOINT_HALT);
        assert_eq!(cap.index, 0x81);
        assert_eq!(cap.length, 0);
        assert_eq!(cap.direction, EfiUsbDataDirection::NoData as u32);
        assert_eq!(cap.data_length, 0);
    }

    #[test]
    fn clear_endpoint_halt_returns_error_on_failure() {
        let mock = make_mock(efi::Status::DEVICE_ERROR);
        assert_eq!(usb_clear_endpoint_halt(&mock.protocol, 0x81), Err(efi::Status::DEVICE_ERROR));
    }

    // ---- usb_get_report_descriptor tests ----

    #[test]
    fn get_report_descriptor_builds_correct_request() {
        let mock = make_mock(efi::Status::SUCCESS);
        let mut buffer = [0u8; 64];
        assert!(usb_get_report_descriptor(&mock.protocol, 0, 64, buffer.as_mut_ptr()).is_ok());
        let cap = mock.captured.get();
        assert_eq!(cap.request_type, USB_REQ_TYPE_STANDARD_DEVICE_IN | 0x01);
        assert_eq!(cap.request, USB_REQ_GET_DESCRIPTOR);
        assert_eq!(cap.value, (USB_DESC_TYPE_REPORT as u16) << 8);
        assert_eq!(cap.index, 0);
        assert_eq!(cap.length, 64);
        assert_eq!(cap.direction, EfiUsbDataDirection::DataIn as u32);
        assert_eq!(cap.data_length, 64);
    }

    #[test]
    fn get_report_descriptor_uses_interface_number_as_index() {
        let mock = make_mock(efi::Status::SUCCESS);
        let mut buffer = [0u8; 32];
        assert!(usb_get_report_descriptor(&mock.protocol, 3, 32, buffer.as_mut_ptr()).is_ok());
        assert_eq!(mock.captured.get().index, 3);
    }

    #[test]
    fn get_report_descriptor_returns_error_on_failure() {
        let mock = make_mock(efi::Status::DEVICE_ERROR);
        let mut buffer = [0u8; 64];
        assert_eq!(
            usb_get_report_descriptor(&mock.protocol, 0, 64, buffer.as_mut_ptr()),
            Err(efi::Status::DEVICE_ERROR),
        );
    }
}
