//! HidIoProtocol function pointer implementations for USB HID devices.
//!
//! Each function recovers the `UsbHidDevice` from the protocol pointer,
//! then delegates to USB IO operations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use core::ffi::c_void;

use r_efi::efi;

use patina::vendor_protocols::hid_io::{HidIoProtocol, HidIoReportCallback, HidReportType};

use crate::{control_transfers, device::UsbHidDevice, interrupt_transfers};

/// Creates a new `HidIoProtocol` populated with this module's function pointers.
pub fn new_hid_io_protocol() -> HidIoProtocol {
    HidIoProtocol {
        get_report_descriptor: hid_get_report_descriptor,
        get_report: hid_get_report,
        set_report: hid_set_report,
        register_report_callback: hid_register_report_callback,
        unregister_report_callback: hid_unregister_report_callback,
    }
}

/// Retrieves the HID report descriptor from the device.
///
/// # Safety
///
/// `this` must point to the `hid_io` field of a valid, heap-allocated
/// [`UsbHidDevice`]. `report_descriptor_size` must be a valid pointer.
/// If the buffer is large enough, `report_descriptor_buffer` must be valid
/// for `*report_descriptor_size` bytes.
unsafe extern "efiapi" fn hid_get_report_descriptor(
    this: *const HidIoProtocol,
    report_descriptor_size: *mut usize,
    report_descriptor_buffer: *mut c_void,
) -> efi::Status {
    if this.is_null() || report_descriptor_size.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: this points to the hid_io field of a valid UsbHidDevice.
    let device = unsafe { &mut *UsbHidDevice::from_hid_io_protocol(this) };

    if device.descriptors.report_descriptor.is_empty() {
        return efi::Status::NOT_FOUND;
    }

    // SAFETY: report_descriptor_size is checked non-null above.
    let requested_size = unsafe { *report_descriptor_size };
    if requested_size < device.descriptors.report_descriptor.len() {
        // SAFETY: report_descriptor_size is checked non-null above.
        unsafe { *report_descriptor_size = device.descriptors.report_descriptor.len() };
        return efi::Status::BUFFER_TOO_SMALL;
    }

    if report_descriptor_buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: Both pointers are valid for report_descriptor.len() bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            device.descriptors.report_descriptor.as_ptr(),
            report_descriptor_buffer as *mut u8,
            device.descriptors.report_descriptor.len(),
        );
    }

    efi::Status::SUCCESS
}

