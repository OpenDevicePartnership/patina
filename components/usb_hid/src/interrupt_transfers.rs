//! Async interrupt transfer management for USB HID.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use core::ffi::c_void;

use r_efi::efi;

use patina::boot_services::{
    BootServices,
    event::{EventTimerType, EventType},
    tpl::Tpl,
};

use crate::{control_transfers, device::UsbHidDevice};
use patina::uefi_protocol::usb_io::types::*;

/// Delay in 100ns units before re-submitting after a transfer error.
/// 100ms matches the standard EDKII `EFI_USB_INTERRUPT_DELAY`.
const RECOVERY_DELAY_100NS: u64 = 1_000_000;

/// Object-safe subset of [`BootServices`] for timer operations.
///
/// The interrupt completion callback only has access to [`UsbHidDevice`] via a
/// raw context pointer. This trait provides a narrow, object-safe interface so
/// the callback can arm a recovery timer without requiring the full (non-object-safe)
/// `BootServices` trait.
pub(crate) trait TransferRecoveryTimer {
    /// Arms a one-shot timer event to fire after `delay_100ns` units of 100ns.
    fn arm_recovery_timer(&self, event: efi::Event, delay_100ns: u64) -> Result<(), efi::Status>;
}

impl<T: BootServices> TransferRecoveryTimer for T {
    fn arm_recovery_timer(&self, event: efi::Event, delay_100ns: u64) -> Result<(), efi::Status> {
        self.set_timer(event, EventTimerType::Relative, delay_100ns)
    }
}

/// Creates a recovery timer event whose notify function re-submits the async
/// interrupt transfer on the given device.
///
/// # Safety
///
/// `device_ptr` must point to a valid, heap-allocated [`UsbHidDevice`] that will
/// remain valid for the lifetime of the returned event. The caller is responsible
/// for closing the event (via [`BootServices::close_event`]) before the device is
/// freed.
pub unsafe fn create_recovery_event<U: BootServices>(
    boot_services: &U,
    device_ptr: *mut UsbHidDevice,
) -> Result<efi::Event, efi::Status> {
    // SAFETY: Caller guarantees device_ptr is valid for the event's lifetime.
    // The event fires at TPL_CALLBACK, below the typical interrupt transfer TPL.
    unsafe {
        boot_services.create_event_unchecked::<UsbHidDevice>(
            EventType::TIMER | EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(recovery_timer_notify),
            device_ptr,
        )
    }
}

/// Timer notify function invoked after the recovery delay to re-submit async
/// interrupt transfers following an error.
extern "efiapi" fn recovery_timer_notify(_event: efi::Event, context: *mut UsbHidDevice) {
    if context.is_null() {
        return;
    }
    // SAFETY: context is a valid UsbHidDevice pointer set during event creation.
    let device = unsafe { &mut *context };
    if let Err(status) = initiate_async_interrupt_input_transfers(device) {
        log::warn!("USB HID: recovery re-submit failed: {status:x?}");
    }
}

/// Initiates input reports from the endpoint by scheduling an async interrupt
/// transfer to poll the device.
pub fn initiate_async_interrupt_input_transfers(device: &mut UsbHidDevice) -> Result<(), efi::Status> {
    // SAFETY: usb_io was opened BY_DRIVER and is valid for the device's lifetime.
    let usb_io = unsafe { &*device.usb_io };

    // SAFETY: usb_io was opened BY_DRIVER and is valid; transfer parameters are valid.
    let status = unsafe {
        (usb_io.usb_async_interrupt_transfer)(
            device.usb_io,
            device.descriptors.int_in_endpoint_descriptor.endpoint_address,
            true,
            device.descriptors.int_in_endpoint_descriptor.interval as usize,
            device.descriptors.int_in_endpoint_descriptor.max_packet_size as usize,
            Some(on_report_interrupt_complete),
            device as *mut UsbHidDevice as *mut c_void,
        )
    };
    if status != efi::Status::SUCCESS {
        return Err(status);
    }

    Ok(())
}

/// Shuts down async interrupt transfers.
pub fn shutdown_async_interrupt_input_transfers(device: &mut UsbHidDevice) -> Result<(), efi::Status> {
    // SAFETY: usb_io is valid for the device's lifetime.
    let usb_io = unsafe { &*device.usb_io };

    // Cancel the async interrupt transfer.
    // SAFETY: usb_io is valid; cancellation parameters are valid.
    let status = unsafe {
        (usb_io.usb_async_interrupt_transfer)(
            device.usb_io,
            device.descriptors.int_in_endpoint_descriptor.endpoint_address,
            false,
            0,
            0,
            None,
            core::ptr::null_mut(),
        )
    };
    if status != efi::Status::SUCCESS && status != efi::Status::NOT_FOUND {
        log::warn!("USB HID: unexpected error shutting down async transfer: {status:x?}");
    }

    Ok(())
}

