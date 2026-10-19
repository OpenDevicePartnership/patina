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
        Handle, NotifyCallback, NotifyRegistration, OpenAttributes, ProtocolError, ProtocolPtr, ProtocolServices, Tpl,
    },
};
use patina::error::EfiError;
use patina::standard::efi;
#[cfg(feature = "unstable-device-path")]
use patina::uefi::device_path::paths::{DevicePath, DevicePathBuf};

use crate::events::EVENT_DB;
#[cfg(feature = "unstable-device-path")]
use crate::protocols::core_locate_device_path;
use crate::protocols::{
    PROTOCOL_DB, core_close_protocol, core_install_protocol_interface, core_open_protocol,
    core_uninstall_protocol_interface,
};

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

const PRIVATE_AGENT_MARKER_GUID: BinaryGuid = BinaryGuid::from_string("7B6F3A21-9E4D-4C88-B2A5-1D8C6F0E3A97");

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

    fn open_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        agent: Handle,
        attributes: OpenAttributes,
    ) -> Result<ProtocolPtr, ProtocolError> {
        let raw_handle = handle.as_raw();
        let guid = protocol.into_inner();
        let raw_agent = Some(agent.as_raw());
        let raw_controller = attributes.controller(handle).map(|controller| controller.as_raw());

        // SAFETY: `raw_handle`/`raw_agent`/`raw_controller` come from validated `Handle` values.
        // The implicit UEFI spec assumption that driver bindings remain valid for the duration of
        // the call cannot be verified here (see `core_disconnect_controller`'s safety documentation).
        let interface = unsafe { core_open_protocol(raw_handle, guid, raw_agent, raw_controller, attributes.bits()) }
            .map_err(ProtocolError::from)?;

        ProtocolPtr::from_raw(interface).ok_or(ProtocolError::NotFound)
    }

    fn close_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        agent: Handle,
        controller: Option<Handle>,
    ) -> Result<(), ProtocolError> {
        core_close_protocol(
            handle.as_raw(),
            protocol.into_inner(),
            agent.as_raw(),
            controller.map(|controller| controller.as_raw()),
        )
        .map_err(ProtocolError::from)
    }

    fn register_agent(&self) -> Result<Handle, ProtocolError> {
        self.install_marker(None, PRIVATE_AGENT_MARKER_GUID)
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
    use patina::component::service::uefi_services::{driver_binding::DriverBinding, protocol::ProtocolServicesExt};
    use patina::uefi::device_path::walker::BorrowedDevicePathWalker;
    use std::{cell::RefCell, rc::Rc};
    use uuid::Uuid;

    const BY_DRIVER: OpenAttributes = OpenAttributes::ByDriver { controller: None };
    const BY_DRIVER_EXCLUSIVE: OpenAttributes = OpenAttributes::ByDriverExclusive { controller: None };

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
    fn test_protocol_services_open_interface_returns_installed_interface() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("f87243cb-d5f6-4538-9475-c50bb7acb405");
            let interface = fake_interface(0x7000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            let opened = service.open_interface(handle, guid, agent, OpenAttributes::Shared).unwrap();

            assert_eq!(opened, interface);
            service.close_interface(handle, guid, agent, None).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_open_interface_missing_protocol_is_unsupported() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let installed_guid = test_guid("ae667572-4cce-4812-a0ba-e258599a7b83");
            let other_guid = test_guid("368e9e2f-e4fa-449c-8f22-a9a4c25361e5");

            let handle = service.install_interface(None, installed_guid, fake_interface(0x8000)).unwrap();
            let agent = service.register_agent().unwrap();

            assert_eq!(
                service.open_interface(handle, other_guid, agent, OpenAttributes::Shared),
                Err(ProtocolError::Unsupported)
            );
        });
    }

    #[test]
    fn test_protocol_services_close_interface_without_open_returns_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("531575e8-5dfa-4102-ad6a-913d952a383d");

            let handle = service.install_interface(None, guid, fake_interface(0x9000)).unwrap();
            let agent = service.register_agent().unwrap();

            assert_eq!(service.close_interface(handle, guid, agent, None), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_open_interface_usage_is_advisory_not_blocking() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("92d405ac-731b-4d52-8fc7-352b3c25259e");
            let interface = fake_interface(0xa000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::Shared).unwrap();

            // The usage is recorded and visible...
            let usages =
                PROTOCOL_DB.get_open_protocol_information_by_protocol(handle.as_raw(), guid.into_inner()).unwrap();
            assert_eq!(usages.len(), 1);

            // A `GET_PROTOCOL` usage does not block uninstall: `UninstallProtocolInterface` clears
            // `GET_PROTOCOL`/`BY_HANDLE_PROTOCOL`/`TEST_PROTOCOL` usages. Only `BY_DRIVER` usages can
            // block removal, when a `DisconnectController` attempt that may fail.
            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_open_interface_exclusive_blocks_uninstall_until_closed() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("f9ca168c-d0e7-4c79-84cb-ea4df9938077");
            let interface = fake_interface(0xb000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::Exclusive).unwrap();

            // `Exclusive` is not force-cleared by uninstall (unlike `Shared`).
            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));

            service.close_interface(handle, guid, agent, None).unwrap();
            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_open_interface_exclusive_different_agent_denied() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("2d50afa0-5ac9-4f19-ac8a-37dea0668196");
            let handle = service.install_interface(None, guid, fake_interface(0xb100)).unwrap();
            let agent_a = service.register_agent().unwrap();
            let agent_b = service.register_agent().unwrap();

            service.open_interface(handle, guid, agent_a, OpenAttributes::Exclusive).unwrap();

            assert_eq!(
                service.open_interface(handle, guid, agent_b, OpenAttributes::Exclusive),
                Err(ProtocolError::AccessDenied)
            );
        });
    }

    #[test]
    fn test_protocol_services_open_interface_by_driver_blocks_uninstall_without_driver_binding() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("fe0b9ba0-5bfd-4fd6-8c09-dd009f65b1b8");
            let interface = fake_interface(0xb200);

            let handle = service.install_interface(None, guid, interface).unwrap();
            // A simple call to register_agent handle has no driver binding installed, so DisconnectController
            // cannot find a Stop() to call and the release is permanently denied.
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, BY_DRIVER).unwrap();

            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));
        });
    }

    #[test]
    fn test_protocol_services_open_interface_by_driver_reopen_same_agent_returns_already_started() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("afc9e470-2eb1-4279-b457-a1daf26259e5");
            let interface = fake_interface(0xb300);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            let attributes = BY_DRIVER;
            service.open_interface(handle, guid, agent, attributes).unwrap();

            // The original usage is unaffected by the rejected reopen.
            assert_eq!(service.open_interface(handle, guid, agent, attributes), Err(ProtocolError::AlreadyStarted));
            let usages =
                PROTOCOL_DB.get_open_protocol_information_by_protocol(handle.as_raw(), guid.into_inner()).unwrap();
            assert_eq!(usages.len(), 1);
        });
    }

    #[test]
    fn test_protocol_services_open_interface_by_driver_different_agent_denied() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("88a54816-7034-407f-8bb4-de94fd9be3b0");
            let handle = service.install_interface(None, guid, fake_interface(0xb400)).unwrap();
            let agent_a = service.register_agent().unwrap();
            let agent_b = service.register_agent().unwrap();

            service.open_interface(handle, guid, agent_a, BY_DRIVER).unwrap();

            assert_eq!(service.open_interface(handle, guid, agent_b, BY_DRIVER), Err(ProtocolError::AccessDenied));
        });
    }

    /// A test driver binding instance that can be configured to either allow or deny a `Stop()` call,
    /// to test behavior when uninstalling a protocol that has a `BY_DRIVER` usage.
    struct StopWith {
        guid: BinaryGuid,
        should_stop: bool,
    }

    impl DriverBinding for StopWith {
        fn start(
            &self,
            _agent: Handle,
            _controller: Handle,
            _remaining: Option<BorrowedDevicePathWalker>,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }

        fn stop(&self, agent: Handle, controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
            if !self.should_stop {
                return Err(ProtocolError::AccessDenied);
            }
            CoreProtocolServices.close_interface(controller, self.guid, agent, Some(controller))
        }
    }

    #[test]
    fn test_protocol_services_uninstall_releases_by_driver_when_stop_returns_ok() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("fb57ed13-d572-4001-8be2-a67ae92bc8df");
            let interface = fake_interface(0xb500);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            service.open_interface(handle, guid, agent, BY_DRIVER).unwrap();

            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_uninstall_blocked_by_driver_when_stop_returns_err() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("34549404-9c92-479c-909e-8711a07d28d8");
            let interface = fake_interface(0xb600);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.install_driver_binding(StopWith { guid, should_stop: false }).unwrap();
            service.open_interface(handle, guid, agent, BY_DRIVER).unwrap();

            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));
        });
    }

    #[test]
    fn test_protocol_services_by_driver_exclusive_preempts_holder_with_driver_binding() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("567aca28-2230-452d-a734-6c44d248e752");
            let handle = service.install_interface(None, guid, fake_interface(0xb700)).unwrap();

            let holder = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            service.open_interface(handle, guid, holder, BY_DRIVER).unwrap();

            let preempting = service.register_agent().unwrap();
            // ByDriverExclusive should preempt the existing ByDriver holder, since its Stop() allows release.
            service.open_interface(handle, guid, preempting, BY_DRIVER_EXCLUSIVE).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_by_driver_exclusive_fails_when_holder_has_no_driver_binding() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("965c4543-6507-465c-9d04-3fb005bb8756");
            let handle = service.install_interface(None, guid, fake_interface(0xb800)).unwrap();

            let holder = service.register_agent().unwrap();
            service.open_interface(handle, guid, holder, BY_DRIVER).unwrap();

            let preempting = service.register_agent().unwrap();
            assert_eq!(
                service.open_interface(handle, guid, preempting, BY_DRIVER_EXCLUSIVE),
                Err(ProtocolError::AccessDenied)
            );
        });
    }

    #[test]
    fn test_protocol_services_by_child_controller_same_handle_invalid_parameter() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("66ef06ec-35a4-4820-b78a-a948aac22033");
            let handle = service.install_interface(None, guid, fake_interface(0xb900)).unwrap();
            let agent = service.register_agent().unwrap();

            assert_eq!(
                service.open_interface(handle, guid, agent, OpenAttributes::ByChildController { controller: handle }),
                Err(ProtocolError::InvalidParameter)
            );
        });
    }

    #[test]
    fn test_protocol_services_by_child_controller_visible_via_get_child_handles() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("5199113e-b1e4-4cbd-821d-f7d67ade3270");
            let parent = service.install_interface(None, guid, fake_interface(0xba00)).unwrap();
            let child = service.install_interface(None, guid, fake_interface(0xbb00)).unwrap();
            let agent = service.register_agent().unwrap();

            service
                .open_interface(parent, guid, agent, OpenAttributes::ByChildController { controller: child })
                .unwrap();

            let children = PROTOCOL_DB.get_child_handles(parent.as_raw());
            assert_eq!(children, alloc::vec![child.as_raw()]);
        });
    }

    #[test]
    fn test_protocol_services_register_agent_returns_distinct_handles() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let agent_a = service.register_agent().unwrap();
            let agent_b = service.register_agent().unwrap();

            assert_ne!(agent_a, agent_b);
        });
    }

    #[test]
    fn test_protocol_services_install_driver_binding_returns_distinct_handles() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("78c3dc9b-3d97-404a-a198-0a96b5678b83");
            let handle_a = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            let handle_b = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();

            assert_ne!(handle_a, handle_b);
        });
    }

    #[test]
    fn test_protocol_services_install_driver_binding_installs_driver_binding_protocol() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("7982a0ba-ff85-43e9-a740-87873ae549f3");
            let handle = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();

            let interface = PROTOCOL_DB
                .get_interface_for_handle(handle.as_raw(), efi::protocols::driver_binding::PROTOCOL_GUID)
                .unwrap();
            let protocol = interface as *const efi::protocols::driver_binding::Protocol;
            // SAFETY: `install_driver_binding` just installed a valid `DriverBindingHolder` at this
            // address, whose first field is the `Protocol` struct itself.
            let protocol = unsafe { &*protocol };
            assert_eq!(protocol.driver_binding_handle, handle.as_raw());
            assert_eq!(protocol.image_handle, handle.as_raw());
        });
    }

    #[test]
    fn test_protocol_services_install_driver_binding_on_existing_agent_serves_by_driver_stop() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("a62be7d2-2493-4f6b-944a-2cb1438562f4");
            let interface = fake_interface(0xbc00);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, BY_DRIVER).unwrap();

            // Without a driver binding on the agent, the ByDriver usage cannot be released.
            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));

            service.install_driver_binding_on(agent, StopWith { guid, should_stop: true }).unwrap();
            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_install_driver_binding_on_agent_twice_is_invalid_parameter() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("324064b2-cdde-4d50-ad55-750cc44424d8");
            let agent = service.register_agent().unwrap();

            service.install_driver_binding_on(agent, StopWith { guid, should_stop: true }).unwrap();

            assert_eq!(
                service.install_driver_binding_on(agent, StopWith { guid, should_stop: true }),
                Err(ProtocolError::InvalidParameter)
            );
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
