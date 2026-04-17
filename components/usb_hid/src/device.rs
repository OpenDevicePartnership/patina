//! Per-device state for USB HID devices.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use alloc::vec::Vec;
use core::ffi::c_void;

use r_efi::efi;

use patina::vendor_protocols::hid_io::{HidIoProtocol, HidIoReportCallback};

use patina::uefi_protocol::usb_io::{
    EfiUsbIoProtocol,
    types::{EfiUsbEndpointDescriptor, EfiUsbInterfaceDescriptor},
};

use crate::interrupt_transfers::TransferRecoveryTimer;

/// USB HID descriptor set read from the device during initialization.
#[derive(Debug)]
pub struct UsbHidDescriptors {
    pub interface_descriptor: EfiUsbInterfaceDescriptor,
    pub int_in_endpoint_descriptor: EfiUsbEndpointDescriptor,
    pub report_descriptor: Vec<u8>,
}

/// Registered callback state for asynchronous input report notifications.
#[derive(Default)]
pub struct ReportCallbackState {
    pub callback: Option<HidIoReportCallback>,
    pub context: *mut c_void,
}

/// Per-device context for a USB HID device managed by this driver.
///
/// Allocated on the heap during `driver_binding_start` and freed during
/// `driver_binding_stop`. The `hid_io` field is installed as a protocol
/// interface on the controller handle.
#[repr(C)]
pub struct UsbHidDevice {
    // Note: a direct cast is used to recover the UsbHidDevice pointer from the HidIoProtocol pointer, so hid_io must be
    // the first field.
    pub hid_io: HidIoProtocol,
    pub usb_io: *const EfiUsbIoProtocol,
    pub descriptors: UsbHidDescriptors,
    pub report_callback: ReportCallbackState,
    /// Boot services timer interface for delayed error recovery.
    pub(crate) timer_services: &'static dyn TransferRecoveryTimer,
    /// Timer event armed by the interrupt callback on transfer errors. The event's
    /// notify function re-submits the async interrupt transfer after a delay.
    pub(crate) recovery_event: efi::Event,
}

impl UsbHidDevice {
    /// Recovers a raw pointer to the `UsbHidDevice` from a pointer to its `hid_io` field.
    ///
    /// This is a pure pointer cast (no dereference) and is therefore safe. The
    /// caller is responsible for ensuring the returned pointer is valid before
    /// dereferencing it — `hid_io_ptr` must point to the `hid_io` field of a
    /// heap-allocated `UsbHidDevice`, and the `hid_io` field must be the first
    /// field in the `#[repr(C)]` layout.
    pub fn from_hid_io_protocol(hid_io_ptr: *const HidIoProtocol) -> *mut Self {
        hid_io_ptr as *mut UsbHidDevice
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::hid_io_impl;

    #[test]
    fn from_hid_io_protocol_recovers_device() {
        struct NoopTimer;
        impl crate::interrupt_transfers::TransferRecoveryTimer for NoopTimer {
            fn arm_recovery_timer(&self, _: efi::Event, _: u64) -> Result<(), efi::Status> {
                Ok(())
            }
        }
        static NOOP: NoopTimer = NoopTimer;

        let device = Box::new(UsbHidDevice {
            hid_io: hid_io_impl::new_hid_io_protocol(),
            usb_io: core::ptr::null(),
            descriptors: UsbHidDescriptors {
                interface_descriptor: EfiUsbInterfaceDescriptor::default(),
                int_in_endpoint_descriptor: EfiUsbEndpointDescriptor::default(),
                report_descriptor: Vec::new(),
            },
            report_callback: ReportCallbackState::default(),
            timer_services: &NOOP,
            recovery_event: core::ptr::null_mut(),
        });

        let hid_io_ptr = &device.hid_io as *const HidIoProtocol;
        let recovered = UsbHidDevice::from_hid_io_protocol(hid_io_ptr);
        assert_eq!(recovered as usize, &*device as *const _ as usize);

        // Prevent drop from double-freeing the leaked box.
        core::mem::forget(device);
    }
}