/// Interrupt completion callback. Invoked by the USB bus driver when data
/// arrives on the interrupt-in endpoint (or on error).
///
/// # Safety
///
/// `context` must be a valid pointer to a [`UsbHidDevice`] that was set
/// during transfer initiation. `data` must be valid for `data_length` bytes
/// when `result` indicates success.
unsafe extern "efiapi" fn on_report_interrupt_complete(
    data: *mut c_void,
    data_length: usize,
    context: *mut c_void,
    result: u32,
) -> efi::Status {
    if context.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }

    // SAFETY: context is a pointer to UsbHidDevice set during transfer initiation.
    let device = unsafe { &mut *(context as *mut UsbHidDevice) };

    if result != EFI_USB_NOERROR {
        // Handle stall by clearing the endpoint halt.
        if (result & EFI_USB_ERR_STALL) != 0 {
            // SAFETY: usb_io is valid for the device's lifetime.
            let usb_io = unsafe { &*device.usb_io };
            let _ = control_transfers::usb_clear_endpoint_halt(
                usb_io,
                device.descriptors.int_in_endpoint_descriptor.endpoint_address,
            );
        }

        // Cancel the current async transfer and re-submit.
        // SAFETY: usb_io is valid for the device's lifetime.
        let usb_io = unsafe { &*device.usb_io };
        // SAFETY: usb_io is valid; cancelling the current async transfer.
        let _ = unsafe {
            (usb_io.usb_async_interrupt_transfer)(
                device.usb_io,
                device.descriptors.int_in_endpoint_descriptor.endpoint_address,
                false,
                0,
                0,
                None,
                core::ptr::null_mut(),
            )
        };

        // Arm the recovery timer for delayed re-submission.
        let _ = device.timer_services.arm_recovery_timer(device.recovery_event, RECOVERY_DELAY_100NS);

        return efi::Status::DEVICE_ERROR;
    }

    if data_length > u16::MAX as usize {
        return efi::Status::DEVICE_ERROR;
    }

    if data_length == 0 || data.is_null() {
        return efi::Status::SUCCESS;
    }

    if let Some(callback) = device.report_callback.callback {
        // SAFETY: callback was registered via HidIo protocol; data and context are valid.
        unsafe { callback(data_length as u16, data, device.report_callback.context) };
    }

    efi::Status::SUCCESS
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;
    use core::{
        cell::Cell,
        sync::atomic::{AtomicU16, AtomicUsize, Ordering},
    };

    use patina::uefi_protocol::usb_io::{
        EfiAsyncUsbTransferCallback, EfiUsbDataDirection, EfiUsbDeviceRequest, EfiUsbIoProtocol,
    };

    use crate::{
        device::{ReportCallbackState, UsbHidDescriptors, UsbHidDevice},
        hid_io_impl,
    };

    // ---- Mock USB IO ----

    /// Mock USB IO context. `protocol` must be the first field so extern mock
    /// functions can recover mock state from the `this` pointer.
    #[repr(C)]
    struct MockUsbIo {
        protocol: EfiUsbIoProtocol,
        control_transfer_status: efi::Status,
        async_transfer_status: efi::Status,
        async_call_count: Cell<usize>,
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
        mock.async_call_count.set(mock.async_call_count.get() + 1);
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
            async_call_count: Cell::new(0),
            control_call_count: Cell::new(0),
        }
    }

    // ---- No-op timer for tests that don't exercise recovery ----

    struct NoopTransferRecoveryTimer;
    impl TransferRecoveryTimer for NoopTransferRecoveryTimer {
        fn arm_recovery_timer(&self, _event: efi::Event, _delay_100ns: u64) -> Result<(), efi::Status> {
            Ok(())
        }
    }
    static NOOP_RECOVERY_TIMER: NoopTransferRecoveryTimer = NoopTransferRecoveryTimer;

    fn make_device(usb_io: &MockUsbIo) -> Box<UsbHidDevice> {
        Box::new(UsbHidDevice {
            hid_io: hid_io_impl::new_hid_io_protocol(),
            usb_io: &usb_io.protocol as *const EfiUsbIoProtocol,
            descriptors: UsbHidDescriptors {
                interface_descriptor: EfiUsbInterfaceDescriptor::default(),
                int_in_endpoint_descriptor: EfiUsbEndpointDescriptor {
                    endpoint_address: 0x81,
                    interval: 10,
                    max_packet_size: 8,
                    ..Default::default()
                },
                report_descriptor: vec![0x05, 0x01],
            },
            report_callback: ReportCallbackState::default(),
            timer_services: &NOOP_RECOVERY_TIMER,
            recovery_event: core::ptr::null_mut(),
        })
    }

    // ---- initiate tests ----

    #[test]
    fn initiate_transfers_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        assert!(initiate_async_interrupt_input_transfers(&mut device).is_ok());
        assert_eq!(mock_usb.async_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn initiate_transfers_returns_error_on_failure() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::DEVICE_ERROR);
        let mut device = make_device(&mock_usb);
        assert_eq!(initiate_async_interrupt_input_transfers(&mut device), Err(efi::Status::DEVICE_ERROR),);
        core::mem::forget(device);
    }

    // ---- shutdown tests ----

    #[test]
    fn shutdown_transfers_succeeds() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        assert!(shutdown_async_interrupt_input_transfers(&mut device).is_ok());
        assert_eq!(mock_usb.async_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn shutdown_transfers_tolerates_not_found() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::NOT_FOUND);
        let mut device = make_device(&mock_usb);
        assert!(shutdown_async_interrupt_input_transfers(&mut device).is_ok());
        core::mem::forget(device);
    }

    #[test]
    fn shutdown_transfers_tolerates_unexpected_error() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::DEVICE_ERROR);
        let mut device = make_device(&mock_usb);
        // shutdown always returns Ok even on unexpected errors (it just logs a warning).
        assert!(shutdown_async_interrupt_input_transfers(&mut device).is_ok());
        core::mem::forget(device);
    }

    // ---- on_report_interrupt_complete tests ----

    #[test]
    fn callback_null_context_returns_invalid_parameter() {
        let report = [0u8; 8];
        // SAFETY: testing null context handling.
        let status = unsafe {
            on_report_interrupt_complete(
                report.as_ptr() as *mut c_void,
                report.len(),
                core::ptr::null_mut(),
                EFI_USB_NOERROR,
            )
        };
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn callback_usb_error_returns_device_error_and_arms_timer() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(
                core::ptr::null_mut(),
                0,
                device_ptr as *mut c_void,
                0x01, // non-zero = error, but not stall
            )
        };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        // Should have called async transfer once (cancel only; re-submit is via recovery timer).
        assert_eq!(mock_usb.async_call_count.get(), 1);
        // Should not have called control transfer (no stall).
        assert_eq!(mock_usb.control_call_count.get(), 0);
        core::mem::forget(device);
    }

    #[test]
    fn callback_stall_error_clears_halt_and_arms_timer() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(core::ptr::null_mut(), 0, device_ptr as *mut c_void, EFI_USB_ERR_STALL)
        };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        // Should have called control transfer to clear endpoint halt.
        assert_eq!(mock_usb.control_call_count.get(), 1);
        // Should have called async transfer once (cancel only; re-submit is via recovery timer).
        assert_eq!(mock_usb.async_call_count.get(), 1);
        core::mem::forget(device);
    }

    #[test]
    fn callback_data_length_exceeds_u16_max_returns_device_error() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(
                0x1000 as *mut c_void, // non-null
                u16::MAX as usize + 1,
                device_ptr as *mut c_void,
                EFI_USB_NOERROR,
            )
        };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        core::mem::forget(device);
    }

    #[test]
    fn callback_zero_length_returns_success() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(core::ptr::null_mut(), 0, device_ptr as *mut c_void, EFI_USB_NOERROR)
        };
        assert_eq!(status, efi::Status::SUCCESS);
        core::mem::forget(device);
    }

    #[test]
    fn callback_null_data_returns_success() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(core::ptr::null_mut(), 8, device_ptr as *mut c_void, EFI_USB_NOERROR)
        };
        assert_eq!(status, efi::Status::SUCCESS);
        core::mem::forget(device);
    }

    #[test]
    fn callback_no_registered_callback_returns_success() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        let report = [0xAAu8; 4];
        // No callback registered — should silently succeed.
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(
                report.as_ptr() as *mut c_void,
                report.len(),
                device_ptr as *mut c_void,
                EFI_USB_NOERROR,
            )
        };
        assert_eq!(status, efi::Status::SUCCESS);
        core::mem::forget(device);
    }

    // Shared atomic counters for the test callback below.
    static CALLBACK_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);
    static CALLBACK_REPORT_SIZE: AtomicU16 = AtomicU16::new(0);

    unsafe extern "efiapi" fn counting_callback(
        report_buffer_size: u16,
        _report_buffer: *mut c_void,
        _context: *mut c_void,
    ) {
        CALLBACK_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
        CALLBACK_REPORT_SIZE.store(report_buffer_size, Ordering::SeqCst);
    }

    #[test]
    fn callback_invokes_registered_callback_with_report() {
        CALLBACK_INVOCATIONS.store(0, Ordering::SeqCst);
        CALLBACK_REPORT_SIZE.store(0, Ordering::SeqCst);

        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mut device = make_device(&mock_usb);
        device.report_callback.callback = Some(counting_callback);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        let report = [0x10u8, 0x20, 0x30];
        // SAFETY: device_ptr is a valid UsbHidDevice; report is valid.
        let status = unsafe {
            on_report_interrupt_complete(
                report.as_ptr() as *mut c_void,
                report.len(),
                device_ptr as *mut c_void,
                EFI_USB_NOERROR,
            )
        };
        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(CALLBACK_INVOCATIONS.load(Ordering::SeqCst), 1);
        assert_eq!(CALLBACK_REPORT_SIZE.load(Ordering::SeqCst), 3);
        core::mem::forget(device);
    }

    // ---- Timer-based recovery tests ----

    /// Mock timer services for testing delayed recovery.
    struct MockTransferRecoveryTimer {
        arm_called: Cell<bool>,
        arm_event: Cell<Option<efi::Event>>,
        arm_delay: Cell<u64>,
    }

    impl MockTransferRecoveryTimer {
        fn new() -> Self {
            Self { arm_called: Cell::new(false), arm_event: Cell::new(None), arm_delay: Cell::new(0) }
        }
    }

    impl TransferRecoveryTimer for MockTransferRecoveryTimer {
        fn arm_recovery_timer(&self, event: efi::Event, delay_100ns: u64) -> Result<(), efi::Status> {
            self.arm_called.set(true);
            self.arm_event.set(Some(event));
            self.arm_delay.set(delay_100ns);
            Ok(())
        }
    }

    fn make_device_with_timer(usb_io: &MockUsbIo, timer_services: &MockTransferRecoveryTimer) -> Box<UsbHidDevice> {
        // SAFETY: The timer_services reference is transmuted to 'static for storage in
        // UsbHidDevice. The test ensures the MockTransferRecoveryTimer outlives the device.
        let timer_ref: &'static dyn TransferRecoveryTimer =
            unsafe { core::mem::transmute(timer_services as &dyn TransferRecoveryTimer) };
        let sentinel_event = 0xBEEF as efi::Event;
        Box::new(UsbHidDevice {
            hid_io: hid_io_impl::new_hid_io_protocol(),
            usb_io: &usb_io.protocol as *const EfiUsbIoProtocol,
            descriptors: UsbHidDescriptors {
                interface_descriptor: EfiUsbInterfaceDescriptor::default(),
                int_in_endpoint_descriptor: EfiUsbEndpointDescriptor {
                    endpoint_address: 0x81,
                    interval: 10,
                    max_packet_size: 8,
                    ..Default::default()
                },
                report_descriptor: vec![0x05, 0x01],
            },
            report_callback: ReportCallbackState::default(),
            timer_services: timer_ref,
            recovery_event: sentinel_event,
        })
    }

    #[test]
    fn callback_error_arms_recovery_timer_instead_of_immediate_resubmit() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mock_timer = MockTransferRecoveryTimer::new();
        let mut device = make_device_with_timer(&mock_usb, &mock_timer);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe { on_report_interrupt_complete(core::ptr::null_mut(), 0, device_ptr as *mut c_void, 0x01) };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        // Should have called async transfer once (cancel only, no immediate re-submit).
        assert_eq!(mock_usb.async_call_count.get(), 1);
        // Recovery timer should have been armed.
        assert!(mock_timer.arm_called.get());
        assert_eq!(mock_timer.arm_event.get(), Some(0xBEEF as efi::Event));
        assert_eq!(mock_timer.arm_delay.get(), RECOVERY_DELAY_100NS);
        core::mem::forget(device);
    }

    #[test]
    fn callback_stall_error_with_timer_clears_halt_and_arms_timer() {
        let mock_usb = make_mock_usb_io(efi::Status::SUCCESS, efi::Status::SUCCESS);
        let mock_timer = MockTransferRecoveryTimer::new();
        let mut device = make_device_with_timer(&mock_usb, &mock_timer);
        let device_ptr = &mut *device as *mut UsbHidDevice;
        // SAFETY: device_ptr is a valid UsbHidDevice.
        let status = unsafe {
            on_report_interrupt_complete(core::ptr::null_mut(), 0, device_ptr as *mut c_void, EFI_USB_ERR_STALL)
        };
        assert_eq!(status, efi::Status::DEVICE_ERROR);
        // Should have cleared endpoint halt via control transfer.
        assert_eq!(mock_usb.control_call_count.get(), 1);
        // Should have called async transfer once (cancel only).
        assert_eq!(mock_usb.async_call_count.get(), 1);
        // Recovery timer should have been armed.
        assert!(mock_timer.arm_called.get());
        core::mem::forget(device);
    }
}
