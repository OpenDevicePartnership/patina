//! USB HID driver binding implementation.
//!
//! The [`UsbHidDriver`] implements [`patina::driver_binding::DriverBinding`] to
//! manage USB HID devices. It consumes USB IO and produces HidIo.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::boxed::Box;
use core::{ffi::c_void, ptr::NonNull};

use r_efi::{efi, protocols::device_path::Protocol as EfiDevicePathProtocol};

use patina::{boot_services::BootServices, driver_binding::DriverBinding};

use patina::{
    uefi_protocol::usb_io::{EfiUsbIoProtocol, USB_IO_PROTOCOL_GUID, types::*},
    vendor_protocols::hid_io,
};

use crate::{descriptors, device::UsbHidDevice, hid_io_impl, transfers, usb_requests};
use patina::boot_services::event::EventTimerType;

/// USB HID driver that implements [`DriverBinding`].
pub struct UsbHidDriver {
    agent: efi::Handle,
}

impl UsbHidDriver {
    /// Creates a new USB HID driver bound to the given agent handle.
    pub fn new(agent: efi::Handle) -> Self {
        Self { agent }
    }
}

/// Checks whether the controller has USB IO protocol with HID interface class.
fn is_usb_hid(usb_io: &EfiUsbIoProtocol) -> bool {
    let mut interface_descriptor = EfiUsbInterfaceDescriptor::default();
    // SAFETY: usb_io and interface_descriptor are valid.
    let status =
        unsafe { (usb_io.usb_get_interface_descriptor)(usb_io as *const EfiUsbIoProtocol, &mut interface_descriptor) };
    if status != efi::Status::SUCCESS {
        return false;
    }

    interface_descriptor.interface_class == CLASS_HID
}

// efi::Handle is an opaque *mut c_void that is never actually dereferenced as a pointer.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
// FFI wrapper trait; core logic is tested elsewhere via unit tests on individual modules.
#[coverage(off)]
impl DriverBinding for UsbHidDriver {
    /// Tests if the given controller has USB IO with HID interface class.
    fn driver_binding_supported<U: BootServices + 'static>(
        &self,
        boot_services: &'static U,
        controller: efi::Handle,
        _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<bool, efi::Status> {
        // SAFETY: EfiUsbIoProtocol layout matches the USB IO GUID.
        let usb_io = match unsafe {
            boot_services.open_protocol::<EfiUsbIoProtocol>(
                controller,
                self.agent,
                controller,
                efi::OPEN_PROTOCOL_BY_DRIVER,
            )
        } {
            Ok(usb_io) => usb_io,
            Err(_) => return Ok(false),
        };

        let result = is_usb_hid(usb_io);

        boot_services.close_protocol(controller, USB_IO_PROTOCOL_GUID.as_efi_guid(), self.agent, controller).ok();

        Ok(result)
    }

    /// Starts USB HID support for the given controller.
    fn driver_binding_start<U: BootServices + 'static>(
        &mut self,
        boot_services: &'static U,
        controller: efi::Handle,
        _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<(), efi::Status> {
        log::trace!("USB HID: driver_binding_start on controller {:?}", controller);

        // Open USB IO BY_DRIVER for exclusive access.
        // SAFETY: EfiUsbIoProtocol layout matches the USB IO GUID.
        let usb_io = unsafe {
            boot_services.open_protocol::<EfiUsbIoProtocol>(
                controller,
                self.agent,
                controller,
                efi::OPEN_PROTOCOL_BY_DRIVER,
            )
        }?;

        // Read descriptors from the device.
        let descriptors = match descriptors::read_descriptors(usb_io) {
            Ok(d) => d,
            Err(status) => {
                log::error!("USB HID: failed to read descriptors: {status:x?}");
                self.close_usb_io(boot_services, controller);
                return Err(status);
            }
        };

        // Boot devices: explicitly set report protocol mode.
        if descriptors.interface_descriptor.interface_sub_class == SUBCLASS_BOOT
            && let Err(status) = usb_requests::set_protocol_request(
                usb_io,
                descriptors.interface_descriptor.interface_number,
                REPORT_PROTOCOL,
            )
        {
            log::warn!("USB HID: failed to set report protocol: {status:x?}");
        }

        // Build the device context and leak it for UEFI protocol ownership.
        let device_ptr = Box::into_raw(Box::new(UsbHidDevice {
            hid_io: hid_io_impl::new_hid_io_protocol(),
            usb_io: usb_io as *const EfiUsbIoProtocol,
            descriptors,
            report_callback: crate::device::ReportCallbackState::default(),
            timer_services: boot_services as &'static dyn transfers::TimerServices,
            recovery_event: core::ptr::null_mut(),
        }));

        // Create a recovery timer event for delayed re-submission on transfer errors.
        // SAFETY: device_ptr is a valid heap-allocated UsbHidDevice that will outlive
        // the event (closed in stop before the device is freed).
        match unsafe { transfers::create_recovery_event(boot_services, device_ptr) } {
            Ok(event) => {
                // SAFETY: device_ptr is valid; setting the pre-allocated field.
                unsafe { (*device_ptr).recovery_event = event };
            }
            Err(status) => {
                log::error!("USB HID: failed to create recovery event: {status:x?}");
                // SAFETY: Reclaiming the Box we leaked above.
                drop(unsafe { Box::from_raw(device_ptr) });
                self.close_usb_io(boot_services, controller);
                return Err(status);
            }
        }

        // Install HidIo protocol on the controller.
        // hid_io is the first field in #[repr(C)] UsbHidDevice, so device_ptr == hid_io_ptr.
        // SAFETY: Installing HidIo protocol interface on the controller handle.
        if let Err(status) = unsafe {
            boot_services.install_protocol_interface_unchecked(
                Some(controller),
                &hid_io::HID_IO_PROTOCOL_GUID,
                device_ptr as *mut c_void,
            )
        } {
            log::error!("USB HID: failed to install HidIo protocol: {status:x?}");
            // SAFETY: Reclaiming the Box we leaked above; close recovery event first.
            let device = unsafe { Box::from_raw(device_ptr) };
            let _ = boot_services.close_event(device.recovery_event);
            drop(device);
            self.close_usb_io(boot_services, controller);
            return Err(status);
        }

        Ok(())
    }