/// Retrieves a single report from the device via USB GET_REPORT request.
///
/// # Safety
///
/// `this` must point to the `hid_io` field of a valid, heap-allocated
/// [`UsbHidDevice`]. `report_buffer` must be valid for `report_buffer_size`
/// bytes.
unsafe extern "efiapi" fn hid_get_report(
    this: *const HidIoProtocol,
    report_id: u8,
    report_type: HidReportType,
    report_buffer_size: usize,
    report_buffer: *mut c_void,
) -> efi::Status {
    if this.is_null() || report_buffer_size == 0 || report_buffer_size > u16::MAX as usize || report_buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Only support Get_Report for Input or Feature reports.
    if report_type != HidReportType::InputReport && report_type != HidReportType::Feature {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: this points to the hid_io field of a valid UsbHidDevice.
    let device = unsafe { &*UsbHidDevice::from_hid_io_protocol(this) };
    // SAFETY: usb_io is valid for the device's lifetime.
    let usb_io = unsafe { &*device.usb_io };

    match control_transfers::usb_get_report_request(
        usb_io,
        device.descriptors.interface_descriptor.interface_number,
        report_id,
        report_type as u8,
        report_buffer_size as u16,
        report_buffer as *mut u8,
    ) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

/// Sends a single report to the device via USB SET_REPORT request.
///
/// # Safety
///
/// `this` must point to the `hid_io` field of a valid, heap-allocated
/// [`UsbHidDevice`]. `report_buffer` must be valid for `report_buffer_size`
/// bytes.
unsafe extern "efiapi" fn hid_set_report(
    this: *const HidIoProtocol,
    report_id: u8,
    report_type: HidReportType,
    report_buffer_size: usize,
    report_buffer: *mut c_void,
) -> efi::Status {
    if this.is_null() || report_buffer_size == 0 || report_buffer_size > u16::MAX as usize || report_buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // Only support Set_Report for Output or Feature reports.
    if report_type != HidReportType::OutputReport && report_type != HidReportType::Feature {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: this points to the hid_io field of a valid UsbHidDevice.
    let device = unsafe { &*UsbHidDevice::from_hid_io_protocol(this) };
    // SAFETY: usb_io is valid for the device's lifetime.
    let usb_io = unsafe { &*device.usb_io };

    match control_transfers::usb_set_report_request(
        usb_io,
        device.descriptors.interface_descriptor.interface_number,
        report_id,
        report_type as u8,
        report_buffer_size as u16,
        report_buffer as *const u8,
    ) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => status,
    }
}

/// Registers a callback function to receive asynchronous input reports.
///
/// # Safety
///
/// `this` must point to the `hid_io` field of a valid, heap-allocated
/// [`UsbHidDevice`]. `context` must remain valid for the lifetime of the
/// registration (until the callback is unregistered).
unsafe extern "efiapi" fn hid_register_report_callback(
    this: *const HidIoProtocol,
    callback: HidIoReportCallback,
    context: *mut c_void,
) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: this points to the hid_io field of a valid UsbHidDevice.
    let device = unsafe { &mut *UsbHidDevice::from_hid_io_protocol(this) };

    if device.report_callback.callback.is_some() {
        return efi::Status::ALREADY_STARTED;
    }

    device.report_callback.callback = Some(callback);
    device.report_callback.context = context;

    match interrupt_transfers::initiate_async_interrupt_input_transfers(device) {
        Ok(()) => efi::Status::SUCCESS,
        Err(status) => {
            device.report_callback.callback = None;
            status
        }
    }
}

/// Unregisters a previously registered callback function.
///
/// # Safety
///
/// `this` must point to the `hid_io` field of a valid, heap-allocated
/// [`UsbHidDevice`].
unsafe extern "efiapi" fn hid_unregister_report_callback(
    this: *const HidIoProtocol,
    callback: HidIoReportCallback,
) -> efi::Status {
    if this.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: this points to the hid_io field of a valid UsbHidDevice.
    let device = unsafe { &mut *UsbHidDevice::from_hid_io_protocol(this) };

    // Verify the callback matches the registered one.
    match device.report_callback.callback {
        Some(registered) if core::ptr::fn_addr_eq(registered, callback) => {}
        _ => return efi::Status::NOT_STARTED,
    }

    match interrupt_transfers::shutdown_async_interrupt_input_transfers(device) {
        Ok(()) => {}
        Err(status) => {
            log::error!("USB HID: error shutting down transfers during unregister: {status:x?}");
        }
    }

    device.report_callback.callback = None;

    efi::Status::SUCCESS
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::{boxed::Box, vec, vec::Vec};
    use core::cell::Cell;

    use patina::uefi_protocol::usb_io::{
        EfiAsyncUsbTransferCallback, EfiUsbDataDirection, EfiUsbDeviceRequest, EfiUsbIoProtocol, types::*,
    };

    use crate::{
        device::{ReportCallbackState, UsbHidDescriptors, UsbHidDevice},
        interrupt_transfers::TransferRecoveryTimer,
    };

    struct NoopTransferRecoveryTimer;
    impl TransferRecoveryTimer for NoopTransferRecoveryTimer {
        fn arm_recovery_timer(&self, _event: efi::Event, _delay: u64) -> Result<(), efi::Status> {
            Ok(())
        }
    }
    static NOOP_RECOVERY_TIMER: NoopTransferRecoveryTimer = NoopTransferRecoveryTimer;

    // ---- Mock USB IO ----

    /// Mock USB IO context. `protocol` must be the first field so the extern
    /// mock functions can recover the mock state from the `this` pointer.
    #[repr(C)]
    struct MockUsbIo {
        protocol: EfiUsbIoProtocol,
        control_transfer_status: efi::Status,
        async_transfer_status: efi::Status,
        control_call_count: Cell<usize>,
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
        _request: *const EfiUsbDeviceRequest,
        _direction: EfiUsbDataDirection,
        _timeout: u32,
        _data: *mut c_void,
        _data_length: usize,
        _status: *mut u32,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        mock.control_call_count.set(mock.control_call_count.get() + 1);
        mock.control_transfer_status
    }

    extern "efiapi" fn mock_async_interrupt_transfer(
        this: *const EfiUsbIoProtocol,
        _endpoint: u8,
        _is_new_transfer: bool,
        _polling_interval: usize,
        _data_length: usize,
        _callback: Option<EfiAsyncUsbTransferCallback>,
        _context: *mut c_void,
    ) -> efi::Status {
        // SAFETY: this points to a valid MockUsbIo on the test stack.
        let mock = unsafe { MockUsbIo::from_this(this) };
        mock.async_transfer_status
    }

    fn make_mock_usb_io(control_status: efi::Status, async_status: efi::Status) -> MockUsbIo {
        let mut protocol = EfiUsbIoProtocol::stub();
        protocol.usb_control_transfer = mock_control_transfer;
        protocol.usb_async_interrupt_transfer = mock_async_interrupt_transfer;
        MockUsbIo {
            protocol,
            control_transfer_status: control_status,
            async_transfer_status: async_status,
            control_call_count: Cell::new(0),
        }
    }

    // ---- Test device builder ----

    fn make_device(usb_io: &MockUsbIo, report_descriptor: Vec<u8>) -> Box<UsbHidDevice> {
        Box::new(UsbHidDevice {
            hid_io: new_hid_io_protocol(),
            usb_io: &usb_io.protocol as *const EfiUsbIoProtocol,
            descriptors: UsbHidDescriptors {
                interface_descriptor: EfiUsbInterfaceDescriptor::default(),
                int_in_endpoint_descriptor: EfiUsbEndpointDescriptor {
                    endpoint_address: 0x81,
                    interval: 10,
                    max_packet_size: 8,
                    ..Default::default()
                },
                report_descriptor,
            },
            report_callback: ReportCallbackState::default(),
            timer_services: &NOOP_RECOVERY_TIMER,
            recovery_event: core::ptr::null_mut(),
        })
    }

    unsafe extern "efiapi" fn test_callback(
        _report_buffer_size: u16,
        _report_buffer: *mut c_void,
        _context: *mut c_void,
    ) {
    }

    unsafe extern "efiapi" fn other_callback(
        _report_buffer_size: u16,
        _report_buffer: *mut c_void,
        _context: *mut c_void,
    ) {
    }

    // ---- get_report_descriptor tests ----

    #[test]
    fn get_report_descriptor_null_this_returns_invalid_parameter() {
        let mut size = 64usize;
        let mut buffer = [0u8; 64];
        // SAFETY: testing null this handling.
        let status =
            unsafe { hid_get_report_descriptor(core::ptr::null(), &mut size, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn get_report_descriptor_null_size_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05, 0x01]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 64];
        // SAFETY: device is a valid UsbHidDevice; testing null size handling.
        let status =
            unsafe { hid_get_report_descriptor(hid_io_ptr, core::ptr::null_mut(), buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_descriptor_empty_descriptor_returns_not_found() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut size = 64usize;
        let mut buffer = [0u8; 64];
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_get_report_descriptor(hid_io_ptr, &mut size, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::NOT_FOUND);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_descriptor_buffer_too_small_updates_size() {
        let descriptor = vec![0x05, 0x01, 0x09, 0x06, 0xA1, 0x01];
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, descriptor.clone());
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut size = 2usize; // smaller than descriptor
        let mut buffer = [0u8; 2];
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_get_report_descriptor(hid_io_ptr, &mut size, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::BUFFER_TOO_SMALL);
        assert_eq!(size, descriptor.len());
        core::mem::forget(device);
    }

    #[test]
    fn get_report_descriptor_null_buffer_with_sufficient_size_returns_invalid_parameter() {
        let descriptor = vec![0x05, 0x01];
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, descriptor);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut size = 64usize;
        // SAFETY: device is a valid UsbHidDevice; testing null buffer handling.
        let status = unsafe { hid_get_report_descriptor(hid_io_ptr, &mut size, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_descriptor_succeeds() {
        let descriptor = vec![0x05, 0x01, 0x09, 0x06];
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, descriptor.clone());
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut size = 64usize;
        let mut buffer = [0u8; 64];
        // SAFETY: device is a valid UsbHidDevice; buffer is large enough.
        let status = unsafe { hid_get_report_descriptor(hid_io_ptr, &mut size, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(&buffer[..descriptor.len()], &descriptor);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_descriptor_exact_size_succeeds() {
        let descriptor = vec![0xAA, 0xBB, 0xCC];
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, descriptor.clone());
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut size = descriptor.len();
        let mut buffer = vec![0u8; descriptor.len()];
        // SAFETY: device is a valid UsbHidDevice; buffer is exact size.
        let status = unsafe { hid_get_report_descriptor(hid_io_ptr, &mut size, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(buffer, descriptor);
        core::mem::forget(device);
    }

    // ---- get_report tests ----

    #[test]
    fn get_report_null_this_returns_invalid_parameter() {
        let mut buffer = [0u8; 8];
        // SAFETY: testing null this handling.
        let status = unsafe {
            hid_get_report(core::ptr::null(), 0, HidReportType::InputReport, 8, buffer.as_mut_ptr() as *mut c_void)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn get_report_null_buffer_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice; testing null buffer.
        let status = unsafe { hid_get_report(hid_io_ptr, 0, HidReportType::InputReport, 8, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_zero_size_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing zero size.
        let status =
            unsafe { hid_get_report(hid_io_ptr, 0, HidReportType::InputReport, 0, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_size_exceeds_u16_max_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing oversized request.
        let status = unsafe {
            hid_get_report(
                hid_io_ptr,
                0,
                HidReportType::InputReport,
                u16::MAX as usize + 1,
                buffer.as_mut_ptr() as *mut c_void,
            )
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_output_type_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing invalid report type.
        let status = unsafe {
            hid_get_report(hid_io_ptr, 0, HidReportType::OutputReport, 8, buffer.as_mut_ptr() as *mut c_void)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_input_type_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_get_report(hid_io_ptr, 1, HidReportType::InputReport, 8, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(mock_usb.control_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_feature_type_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_get_report(hid_io_ptr, 1, HidReportType::Feature, 8, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(mock_usb.control_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn get_report_usb_error_is_propagated() {
        let mock_usb = make_mock_usb_io(efi::Status::DEVICE_ERROR, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let mut buffer = [0u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_get_report(hid_io_ptr, 0, HidReportType::InputReport, 8, buffer.as_mut_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        core::mem::forget(device);
    }

    // ---- set_report tests ----

    #[test]
    fn set_report_null_this_returns_invalid_parameter() {
        let buffer = [0x01u8; 8];
        // SAFETY: testing null this handling.
        let status = unsafe {
            hid_set_report(core::ptr::null(), 0, HidReportType::OutputReport, 8, buffer.as_ptr() as *mut c_void)
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn set_report_null_buffer_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice; testing null buffer.
        let status = unsafe { hid_set_report(hid_io_ptr, 0, HidReportType::OutputReport, 8, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_zero_size_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing zero size.
        let status =
            unsafe { hid_set_report(hid_io_ptr, 0, HidReportType::OutputReport, 0, buffer.as_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_size_exceeds_u16_max_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing oversized request.
        let status = unsafe {
            hid_set_report(
                hid_io_ptr,
                0,
                HidReportType::OutputReport,
                u16::MAX as usize + 1,
                buffer.as_ptr() as *mut c_void,
            )
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_input_type_returns_invalid_parameter() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice; testing invalid report type.
        let status =
            unsafe { hid_set_report(hid_io_ptr, 0, HidReportType::InputReport, 8, buffer.as_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_output_type_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_set_report(hid_io_ptr, 1, HidReportType::OutputReport, 8, buffer.as_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(mock_usb.control_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_feature_type_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_set_report(hid_io_ptr, 1, HidReportType::Feature, 8, buffer.as_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(mock_usb.control_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn set_report_usb_error_is_propagated() {
        let mock_usb = make_mock_usb_io(efi::Status::DEVICE_ERROR, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let buffer = [0x01u8; 8];
        // SAFETY: device is a valid UsbHidDevice.
        let status =
            unsafe { hid_set_report(hid_io_ptr, 0, HidReportType::OutputReport, 8, buffer.as_ptr() as *mut c_void) };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        core::mem::forget(device);
    }

    // ---- register_report_callback tests ----

    #[test]
    fn register_callback_null_this_returns_invalid_parameter() {
        // SAFETY: testing null this handling.
        let status = unsafe { hid_register_report_callback(core::ptr::null(), test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn register_callback_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert!(device.report_callback.callback.is_some());
        core::mem::forget(device);
    }

    #[test]
    fn register_callback_already_started() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // Register first callback.
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::SUCCESS);
        // Second registration should fail.
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::ALREADY_STARTED);
        core::mem::forget(device);
    }

    #[test]
    fn register_callback_clears_callback_on_transfer_failure() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::DEVICE_ERROR);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        assert!(device.report_callback.callback.is_none());
        core::mem::forget(device);
    }

    // ---- unregister_report_callback tests ----

    #[test]
    fn unregister_callback_null_this_returns_invalid_parameter() {
        // SAFETY: testing null this handling.
        let status = unsafe { hid_unregister_report_callback(core::ptr::null(), test_callback) };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn unregister_callback_not_started() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice; no callback registered.
        let status = unsafe { hid_unregister_report_callback(hid_io_ptr, test_callback) };
        assert_eq!(status, efi::Status::NOT_STARTED);
        core::mem::forget(device);
    }

    #[test]
    fn unregister_callback_wrong_callback_returns_not_started() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // Register one callback, then try to unregister a different one.
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::SUCCESS);
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_unregister_report_callback(hid_io_ptr, other_callback) };
        assert_eq!(status, efi::Status::NOT_STARTED);
        assert!(device.report_callback.callback.is_some());
        core::mem::forget(device);
    }

    #[test]
    fn unregister_callback_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let device = make_device(&mock_usb, vec![0x05]);
        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_register_report_callback(hid_io_ptr, test_callback, core::ptr::null_mut()) };
        assert_eq!(status, efi::Status::SUCCESS);
        // SAFETY: device is a valid UsbHidDevice.
        let status = unsafe { hid_unregister_report_callback(hid_io_ptr, test_callback) };
        assert_eq!(status, efi::Status::SUCCESS);
        assert!(device.report_callback.callback.is_none());
        core::mem::forget(device);
    }
}
