//! DXE Core implementation of [`ProtocolServices`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_void;
#[cfg(feature = "unstable-device-path")]
use core::ptr::NonNull;

use patina::BinaryGuid;
use patina::component::service::{
    IntoService,
    uefi_services::protocol::{
        Handle, NotifyCallback, NotifyRegistration, ProtocolError, ProtocolPtr, ProtocolServices, Tpl,
    },
};
use patina::error::EfiError;
use patina::standard::efi;
#[cfg(feature = "unstable-device-path")]
use patina::uefi::device_path::paths::{DevicePath, DevicePathBuf};

use crate::events::EVENT_DB;
#[cfg(feature = "unstable-device-path")]
use crate::protocols::core_locate_device_path;
use crate::protocols::{PROTOCOL_DB, core_install_protocol_interface, core_uninstall_protocol_interface};

use super::event::tpl_to_efi;
use super::state::UEFI_SERVICES_STATE;

/// Owns a component-supplied installation-notify closure for the lifetime of a registration (until cancelled).
struct NotifyHolder {
    callback: NotifyCallback,
    registration: *mut c_void,
}

/// C-ABI trampoline signaled when a watched protocol is installed.
///
/// Drains the handles that the registration has flagged as installed and invokes the
/// component-supplied closure for each.
extern "efiapi" fn notify_install_trampoline(_event: efi::Event, context: *mut c_void) {
    if context.is_null() {
        return;
    }

    UEFI_SERVICES_STATE.protocol_install_notify.enter(context);

    // SAFETY: `context` is the `NotifyHolder` created in `register_install_notify`, valid until
    // freed below or by `cancel_install_notify`. Notifications dispatch serially at the event's TPL.
    let holder = unsafe { &mut *context.cast::<NotifyHolder>() };
    while let Some(handle) = PROTOCOL_DB.next_handle_for_registration(holder.registration) {
        if let Some(handle) = Handle::from_raw(handle) {
            (holder.callback)(handle);
        }
    }

    if UEFI_SERVICES_STATE.protocol_install_notify.exit(context) {
        // The dispatch cancelled its own registration above, so free the holder now.
        // SAFETY: `context` has not been freed yet, `cancel_install_notify` only recorded the request.
        drop(unsafe { Box::from_raw(context.cast::<NotifyHolder>()) });
    }
}

/// Core implementation of [`ProtocolServices`], delegating to the core protocol database through the
/// internal `core_*` Rust APIs.
#[derive(IntoService)]
#[service(dyn ProtocolServices)]
pub(crate) struct CoreProtocolServices;

impl ProtocolServices for CoreProtocolServices {
    fn install_interface(
        &self,
        handle: Option<Handle>,
        protocol: BinaryGuid,
        interface: ProtocolPtr,
    ) -> Result<Handle, ProtocolError> {
        let caller_handle = handle.map(|handle| handle.as_raw());
        let installed = core_install_protocol_interface(caller_handle, protocol.into_inner(), interface.as_raw())
            .map_err(ProtocolError::from)?;
        Handle::from_raw(installed).ok_or(ProtocolError::Internal)
    }