    /// Stops USB HID support for the given controller.
    fn driver_binding_stop<U: BootServices + 'static>(
        &mut self,
        boot_services: &'static U,
        controller: efi::Handle,
        _number_of_children: usize,
        _child_handle_buffer: Option<NonNull<efi::Handle>>,
    ) -> Result<(), efi::Status> {
        log::trace!("USB HID: driver_binding_stop on controller {:?}", controller);

        // Retrieve the HidIo protocol to recover the device.
        // SAFETY: HidIo protocol was installed by start.
        let hid_io_ptr = unsafe {
            boot_services.open_protocol_unchecked(
                controller,
                &hid_io::HID_IO_PROTOCOL_GUID,
                self.agent,
                controller,
                efi::OPEN_PROTOCOL_GET_PROTOCOL,
            )
        }? as *const hid_io::HidIoProtocol;

        // SAFETY: hid_io_ptr points into a valid heap-allocated UsbHidDevice.
        let device = unsafe { UsbHidDevice::from_hid_io_protocol(hid_io_ptr) };

        // Cancel the recovery timer and close the event before shutting down transfers.
        let _ = boot_services.set_timer(device.recovery_event, EventTimerType::Cancel, 0);
        let _ = boot_services.close_event(device.recovery_event);

        // Shutdown async transfers if active.
        let _ = transfers::shutdown_async_interrupt_input_transfers(device);

        // Uninstall HidIo protocol.
        // SAFETY: Uninstalling the HidIo protocol interface.
        if let Err(status) = unsafe {
            boot_services.uninstall_protocol_interface_unchecked(
                controller,
                &hid_io::HID_IO_PROTOCOL_GUID,
                hid_io_ptr as *mut c_void,
            )
        } {
            log::error!("USB HID: failed to uninstall HidIo: {status:x?}");
            return Err(status);
        }

        // SAFETY: device was created via Box::into_raw in start.
        drop(unsafe { Box::from_raw(device as *mut UsbHidDevice) });

        // Close USB IO protocol.
        self.close_usb_io(boot_services, controller);

        Ok(())
    }
}

impl UsbHidDriver {
    fn close_usb_io(&self, boot_services: &impl BootServices, controller: efi::Handle) {
        if let Err(status) =
            boot_services.close_protocol(controller, USB_IO_PROTOCOL_GUID.as_efi_guid(), self.agent, controller)
        {
            log::error!("USB HID: error closing USB IO protocol: {status:x?}");
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use patina::boot_services::MockBootServices;

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| patina::boot_services::tpl::Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        // SAFETY: Leaked mock for test use with 'static lifetime requirement.
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    #[test]
    fn supported_returns_false_when_no_usb_io() {
        let boot_services = mock_boot_services();
        boot_services.expect_open_protocol::<EfiUsbIoProtocol>().returning(|_, _, _, _| Err(efi::Status::NOT_FOUND));

        let driver = UsbHidDriver::new(0x1 as efi::Handle);
        assert_eq!(driver.driver_binding_supported(boot_services, 0x2 as efi::Handle, None), Ok(false));
    }
}
