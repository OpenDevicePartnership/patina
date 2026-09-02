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

use patina::BinaryGuid;
use patina::component::service::{
    IntoService,
    uefi_services::protocol::{
        Handle, NotifyCallback, NotifyRegistration, OpenAttributes, ProtocolError, ProtocolPtr, ProtocolServices, Tpl,
    },
};
use patina::error::EfiError;
use patina::standard::efi;

use crate::events::EVENT_DB;
use crate::protocols::{
    PROTOCOL_DB, core_close_protocol, core_install_protocol_interface, core_open_protocol,
    core_uninstall_protocol_interface,
};

use super::event::tpl_to_efi;

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
    // SAFETY: `context` is the `NotifyHolder` created in `register_install_notify`, valid until
    // `cancel_install_notify` reclaims it. Notifications dispatch serially at the event's TPL.
    let holder = unsafe { &mut *(context as *mut NotifyHolder) };
    while let Some(handle) = PROTOCOL_DB.next_handle_for_registration(holder.registration) {
        if let Some(handle) = Handle::from_raw(handle) {
            (holder.callback)(handle);
        }
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
        let raw_controller = attributes.controller().map(|controller| controller.as_raw());

        // SAFETY: `raw_handle`/`raw_agent`/`raw_controller` come from validated `Handle` values.
        // The implicit UEFI spec assumption that driver bindings remain valid for the duration of
        // the call cannot be verified here (see `core_disconnect_controller`'s safety documentation).
        let (interface, _already_started) =
            match unsafe { core_open_protocol(raw_handle, guid, raw_agent, raw_controller, attributes.bits()) } {
                Ok(result) => result,
                Err(EfiError::Unsupported) => return Err(ProtocolError::NotFound),
                Err(err) => return Err(ProtocolError::from(err)),
            };

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
        let raw = core_install_protocol_interface(None, *PRIVATE_AGENT_MARKER_GUID, core::ptr::null_mut())
            .map_err(ProtocolError::from)?;
        Handle::from_raw(raw).ok_or(ProtocolError::Internal)
    }

    fn register_install_notify(
        &self,
        protocol: BinaryGuid,
        notify_tpl: Tpl,
        callback: NotifyCallback,
    ) -> Result<NotifyRegistration, ProtocolError> {
        let holder = Box::new(NotifyHolder { callback, registration: core::ptr::null_mut() });
        let context = Box::into_raw(holder) as *mut c_void;

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
                drop(unsafe { Box::from_raw(context as *mut NotifyHolder) });
                return Err(ProtocolError::from(err));
            }
        };

        let registration = match PROTOCOL_DB.register_protocol_notify(protocol.into_inner(), event) {
            Ok(registration) => registration,
            Err(err) => {
                let _ = EVENT_DB.close_event(event);
                // SAFETY: `context` came from `Box::into_raw` above and has not been freed.
                drop(unsafe { Box::from_raw(context as *mut NotifyHolder) });
                return Err(ProtocolError::from(err));
            }
        };

        // Record the registration key so the trampoline can drain matching handles. This runs
        // before any notification can fire, since queued notifications dispatch only once the TPL
        // drops below TPL_CALLBACK after this call returns.
        // SAFETY: `context` is the `NotifyHolder` pointer created above and has not been freed.
        unsafe {
            (*(context as *mut NotifyHolder)).registration = registration;
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

        if !context.is_null() {
            // SAFETY: `context` is the `NotifyHolder` created in `register_install_notify`. The
            // event is now closed and unregistered, so no further notification can reference it.
            drop(unsafe { Box::from_raw(context as *mut NotifyHolder) });
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
            let guid = test_guid("11111111-2222-3333-4444-555555555556");
            let interface = fake_interface(0x7000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            let opened = service.open_interface(handle, guid, agent, OpenAttributes::Shared).unwrap();

            assert_eq!(opened, interface);
            service.close_interface(handle, guid, agent, None).unwrap();
        });
    }

    #[test]
    fn test_protocol_services_open_interface_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let installed_guid = test_guid("22222222-3333-4444-5555-666666666667");
            let other_guid = test_guid("33333333-4444-5555-6666-777777777778");

            let handle = service.install_interface(None, installed_guid, fake_interface(0x8000)).unwrap();
            let agent = service.register_agent().unwrap();

            assert_eq!(
                service.open_interface(handle, other_guid, agent, OpenAttributes::Shared),
                Err(ProtocolError::NotFound)
            );
        });
    }

    #[test]
    fn test_protocol_services_close_interface_without_open_returns_not_found() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("44444444-5555-6666-7777-888888888889");

            let handle = service.install_interface(None, guid, fake_interface(0x9000)).unwrap();
            let agent = service.register_agent().unwrap();

            assert_eq!(service.close_interface(handle, guid, agent, None), Err(ProtocolError::NotFound));
        });
    }

    #[test]
    fn test_protocol_services_open_interface_usage_is_advisory_not_blocking() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("55555555-6666-7777-8888-99999999999a");
            let interface = fake_interface(0xa000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::Shared).unwrap();

            // The usage is recorded and visible...
            let usages =
                PROTOCOL_DB.get_open_protocol_information_by_protocol(handle.as_raw(), guid.into_inner()).unwrap();
            assert_eq!(usages.len(), 1);

            // ...but a `GET_PROTOCOL` usage does not block uninstall: `UninstallProtocolInterface`
            // force-clears `GET_PROTOCOL`/`BY_HANDLE_PROTOCOL`/`TEST_PROTOCOL` usages itself. Only
            // `BY_DRIVER` usages can block removal, when a `DisconnectController` attempt that may fail.
            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_open_interface_exclusive_blocks_uninstall_until_closed() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("11111111-aaaa-bbbb-cccc-111111111111");
            let interface = fake_interface(0xb000);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::Exclusive).unwrap();

            // `Exclusive` is not force-cleared by uninstall (Unlike `Shared`).
            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));

            service.close_interface(handle, guid, agent, None).unwrap();
            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_open_interface_exclusive_different_agent_denied() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("22222222-aaaa-bbbb-cccc-222222222222");
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
            let guid = test_guid("33333333-aaaa-bbbb-cccc-333333333333");
            let interface = fake_interface(0xb200);

            let handle = service.install_interface(None, guid, interface).unwrap();
            // A simple call to register_agent handle has no driver binding installed, so DisconnectController
            // cannot find a Stop() to call and the release is permanently denied.
            let agent = service.register_agent().unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::ByDriver { controller: handle }).unwrap();

            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));
        });
    }

    #[test]
    fn test_protocol_services_open_interface_by_driver_reopen_same_agent_returns_ok() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("44444444-aaaa-bbbb-cccc-444444444444");
            let interface = fake_interface(0xb300);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.register_agent().unwrap();
            let attributes = OpenAttributes::ByDriver { controller: handle };
            service.open_interface(handle, guid, agent, attributes).unwrap();

            // Reopening with the same agent/controller/attributes will return ALREADY_STARTED internally, but
            // open_interface should return Ok with the interface still returned.
            let reopened = service.open_interface(handle, guid, agent, attributes).unwrap();
            assert_eq!(reopened, interface);
        });
    }

    #[test]
    fn test_protocol_services_open_interface_by_driver_different_agent_denied() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("55555555-aaaa-bbbb-cccc-555555555555");
            let handle = service.install_interface(None, guid, fake_interface(0xb400)).unwrap();
            let agent_a = service.register_agent().unwrap();
            let agent_b = service.register_agent().unwrap();

            service.open_interface(handle, guid, agent_a, OpenAttributes::ByDriver { controller: handle }).unwrap();

            assert_eq!(
                service.open_interface(handle, guid, agent_b, OpenAttributes::ByDriver { controller: handle }),
                Err(ProtocolError::AccessDenied)
            );
        });
    }

    /// A test driver binding instance that can be configured to either allow or deny a `Stop()` call,
    /// to test behavior when uninstalling a protocol that has a `BY_DRIVER` usage.
    struct StopWith {
        guid: BinaryGuid,
        should_stop: bool,
    }

    impl DriverBinding for StopWith {
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
            let guid = test_guid("66666666-aaaa-bbbb-cccc-666666666666");
            let interface = fake_interface(0xb500);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::ByDriver { controller: handle }).unwrap();

            assert!(service.uninstall_interface(handle, guid, interface).is_ok());
        });
    }

    #[test]
    fn test_protocol_services_uninstall_blocked_by_driver_when_stop_returns_err() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("77777777-aaaa-bbbb-cccc-777777777777");
            let interface = fake_interface(0xb600);

            let handle = service.install_interface(None, guid, interface).unwrap();
            let agent = service.install_driver_binding(StopWith { guid, should_stop: false }).unwrap();
            service.open_interface(handle, guid, agent, OpenAttributes::ByDriver { controller: handle }).unwrap();

            assert_eq!(service.uninstall_interface(handle, guid, interface), Err(ProtocolError::AccessDenied));
        });
    }

    #[test]
    fn test_protocol_services_by_driver_exclusive_preempts_holder_with_driver_binding() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("88888888-aaaa-bbbb-cccc-888888888888");
            let handle = service.install_interface(None, guid, fake_interface(0xb700)).unwrap();

            let holder = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            service.open_interface(handle, guid, holder, OpenAttributes::ByDriver { controller: handle }).unwrap();

            let preempting = service.register_agent().unwrap();
            // ByDriverExclusive should preempt the existing ByDriver holder, since its Stop() allows release.
            service
                .open_interface(handle, guid, preempting, OpenAttributes::ByDriverExclusive { controller: handle })
                .unwrap();
        });
    }

    #[test]
    fn test_protocol_services_by_driver_exclusive_fails_when_holder_has_no_driver_binding() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("99999999-aaaa-bbbb-cccc-999999999999");
            let handle = service.install_interface(None, guid, fake_interface(0xb800)).unwrap();

            let holder = service.register_agent().unwrap();
            service.open_interface(handle, guid, holder, OpenAttributes::ByDriver { controller: handle }).unwrap();

            let preempting = service.register_agent().unwrap();
            assert_eq!(
                service.open_interface(
                    handle,
                    guid,
                    preempting,
                    OpenAttributes::ByDriverExclusive { controller: handle }
                ),
                Err(ProtocolError::AccessDenied)
            );
        });
    }

    #[test]
    fn test_protocol_services_by_child_controller_same_handle_invalid_parameter() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("aaaaaaaa-bbbb-cccc-dddd-aaaaaaaaaaab");
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
            let guid = test_guid("bbbbbbbb-cccc-dddd-eeee-bbbbbbbbbbbc");
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
            let guid = test_guid("c1c1c1c1-c2c2-c3c3-c4c4-c5c5c5c5c5c5");
            let handle_a = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();
            let handle_b = service.install_driver_binding(StopWith { guid, should_stop: true }).unwrap();

            assert_ne!(handle_a, handle_b);
        });
    }

    #[test]
    fn test_protocol_services_install_driver_binding_installs_driver_binding_protocol() {
        with_locked_state(|| {
            let service = CoreProtocolServices;
            let guid = test_guid("d1d1d1d1-d2d2-d3d3-d4d4-d5d5d5d5d5d5");
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
}