    fn uninstall_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        interface: ProtocolPtr,
    ) -> Result<(), ProtocolError> {
        core_uninstall_protocol_interface(handle.as_raw(), protocol.into_inner(), interface.as_raw())
            .map_err(ProtocolError::from)
    }

    fn install_marker(&self, handle: Option<Handle>, protocol: BinaryGuid) -> Result<Handle, ProtocolError> {
        let caller_handle = handle.map(|handle| handle.as_raw());
        let installed = core_install_protocol_interface(caller_handle, protocol.into_inner(), core::ptr::null_mut())
            .map_err(ProtocolError::from)?;
        Handle::from_raw(installed).ok_or(ProtocolError::Internal)
    }

    fn uninstall_marker(&self, handle: Handle, protocol: BinaryGuid) -> Result<(), ProtocolError> {
        core_uninstall_protocol_interface(handle.as_raw(), protocol.into_inner(), core::ptr::null_mut())
            .map_err(ProtocolError::from)
    }

    fn locate_interface(&self, protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError> {
        let interface = PROTOCOL_DB.locate_protocol(protocol.into_inner()).map_err(ProtocolError::from)?;
        ProtocolPtr::from_raw(interface).ok_or(ProtocolError::NotFound)
    }

    fn locate_handles(&self, protocol: BinaryGuid) -> Result<Vec<Handle>, ProtocolError> {
        // "No handles present" is not an error, report it as an empty list so callers can iterate
        // without special-casing absence.
        match PROTOCOL_DB.locate_handles(Some(protocol.into_inner())) {
            Ok(handles) => Ok(handles.into_iter().filter_map(Handle::from_raw).collect()),
            Err(EfiError::NotFound) => Ok(Vec::new()),
            Err(err) => Err(ProtocolError::from(err)),
        }
    }

    #[cfg(feature = "unstable-device-path")]
    fn locate_device_path(
        &self,
        protocol: BinaryGuid,
        path: &DevicePath,
    ) -> Result<(Handle, DevicePathBuf), ProtocolError> {
        let ptr = NonNull::new(path.as_bytes().as_ptr() as *mut efi::protocols::device_path::Protocol)
            .ok_or(ProtocolError::InvalidParameter)?;
        let (remaining, handle) = core_locate_device_path(protocol.into_inner(), ptr).map_err(ProtocolError::from)?;
        let handle = Handle::from_raw(handle).ok_or(ProtocolError::Internal)?;
        // SAFETY: `remaining` is returned by `core_locate_device_path` and points within `path`'s
        // buffer, which is valid for the duration of this call.
        let remaining = unsafe { DevicePath::try_from_ptr(remaining.as_ptr() as *const u8) }
            .map_err(|_| ProtocolError::Internal)?;
        Ok((handle, DevicePathBuf::from(remaining)))
    }

    fn interface_on_handle(&self, handle: Handle, protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError> {
        let interface = PROTOCOL_DB
            .get_interface_for_handle(handle.as_raw(), protocol.into_inner())
            .map_err(ProtocolError::from)?;
        ProtocolPtr::from_raw(interface).ok_or(ProtocolError::NotFound)
    }

    fn register_install_notify(
        &self,
        protocol: BinaryGuid,
        notify_tpl: Tpl,
        callback: NotifyCallback,
    ) -> Result<NotifyRegistration, ProtocolError> {
        // `efi::EVT_NOTIFY_SIGNAL` requires the notification TPL to be <= Tpl::Notify, so Tpl::HighLevel is invalid.
        if notify_tpl > Tpl::Notify {
            return Err(ProtocolError::InvalidParameter);
        }

        let holder = Box::new(NotifyHolder { callback, registration: core::ptr::null_mut() });
        let context = Box::into_raw(holder).cast::<c_void>();

        // Create a notify-signal event whose closure drains newly installed handles.
        let event = match EVENT_DB.create_event(
            efi::EVT_NOTIFY_SIGNAL,
            tpl_to_efi(notify_tpl),
            Some(notify_install_trampoline),
            Some(context),
            None,
        ) {
            Ok(event) => event,
            Err(err) => {
                // SAFETY: `context` came from `Box::into_raw` above and has not been freed.
                drop(unsafe { Box::from_raw(context.cast::<NotifyHolder>()) });
                return Err(ProtocolError::from(err));
            }
        };

        let registration = match PROTOCOL_DB.register_protocol_notify(protocol.into_inner(), event) {
            Ok(registration) => registration,
            Err(err) => {
                let _ = EVENT_DB.close_event(event);
                // SAFETY: `context` came from `Box::into_raw` above and has not been freed.
                drop(unsafe { Box::from_raw(context.cast::<NotifyHolder>()) });
                return Err(ProtocolError::from(err));
            }
        };

        // Record the registration key so the trampoline can drain matching handles. This runs
        // before any notification can fire, since queued notifications dispatch only once the TPL
        // drops below TPL_CALLBACK after this call returns.
        // SAFETY: `context` is the `NotifyHolder` pointer created above and has not been freed.
        unsafe {
            (*context.cast::<NotifyHolder>()).registration = registration;
        }

        // Deliver handles that already have the protocol installed.
        let _ = EVENT_DB.signal_event(event);

        Ok(NotifyRegistration::from_raw(event, registration, context))
    }

    fn cancel_install_notify(&self, registration: NotifyRegistration) -> Result<(), ProtocolError> {
        let event = registration.event();
        let context = registration.context();

        PROTOCOL_DB.unregister_protocol_notify_events(vec![event]);

        EVENT_DB.close_event(event).map_err(ProtocolError::from)?;

        // This registration is cancelling itself, or an ancestor registration if cancelled from a
        // nested notification. The notify install trampoline frees the holder when the dispatch
        // loop returns.
        if !context.is_null() && !UEFI_SERVICES_STATE.protocol_install_notify.request_close(context) {
            // SAFETY: `context` is the `NotifyHolder` created in `register_install_notify`. The
            // event is now closed and unregistered, and its dispatch (if any) is not on the
            // call stack, so nothing can reference it any further.
            drop(unsafe { Box::from_raw(context.cast::<NotifyHolder>()) });
        }

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{events::restore_tpl, test_support};
    use core::str::FromStr;
    use std::{cell::RefCell, rc::Rc};
    use uuid::Uuid;

    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_clean_global_lock(|| {
            test_support::init_test_logger();
            // Bring the shared TPL back to TPL_APPLICATION so `EVENT_DB.signal_event` (used by
            // `register_install_notify`) dispatches queued notifications synchronously within this
            // test, regardless of what an earlier test left it at.
            restore_tpl(efi::TPL_APPLICATION);
            f();
        })
        .unwrap();
    }

    fn test_guid(uuid_str: &str) -> BinaryGuid {
        BinaryGuid::from_bytes(Uuid::from_str(uuid_str).unwrap().as_bytes())
    }

    fn fake_interface(addr: usize) -> ProtocolPtr {
        ProtocolPtr::from_raw(addr as *mut c_void).unwrap()
    }

    #[test]
    fn test_protocol_services_install_interface_creates_new_handle() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("11111111-1111-1111-1111-111111111111");

            let handle = service.install_interface(None, guid, fake_interface(0x1000)).unwrap();

            assert!(!handle.as_raw().is_null());
        });
    }

    #[test]
    fn test_protocol_services_install_interface_reuses_provided_handle() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid_a = test_guid("11111111-2222-1111-1111-111111111111");
            let guid_b = test_guid("22222222-3333-2222-2222-222222222222");

            let handle = service.install_interface(None, guid_a, fake_interface(0x1000)).unwrap();
            let same_handle = service.install_interface(Some(handle), guid_b, fake_interface(0x2000)).unwrap();

            assert_eq!(handle, same_handle);
        });
    }

    #[test]
    fn test_protocol_services_uninstall_interface_removes_protocol() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("33333333-3333-3333-3333-333333333333");
            let interface = fake_interface(0x1000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            service.uninstall_interface(handle, guid, interface).unwrap();

            assert_eq!(service.locate_interface(guid), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_uninstall_interface_mismatched_pointer_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("44444444-4444-4444-4444-444444444444");

            let handle = service.install_interface(None, guid, fake_interface(0x1000)).unwrap();
            let result = service.uninstall_interface(handle, guid, fake_interface(0x2000));

            assert_eq!(result, Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_install_marker_creates_new_handle() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("17171717-1717-1717-1717-171717171717");

            let handle = service.install_marker(None, guid).unwrap();

            assert!(!handle.as_raw().is_null());
        });
    }

    #[test]
    fn test_protocol_services_install_marker_is_discoverable_via_locate_handles() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("18181818-1818-1818-1818-181818181818");

            let handle = service.install_marker(None, guid).unwrap();

            assert_eq!(service.locate_handles(guid).unwrap(), vec![handle]);
        });
    }

    #[test]
    fn test_protocol_services_uninstall_marker_removes_it() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("19191919-1919-1919-1919-191919191919");

            let handle = service.install_marker(None, guid).unwrap();
            service.uninstall_marker(handle, guid).unwrap();

            assert!(service.locate_handles(guid).unwrap().is_empty());
        });
    }

    #[test]
    fn test_protocol_services_locate_interface_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("55555555-5555-5555-5555-555555555555");

            assert_eq!(service.locate_interface(guid), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_locate_interface_returns_installed_interface() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("66666666-6666-6666-6666-666666666666");
            let interface = fake_interface(0x3000);

            service.install_interface(None, guid, interface).unwrap();

            assert_eq!(service.locate_interface(guid).unwrap(), interface);
        });
    }

    #[test]
    fn test_protocol_services_locate_interface_null_interface_reports_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("77777777-7777-7777-7777-777777777777");

            // A protocol can legitimately be installed with a null interface.
            // `ProtocolPtr` cannot represent a null interface, so this reports NotFound even
            //though PROTOCOL_DB has an entry for it.
            PROTOCOL_DB.install_protocol_interface(None, guid.into_inner(), core::ptr::null_mut()).unwrap();

            assert_eq!(service.locate_interface(guid), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_locate_handles_empty_when_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("88888888-8888-8888-8888-888888888888");

            assert!(service.locate_handles(guid).unwrap().is_empty());
        });
    }

    #[test]
    fn test_protocol_services_locate_handles_returns_installed_handles() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("99999999-9999-9999-9999-999999999999");

            let handle_a = service.install_interface(None, guid, fake_interface(0x1000)).unwrap();
            let handle_b = service.install_interface(None, guid, fake_interface(0x2000)).unwrap();

            let handles = service.locate_handles(guid).unwrap();

            assert_eq!(handles.len(), 2);
            assert!(handles.contains(&handle_a));
            assert!(handles.contains(&handle_b));
        });
    }

    #[cfg(feature = "unstable-device-path")]
    #[test]
    fn test_protocol_services_locate_device_path_returns_handle_and_remaining_path() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("dddddddd-dddd-dddd-dddd-dddddddddddd");

            // Install `guid` and a device path protocol (the handle's "root" path) on the same handle.
            let handle = service.install_interface(None, guid, fake_interface(0x6000)).unwrap();
            let root_bytes = patina::devpath!("PciRoot(0)");
            let root_device_path_ptr =
                Box::into_raw(Box::new(root_bytes)).cast::<efi::protocols::device_path::Protocol>();
            core_install_protocol_interface(
                Some(handle.as_raw()),
                efi::protocols::device_path::PROTOCOL_GUID,
                root_device_path_ptr.cast::<c_void>(),
            )
            .unwrap();

            // Query with the root path plus an extra Pci(0x11,0) node.
            let query_bytes = patina::devpath!("PciRoot(0)/Pci(0x11,0)");
            // SAFETY: `query_bytes` is a valid, well-formed device path byte buffer.
            let query_path = unsafe { DevicePath::try_from_ptr(query_bytes.as_ptr()) }.unwrap();

            let (located, remaining) = service.locate_device_path(guid, query_path).unwrap();

            assert_eq!(located, handle);
            // Only the Pci(0x11,0) node (plus the end node) should remain.
            assert_eq!(remaining.node_count(), 2);
        });
    }

    #[cfg(feature = "unstable-device-path")]
    #[test]
    fn test_protocol_services_locate_device_path_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee");

            let query_bytes = patina::devpath!("PciRoot(0)/Pci(0x11,0)");
            // SAFETY: `query_bytes` is a valid, well-formed device path byte buffer.
            let query_path = unsafe { DevicePath::try_from_ptr(query_bytes.as_ptr()) }.unwrap();

            assert_eq!(service.locate_device_path(guid, query_path), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_interface_on_handle_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
            let interface = fake_interface(0x4000);

            let handle = service.install_interface(None, guid, interface).unwrap();

            assert_eq!(service.interface_on_handle(handle, guid).unwrap(), interface);
        });
    }

    #[test]
    fn test_protocol_services_interface_on_handle_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let installed_guid = test_guid("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb");
            let other_guid = test_guid("cccccccc-cccc-cccc-cccc-cccccccccccc");

            let handle = service.install_interface(None, installed_guid, fake_interface(0x5000)).unwrap();

            assert_eq!(service.interface_on_handle(handle, other_guid), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_register_install_notify_fires_for_already_installed_handle() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("dddddddd-dddd-dddd-dddd-dddddddddddd");
            let handle = service.install_interface(None, guid, fake_interface(0x6000)).unwrap();

            let seen: Rc<RefCell<Vec<Handle>>> = Rc::new(RefCell::new(Vec::new()));
            let recorder = Rc::clone(&seen);
            let registration = service
                .register_install_notify(guid, Tpl::Callback, Box::new(move |h| recorder.borrow_mut().push(h)))
                .unwrap();

            // The handle already had the protocol installed, so the callback fires synchronously
            // as part of registration, before any future install occurs.
            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0], handle);

            service.cancel_install_notify(registration).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_register_install_notify_fires_for_future_install() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee");

            let seen: Rc<RefCell<Vec<Handle>>> = Rc::new(RefCell::new(Vec::new()));
            let recorder = Rc::clone(&seen);
            let registration = service
                .register_install_notify(guid, Tpl::Callback, Box::new(move |h| recorder.borrow_mut().push(h)))
                .unwrap();

            // Nothing was installed for this protocol at registration time.
            assert!(seen.borrow().is_empty());

            let handle = service.install_interface(None, guid, fake_interface(0x7000)).unwrap();

            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0], handle);

            service.cancel_install_notify(registration).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_register_install_notify_fires_for_already_installed_marker() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("20202020-2020-2020-2020-202020202020");
            let handle = service.install_marker(None, guid).unwrap();

            let seen: Rc<RefCell<Vec<Handle>>> = Rc::new(RefCell::new(Vec::new()));
            let recorder = Rc::clone(&seen);
            let registration = service
                .register_install_notify(guid, Tpl::Callback, Box::new(move |h| recorder.borrow_mut().push(h)))
                .unwrap();

            // Note: A marker protocol works the same way as a regular protocol for install notifications.
            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0], handle);

            service.cancel_install_notify(registration).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_register_install_notify_fires_for_future_marker_install() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("21212121-2121-2121-2121-212121212121");

            let seen: Rc<RefCell<Vec<Handle>>> = Rc::new(RefCell::new(Vec::new()));
            let recorder = Rc::clone(&seen);
            let registration = service
                .register_install_notify(guid, Tpl::Callback, Box::new(move |h| recorder.borrow_mut().push(h)))
                .unwrap();

            assert!(seen.borrow().is_empty());

            let handle = service.install_marker(None, guid).unwrap();

            assert_eq!(seen.borrow().len(), 1);
            assert_eq!(seen.borrow()[0], handle);

            service.cancel_install_notify(registration).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_register_install_notify_rejects_high_level_tpl() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("16161616-1616-1616-1616-161616161616");

            let result = service.register_install_notify(guid, Tpl::HighLevel, Box::new(|_h| {}));

            assert_eq!(result.err(), Some(ProtocolError::InvalidParameter));
        });
    }

    #[test]
    fn test_protocol_services_cancel_install_notify_stops_future_notifications() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("ffffffff-ffff-ffff-ffff-ffffffffffff");

            let count = Rc::new(RefCell::new(0usize));
            let counter = Rc::clone(&count);
            let registration = service
                .register_install_notify(guid, Tpl::Callback, Box::new(move |_h| *counter.borrow_mut() += 1))
                .unwrap();

            service.cancel_install_notify(registration).unwrap();
            service.install_interface(None, guid, fake_interface(0x8000)).unwrap();

            assert_eq!(*count.borrow(), 0);
        });
    }

    #[test]
    fn test_protocol_services_cancel_install_notify_frees_context() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("12121212-1212-1212-1212-121212121212");

            // The closure's capture is the only thing that can prove the boxed `NotifyHolder` (and
            // this `Rc`) was actually dropped, rather than leaked, by `cancel_install_notify`.
            let marker = Rc::new(());
            let captured = Rc::clone(&marker);
            let registration = service
                .register_install_notify(
                    guid,
                    Tpl::Callback,
                    Box::new(move |_h| {
                        let _ = &captured;
                    }),
                )
                .unwrap();

            assert_eq!(Rc::strong_count(&marker), 2);

            service.cancel_install_notify(registration).unwrap();

            assert_eq!(Rc::strong_count(&marker), 1);
        });
    }

    #[test]
    fn test_protocol_services_cancel_install_notify_invalid_registration() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            // An event value that was never created by `EVENT_DB`, so closing it must fail.
            let bogus =
                NotifyRegistration::from_raw(0x7FFF_FFFF as *mut c_void, core::ptr::null_mut(), core::ptr::null_mut());

            assert_eq!(service.cancel_install_notify(bogus), Err(ProtocolError::InvalidParameter));
        });
    }

    #[test]
    fn test_protocol_services_cancel_install_notify_from_within_own_callback_defers_free() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("13131313-1313-1313-1313-131313131313");

            // The callback needs its own `NotifyRegistration` to cancel itself, but that value is
            // only known after `register_install_notify` returns, so it is threaded through after.
            let self_registration: Rc<RefCell<Option<NotifyRegistration>>> = Rc::new(RefCell::new(None));
            let self_registration_in_callback = Rc::clone(&self_registration);

            // The closure's capture is the only thing that can prove the boxed `NotifyHolder` (and
            // this `Rc`) was actually dropped, and only after the callback finished running.
            let marker = Rc::new(());
            let captured = Rc::clone(&marker);
            let ran_after_self_cancel = Rc::new(RefCell::new(0usize));
            let ran_after_self_cancel_in_callback = Rc::clone(&ran_after_self_cancel);

            let registration = service
                .register_install_notify(
                    guid,
                    Tpl::Callback,
                    Box::new(move |_handle| {
                        let _ = &captured;
                        if let Some(reg) = self_registration_in_callback.borrow_mut().take() {
                            assert_eq!(CoreProtocolServices.cancel_install_notify(reg), Ok(()));
                        }
                        // If the closure's own captured state had been freed by the call above,
                        // touching captured state here would be a use-after-free.
                        *ran_after_self_cancel_in_callback.borrow_mut() += 1;
                    }),
                )
                .unwrap();
            self_registration.replace(Some(registration));

            // Nothing was installed for this protocol at registration time, so the callback (and
            // its nested self-cancel) only runs once this install signals the registration's event.
            service.install_interface(None, guid, fake_interface(0x9000)).unwrap();

            assert_eq!(*ran_after_self_cancel.borrow(), 1);
            assert_eq!(Rc::strong_count(&marker), 1);
        });
    }

    #[test]
    fn test_protocol_services_cancel_install_notify_from_nested_notification_defers_free() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid_a = test_guid("14141414-1414-1414-1414-141414141414");
            let guid_b = test_guid("15151515-1515-1515-1515-151515151515");

            let registration_b_slot: Rc<RefCell<Option<NotifyRegistration>>> = Rc::new(RefCell::new(None));
            let registration_b_slot_for_a = Rc::clone(&registration_b_slot);

            // Registration A (higher TPL) cancels registration B when it fires.
            let registration_a = service
                .register_install_notify(
                    guid_a,
                    Tpl::Notify,
                    Box::new(move |_handle| {
                        let reg_b = registration_b_slot_for_a
                            .borrow_mut()
                            .take()
                            .expect("registration B set before triggering A");
                        assert_eq!(CoreProtocolServices.cancel_install_notify(reg_b), Ok(()));
                    }),
                )
                .unwrap();

            let marker = Rc::new(());
            let captured = Rc::clone(&marker);
            let ran_after_nested_cancel = Rc::new(RefCell::new(0usize));
            let ran_after_nested_cancel_in_b = Rc::clone(&ran_after_nested_cancel);

            // Registration B (lower TPL) installs protocol A from its own callback, which nests
            // A's dispatch (and B's cancellation) inside B's still-running callback.
            let registration_b = service
                .register_install_notify(
                    guid_b,
                    Tpl::Callback,
                    Box::new(move |_handle| {
                        let _ = &captured;
                        CoreProtocolServices.install_interface(None, guid_a, fake_interface(0xA000)).unwrap();
                        // Registration B was cancelled above, nested inside this call, while this
                        // callback (and its captured state) was still running.
                        *ran_after_nested_cancel_in_b.borrow_mut() += 1;
                    }),
                )
                .unwrap();
            registration_b_slot.replace(Some(registration_b));

            service.install_interface(None, guid_b, fake_interface(0xB000)).unwrap();

            assert_eq!(*ran_after_nested_cancel.borrow(), 1);
            assert_eq!(Rc::strong_count(&marker), 1);

            service.cancel_install_notify(registration_a).unwrap();
        });
    }
}
