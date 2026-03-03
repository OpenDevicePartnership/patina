//! HID driver binding implementation.
//!
//! The [`HidDriver`] implements [`patina::driver_binding::DriverBinding`] to
//! manage HID instances on controllers that support the HidIo protocol.
//!
//! When started on a controller, it creates a [`crate::hid_io::UefiHidIo`]
//! instance, instantiates keyboard and pointer receivers, initializes them
//! via `&dyn HidIo`, and starts report reception through the device.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::{boxed::Box, vec::Vec};
use core::{ffi::c_void, ptr::NonNull};

use r_efi::{efi, protocols::device_path::Protocol as EfiDevicePathProtocol};

use patina::{
    BinaryGuid, boot_services::BootServices, driver_binding::DriverBinding, uefi_protocol::ProtocolInterface,
};

use crate::hid_io::{HidReportReceiver, ReceiverFactory, UefiHidIo};
use crate::keyboard::KeyboardHidHandler;
use crate::pointer::PointerHidHandler;

/// Per-controller context installed as a private protocol to track the HID instance.
struct HidInstance<T: BootServices + Clone + 'static> {
    _device: UefiHidIo<T>,
}

// SAFETY: HidInstance GUID uniquely identifies this private protocol.
unsafe impl<T: BootServices + Clone + 'static> ProtocolInterface for HidInstance<T> {
    const PROTOCOL_GUID: efi::Guid = *BinaryGuid::from_string("0a87cfdb-c482-48e4-ade7-d9f99620e169").as_efi_guid();
}

impl<T: BootServices + Clone + 'static> HidInstance<T> {
    // Creates a new HidInstance wrapping the given device.
    fn new(device: UefiHidIo<T>) -> Self {
        Self { _device: device }
    }
}

/// HID driver that implements [`DriverBinding`].
///
/// Creates and manages HID instances on controllers that expose the HidIo
/// protocol. Directly constructs [`UefiHidIo`] devices and creates keyboard
/// and pointer handlers as report receivers.
pub struct HidDriver<T: BootServices + Clone + 'static> {
    boot_services: &'static T,
    agent: efi::Handle,
}

impl<T: BootServices + Clone + 'static> HidDriver<T> {
    /// Creates a new HID driver bound to the given agent handle.
    ///
    /// `agent` is the image handle for this driver, used for protocol operations.
    pub fn new(boot_services: &'static T, agent: efi::Handle) -> Self {
        Self { boot_services, agent }
    }

    // Creates factory functions for HID report receivers.
    fn new_receiver_factories(&self) -> Vec<ReceiverFactory> {
        let bs = self.boot_services;
        alloc::vec![
            Box::new(move |controller, hid_io| {
                Ok(PointerHidHandler::new(bs, controller, hid_io)? as Box<dyn HidReportReceiver>)
            }),
            Box::new(move |controller, hid_io| {
                Ok(KeyboardHidHandler::new(bs, controller, hid_io)? as Box<dyn HidReportReceiver>)
            }),
        ]
    }
}

// controller is an efi::Handle (raw pointer) from the DriverBinding trait. efi::Handle is defined as *mut c_void, but
// essentially an opaque type that happens to be a pointer. The unsafe deref warning will be resolved once latest
// r_efi with unsafe API is integrated.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
impl<T: BootServices + Clone + 'static> DriverBinding for HidDriver<T> {
    /// Tests if the given controller supports the HidIo protocol.
    fn driver_binding_supported<U: BootServices + 'static>(
        &self,
        _boot_services: &'static U,
        controller: efi::Handle,
        _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<bool, efi::Status> {
        Ok(UefiHidIo::supports(self.boot_services, self.agent, controller))
    }

    /// Starts HID support for the given controller.
    ///
    /// Creates a UefiHidIo device with keyboard and pointer receivers, and
    /// installs a private protocol to track the instance context.
    fn driver_binding_start<U: BootServices + 'static>(
        &mut self,
        boot_services: &'static U,
        controller: efi::Handle,
        _remaining_device_path: Option<NonNull<EfiDevicePathProtocol>>,
    ) -> Result<(), efi::Status> {
        log::trace!("driver_binding_start: starting HID on controller {:?}", controller);
        let device = UefiHidIo::new(self.boot_services, self.agent, controller, self.new_receiver_factories())?;

        let hid_instance = Box::new(HidInstance::new(device));
        boot_services.install_protocol_interface(Some(controller), hid_instance)?;

        Ok(())
    }

    /// Stops HID support for the given controller.
    ///
    /// Retrieves and drops the HID instance context, reclaiming all resources.
    fn driver_binding_stop<U: BootServices + 'static>(
        &mut self,
        boot_services: &'static U,
        controller: efi::Handle,
        _number_of_children: usize,
        _child_handle_buffer: Option<NonNull<efi::Handle>>,
    ) -> Result<(), efi::Status> {
        log::trace!("driver_binding_stop: stopping HID on controller {:?}", controller);
        // SAFETY: The private protocol was installed on this controller by start.
        let hid_instance = unsafe {
            boot_services.open_protocol_unchecked(
                controller,
                &HidInstance::<T>::PROTOCOL_GUID,
                self.agent,
                controller,
                efi::OPEN_PROTOCOL_GET_PROTOCOL,
            )
        }? as *mut HidInstance<T>;

        // SAFETY: Uninstalling our private protocol interface.
        if let Err(status) = unsafe {
            boot_services.uninstall_protocol_interface_unchecked(
                controller,
                &HidInstance::<T>::PROTOCOL_GUID,
                hid_instance as *mut c_void,
            )
        } {
            log::error!("hid::driver_binding_stop: failed to uninstall protocol: {status:x?}");
            return Err(status);
        }

        // SAFETY: hid_instance was created via Box::into_raw (through install_protocol_interface) in start.
        drop(unsafe { Box::from_raw(hid_instance) });
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::HidDriver;
    use crate::hid_io::protocol::HidIoProtocol;
    use patina::{boot_services::MockBootServices, driver_binding::DriverBinding};
    use r_efi::efi;

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| patina::boot_services::tpl::Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        // SAFETY: Leaked mock for test use with 'static lifetime requirement.
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    #[test]
    fn supported_returns_true_when_hid_io_present() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_open_protocol::<HidIoProtocol>()
            .withf_st(|controller, _, _, _| *controller == 0x3 as efi::Handle)
            .returning(|_, _, _, _| {
                // SAFETY: supports() never dereferences the protocol; zeroed is fine.
                Ok(HidIoProtocol::stub())
            });
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| Err(efi::Status::NOT_FOUND));

        let hid_driver = HidDriver::new(boot_services, 0x1 as efi::Handle);

        assert_eq!(hid_driver.driver_binding_supported(boot_services, 0x2 as efi::Handle, None), Ok(false));
        assert_eq!(hid_driver.driver_binding_supported(boot_services, 0x3 as efi::Handle, None), Ok(true));
    }

    #[test]
    fn start_returns_unsupported_with_no_receivers() {
        let boot_services = mock_boot_services();
        // open_protocol succeeds (protocol exists) but receiver factories fail → UNSUPPORTED.
        boot_services.expect_open_protocol::<HidIoProtocol>().returning(|_, _, _, _| Ok(HidIoProtocol::stub()));
        boot_services.expect_close_protocol().returning(|_, _, _, _| Ok(()));

        let mut hid_driver = HidDriver::new(boot_services, 0x1 as efi::Handle);

        // All receiver factories fail (stub protocol has no real descriptor) → UNSUPPORTED.
        assert_eq!(
            hid_driver.driver_binding_start(boot_services, 0x2 as efi::Handle, None),
            Err(efi::Status::UNSUPPORTED)
        );
    }
}
