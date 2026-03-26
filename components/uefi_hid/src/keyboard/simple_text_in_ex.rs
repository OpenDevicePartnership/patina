//! Simple Text In Ex Protocol FFI Support.
//!
//! This module manages the Simple Text In Ex FFI.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::boxed::Box;
use core::{ffi::c_void, ptr};

use r_efi::{efi, protocols};

use patina::{
    boot_services::{
        BootServices,
        c_ptr::PtrMetadata,
        event::{EventNotifyCallback, EventType},
        tpl::Tpl,
    },
    uefi_protocol::ProtocolInterface,
};

use super::KeyboardHidHandler;

/// FFI context for SimpleTextInputEx protocol.
///
/// A pointer to KeyboardHidHandler is included in the context so that it can be reclaimed in the simple_text_in_ex API
/// implementations. Mutual exclusion on the KeyboardHidHandler is provided by the TplMutex on its key_queue state.
///
/// The simple_text_in_ex protocol element must be the first element in the structure so that the full structure can be
/// recovered by simple casting.
#[repr(C)]
pub(crate) struct SimpleTextInExFfi<T: BootServices + Clone + 'static> {
    simple_text_in_ex: protocols::simple_text_input_ex::Protocol,
    pub(crate) boot_services: &'static T,
    pub(crate) keyboard_handler: *mut KeyboardHidHandler<T>,
}

// SAFETY: SimpleTextInExFfi<T> is #[repr(C)] with protocols::simple_text_input_ex::Protocol as its
// first field, so a pointer to SimpleTextInExFfi<T> is a valid pointer to Protocol per the
// first-field casting pattern.
unsafe impl<T: BootServices + Clone + 'static> ProtocolInterface for SimpleTextInExFfi<T> {
    const PROTOCOL_GUID: patina::BinaryGuid = patina::BinaryGuid(protocols::simple_text_input_ex::PROTOCOL_GUID);
}

impl<T: BootServices + Clone + 'static> SimpleTextInExFfi<T> {
    /// Installs the simple text in ex protocol. Returns the key required to uninstall.
    pub(crate) fn install(
        boot_services: &'static T,
        controller: efi::Handle,
        keyboard_handler: &mut KeyboardHidHandler<T>,
    ) -> Result<PtrMetadata<'static, Box<Self>>, efi::Status> {
        let mut ctx = Box::new(SimpleTextInExFfi {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: Self::simple_text_in_ex_reset,
                read_key_stroke_ex: Self::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: Self::simple_text_in_ex_set_state,
                register_key_notify: Self::simple_text_in_ex_register_key_notify,
                unregister_key_notify: Self::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: keyboard_handler as *mut KeyboardHidHandler<T>,
        });

        let ctx_ptr: *mut Self = &mut *ctx;

        let wait_for_key_event = boot_services.create_event(
            EventType::NOTIFY_WAIT,
            Tpl::NOTIFY,
            Some(Self::simple_text_in_ex_wait_for_key as EventNotifyCallback<*mut Self>),
            ctx_ptr,
        )?;

        ctx.simple_text_in_ex.wait_for_key_ex = wait_for_key_event;

        // Key notifies dispatch at TPL_CALLBACK per UEFI spec 2.10 section 12.2.5.
        let key_notify_event = match boot_services.create_event(
            EventType::NOTIFY_SIGNAL,
            Tpl::CALLBACK,
            Some(Self::process_key_notifies as EventNotifyCallback<*mut Self>),
            ctx_ptr,
        ) {
            Ok(event) => event,
            Err(status) => {
                let _ = boot_services.close_event(wait_for_key_event);
                return Err(status);
            }
        };

        match boot_services.install_protocol_interface(Some(controller), ctx) {
            Ok((_handle, key)) => {
                keyboard_handler.key_notify_event = key_notify_event;
                Ok(key)
            }
            Err(status) => {
                // install_protocol_interface reconstructs and drops the Box on failure,
                // but doesn't know about our events.
                let _ = boot_services.close_event(wait_for_key_event);
                let _ = boot_services.close_event(key_notify_event);
                Err(status)
            }
        }
    }

    /// Uninstalls the simple text in ex protocol using the key from install.
    pub(crate) fn uninstall(
        boot_services: &'static T,
        controller: efi::Handle,
        key: PtrMetadata<'static, Box<Self>>,
    ) -> Result<(), efi::Status> {
        // Save the raw pointer before consuming key, in case uninstall fails.
        let raw_ptr = key.ptr_value as *mut SimpleTextInExFfi<T>;

        let ctx = match boot_services.uninstall_protocol_interface(controller, key) {
            Ok(ctx) => ctx,
            Err(status) => {
                log::error!("Failed to uninstall simple_text_in_ex interface, status: {:x?}", status);
                // Protocol is still installed. Null keyboard_handler so callbacks don't access a
                // dropped KeyboardHidHandler.
                // SAFETY: raw_ptr was saved before consuming key; protocol is still installed so memory is valid.
                unsafe {
                    if let Some(ctx) = raw_ptr.as_mut() {
                        ctx.keyboard_handler = ptr::null_mut();
                    }
                }
                return Err(status);
            }
        };

        if let Err(status) = boot_services.close_event(ctx.simple_text_in_ex.wait_for_key_ex) {
            log::error!("Failed to close simple_text_in_ex.wait_for_key_ex event, status: {:x?}", status);
            // Leak ctx so the still-live event callback doesn't use freed memory.
            core::mem::forget(ctx);
            return Err(status);
        }

        // Close key_notify_event stored on keyboard_handler.
        // SAFETY: dereferencing keyboard_handler pointer; valid while ctx exists because uninstall succeeded.
        if let Some(keyboard_handler) = unsafe { ctx.keyboard_handler.as_mut() } {
            let key_notify_event = keyboard_handler.key_notify_event;
            keyboard_handler.key_notify_event = ptr::null_mut();
            if !key_notify_event.is_null()
                && let Err(status) = boot_services.close_event(key_notify_event)
            {
                log::error!("Failed to close key_notify_event, status: {:x?}", status);
                // Leak ctx so the still-live event callback doesn't use freed memory.
                core::mem::forget(ctx);
                return Err(status);
            }
        }
        // ctx drops here, freeing the SimpleTextInExFfi allocation.
        Ok(())
    }

    // Resets the keyboard state — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_reset(
        this: *mut protocols::simple_text_input_ex::Protocol,
        extended_verification: efi::Boolean,
    ) -> efi::Status {
        // SAFETY: casting `this` (first field of #[repr(C)] struct) to recover full context; null handled below.
        let context = unsafe { (this as *mut Self).as_mut() };
        let Some(context) = context else { return efi::Status::INVALID_PARAMETER };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_ex_reset invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };
        keyboard_handler.reset(extended_verification.into());
        log::trace!("simple_text_in_ex_reset: extended_verification={:?}", bool::from(extended_verification));
        efi::Status::SUCCESS
    }

    // Reads a key stroke — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_read_key_stroke(
        this: *mut protocols::simple_text_input_ex::Protocol,
        key_data: *mut protocols::simple_text_input_ex::KeyData,
    ) -> efi::Status {
        if this.is_null() || key_data.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
        // SAFETY: casting `this` (first field of #[repr(C)] struct) to recover full context; null handled below.
        let Some(context) = (unsafe { (this as *mut Self).as_mut() }) else {
            return efi::Status::INVALID_PARAMETER;
        };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_ex_read_key_stroke invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };

        let mut kq = keyboard_handler.state.lock();
        if let Some(key) = kq.pop_key() {
            log::trace!(
                "simple_text_in_ex_read_key_stroke: unicode=0x{:04X} scan=0x{:04X}",
                key.key.unicode_char,
                key.key.scan_code,
            );
            // SAFETY: writing through output pointer; null-checked at top of function.
            unsafe { key_data.write_unaligned(key) }
            efi::Status::SUCCESS
        } else {
            let key = protocols::simple_text_input_ex::KeyData { key_state: kq.init_key_state(), ..Default::default() };
            // SAFETY: writing through output pointer; null-checked at top of function.
            unsafe { key_data.write_unaligned(key) };
            efi::Status::NOT_READY
        }
    }

    // Sets the keyboard toggle state — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_set_state(
        this: *mut protocols::simple_text_input_ex::Protocol,
        key_toggle_state: *mut protocols::simple_text_input_ex::KeyToggleState,
    ) -> efi::Status {
        if this.is_null() || key_toggle_state.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
        // SAFETY: casting `this` (first field of #[repr(C)] struct) to recover full context; null handled below.
        let Some(context) = (unsafe { (this as *mut Self).as_mut() }) else {
            return efi::Status::INVALID_PARAMETER;
        };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_ex_set_state invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };
        // SAFETY: reading through pointer; null-checked at top of function.
        keyboard_handler.set_key_toggle_state(unsafe { key_toggle_state.read() });
        log::trace!("simple_text_in_ex_set_state: toggle state set");
        efi::Status::SUCCESS
    }

    // Registers a key notification callback — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_register_key_notify(
        this: *mut protocols::simple_text_input_ex::Protocol,
        key_data_ptr: *mut protocols::simple_text_input_ex::KeyData,
        key_notification_function: protocols::simple_text_input_ex::KeyNotifyFunction,
        notify_handle: *mut *mut c_void,
    ) -> efi::Status {
        if this.is_null()
            || key_data_ptr.is_null()
            || notify_handle.is_null()
            || key_notification_function as usize == 0
        {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: casting `this` (first field of #[repr(C)] struct) to recover full context; null handled below.
        let Some(context) = (unsafe { (this as *mut Self).as_mut() }) else {
            return efi::Status::INVALID_PARAMETER;
        };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_ex_register_key_notify invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };

        // SAFETY: reading through pointer; null-checked at top of function.
        let key_data = unsafe { key_data_ptr.read() };
        let handle = keyboard_handler.insert_key_notify_callback(key_data, key_notification_function);
        log::trace!("simple_text_in_ex_register_key_notify: handle=0x{:X}", handle);
        // SAFETY: writing through output pointer; null-checked at top of function.
        unsafe { notify_handle.write(handle as *mut c_void) };
        efi::Status::SUCCESS
    }

    // Unregisters a key notification callback — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_unregister_key_notify(
        this: *mut protocols::simple_text_input_ex::Protocol,
        notification_handle: *mut c_void,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
        // SAFETY: casting `this` (first field of #[repr(C)] struct) to recover full context; null handled below.
        let Some(context) = (unsafe { (this as *mut Self).as_mut() }) else {
            return efi::Status::INVALID_PARAMETER;
        };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_ex_unregister_key_notify invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };
        match keyboard_handler.remove_key_notify_callback(notification_handle as usize) {
            Ok(()) => {
                log::trace!("simple_text_in_ex_unregister_key_notify: handle=0x{:X}", notification_handle as usize);
                efi::Status::SUCCESS
            }
            Err(status) => status,
        }
    }

    // Handles the wait_for_key_ex event — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn simple_text_in_ex_wait_for_key(event: efi::Event, context: *mut Self) {
        // SAFETY: dereferencing context pointer from UEFI event; null handled by else branch.
        let Some(context) = (unsafe { context.as_mut() }) else {
            log::error!("simple_text_in_ex_wait_for_key invoked with invalid context");
            return;
        };
        // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else { return };
        let mut kq = keyboard_handler.state.lock();
        while let Some(key_data) = kq.peek_key() {
            if key_data.key.unicode_char == 0 && key_data.key.scan_code == 0 {
                let _ = kq.pop_key();
                continue;
            } else {
                let _ = context.boot_services.signal_event(event);
                break;
            }
        }
    }

    // Dispatches registered key notification callbacks — part of the simple_text_in_ex protocol interface.
    extern "efiapi" fn process_key_notifies(_event: efi::Event, context: *mut Self) {
        // SAFETY: dereferencing context pointer from UEFI event; null handled by else branch.
        let Some(context) = (unsafe { context.as_mut() }) else {
            return;
        };
        loop {
            // SAFETY: dereferencing keyboard_handler raw pointer; validity ensured by protocol lifecycle.
            let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
            let Some(keyboard_handler) = keyboard_handler else {
                log::error!("process_key_notifies event called without a valid keyboard_handler");
                return;
            };
            let (pending_key, pending_callbacks) = keyboard_handler.pending_callbacks();
            if let Some(mut pending_key) = pending_key {
                let key_ptr = &mut pending_key as *mut protocols::simple_text_input_ex::KeyData;
                for callback in pending_callbacks {
                    let _ = callback(key_ptr);
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use core::ptr;

    use alloc::boxed::Box;
    use r_efi::{efi, protocols};

    use patina::boot_services::{
        MockBootServices,
        c_ptr::{CPtr, PtrMetadata},
    };

    use super::*;

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| patina::boot_services::tpl::Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        // SAFETY: Leaked to obtain 'static lifetime for test use; never freed.
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    type StiExPtr = SimpleTextInExFfi<MockBootServices>;

    fn test_context(
        boot_services: &'static MockBootServices,
        handler: &mut KeyboardHidHandler<MockBootServices>,
    ) -> SimpleTextInExFfi<MockBootServices> {
        SimpleTextInExFfi {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: handler as *mut KeyboardHidHandler<MockBootServices>,
        }
    }

    fn leaked_context_key(
        boot_services: &'static MockBootServices,
        handler: &mut KeyboardHidHandler<MockBootServices>,
    ) -> PtrMetadata<'static, Box<StiExPtr>> {
        let ctx = Box::new(SimpleTextInExFfi {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: 0x42 as efi::Event,
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: handler as *mut _,
        });
        let key = ctx.metadata();
        let _ = Box::into_raw(ctx);
        key
    }

    // --- install/uninstall ---

    #[test]
    fn install_succeeds() {
        let boot_services = mock_boot_services();
        // Two create_event calls: wait_for_key and key_notify
        boot_services.expect_create_event::<*mut StiExPtr>().times(2).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services.expect_install_protocol_interface::<StiExPtr, Box<StiExPtr>>().times(1).returning(
            |_, protocol_interface| {
                let key = protocol_interface.metadata();
                let _ = Box::into_raw(protocol_interface);
                Ok((0x1 as efi::Handle, key))
            },
        );

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInExFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert!(result.is_ok());
        assert!(!handler.key_notify_event.is_null());

        let key = result.unwrap();
        // SAFETY: Reclaiming the Box leaked by the mock install_protocol_interface.
        drop(unsafe { Box::from_raw(key.ptr_value as *mut StiExPtr) });
    }

    #[test]
    fn install_returns_error_on_first_create_event_failure() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_create_event::<*mut StiExPtr>()
            .times(1)
            .returning(|_, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));
        boot_services.expect_install_protocol_interface::<StiExPtr, Box<StiExPtr>>().never();

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInExFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::OUT_OF_RESOURCES));
    }

    #[test]
    fn install_cleans_up_events_on_install_protocol_failure() {
        let boot_services = mock_boot_services();
        boot_services.expect_create_event::<*mut StiExPtr>().times(2).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services
            .expect_install_protocol_interface::<StiExPtr, Box<StiExPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));
        // Both events cleaned up on failure.
        boot_services.expect_close_event().times(2).returning(|_| Ok(()));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInExFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::ACCESS_DENIED));
    }

    #[test]
    fn uninstall_succeeds() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<StiExPtr, Box<StiExPtr>>()
            .times(1)
            // SAFETY: Reclaiming the Box from the key, mirroring the real uninstall_protocol_interface.
            .returning(|_, key| Ok(unsafe { Box::from_raw(key.ptr_value as *mut StiExPtr) }));
        // Two close_event calls: wait_for_key_ex and key_notify_event.
        boot_services.expect_close_event().times(2).returning(|_| Ok(()));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        handler.key_notify_event = 0x99 as efi::Event;
        let key = leaked_context_key(boot_services, &mut handler);
        let result = SimpleTextInExFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Ok(()));
        assert!(handler.key_notify_event.is_null());
    }

    #[test]
    fn uninstall_failure_nulls_keyboard_handler() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<StiExPtr, Box<StiExPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);
        let raw_ptr = key.ptr_value as *mut StiExPtr;

        let result = SimpleTextInExFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Err(efi::Status::ACCESS_DENIED));

        // SAFETY: raw_ptr points to the still-live leaked context (uninstall failed).
        let ctx = unsafe { raw_ptr.as_ref() }.unwrap();
        assert!(ctx.keyboard_handler.is_null());

        // SAFETY: Reclaiming the leaked context Box for cleanup.
        drop(unsafe { Box::from_raw(raw_ptr) });
    }

    // --- FFI callbacks ---

    #[test]
    fn reset_returns_invalid_parameter_on_null() {
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset(ptr::null_mut(), false.into());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn reset_returns_device_error_when_handler_null() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset(
            &mut ctx.simple_text_in_ex as *mut _,
            false.into(),
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_key_stroke_returns_not_ready_on_empty_queue() {
        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
        );
        assert_eq!(status, efi::Status::NOT_READY);
    }

    #[test]
    fn read_key_stroke_returns_invalid_parameter_on_null() {
        let status =
            SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke(ptr::null_mut(), ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn set_state_returns_invalid_parameter_on_null() {
        let status =
            SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state(ptr::null_mut(), ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn register_key_notify_returns_invalid_parameter_on_null_this() {
        // All parameters must be non-null. Testing with null `this` is sufficient
        // since the first check short-circuits.
        extern "efiapi" fn noop(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
            efi::Status::SUCCESS
        }
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let mut handle: *mut c_void = ptr::null_mut();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify(
            ptr::null_mut(),
            &mut key_data,
            noop,
            &mut handle,
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn unregister_key_notify_returns_invalid_parameter_on_null() {
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify(
            ptr::null_mut(),
            ptr::null_mut(),
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn wait_for_key_handles_null_context() {
        let event = 0x1234 as efi::Event;
        // Should log an error but not panic.
        SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_wait_for_key(event, ptr::null_mut());
    }

    #[test]
    fn reset_succeeds_with_valid_handler() {
        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset(
            &mut ctx.simple_text_in_ex as *mut _,
            false.into(),
        );
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn read_key_stroke_returns_success_when_key_available() {
        use hidparser::report_data_types::Usage;

        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        handler.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        // Push an Enter keystroke (usage 0x00070028).
        handler.state.lock().keystroke(Usage::from(0x00070028u32), super::super::key_queue::KeyAction::KeyDown);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
        );
        assert_eq!(status, efi::Status::SUCCESS);
        assert!(key_data.key.unicode_char != 0 || key_data.key.scan_code != 0);
    }

    #[test]
    fn read_key_stroke_returns_device_error_when_handler_null() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi::<MockBootServices> {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn set_state_succeeds_with_valid_handler() {
        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut toggle_state: protocols::simple_text_input_ex::KeyToggleState =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID;
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut toggle_state,
        );
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn set_state_returns_device_error_when_handler_null() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi::<MockBootServices> {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let mut toggle_state: protocols::simple_text_input_ex::KeyToggleState =
            protocols::simple_text_input_ex::TOGGLE_STATE_VALID;
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut toggle_state,
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn register_key_notify_succeeds() {
        extern "efiapi" fn noop(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
            efi::Status::SUCCESS
        }

        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let mut handle: *mut c_void = ptr::null_mut();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
            noop,
            &mut handle,
        );
        assert_eq!(status, efi::Status::SUCCESS);
        assert!(!handle.is_null());
    }

    #[test]
    fn register_key_notify_returns_device_error_when_handler_null() {
        extern "efiapi" fn noop(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
            efi::Status::SUCCESS
        }

        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi::<MockBootServices> {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let mut handle: *mut c_void = ptr::null_mut();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
            noop,
            &mut handle,
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn unregister_key_notify_returns_device_error_when_handler_null() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi::<MockBootServices> {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify(
            &mut ctx.simple_text_in_ex as *mut _,
            core::ptr::dangling_mut::<c_void>(),
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn unregister_key_notify_succeeds_after_register() {
        extern "efiapi" fn noop(_key: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
            efi::Status::SUCCESS
        }

        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut key_data = protocols::simple_text_input_ex::KeyData::default();
        let mut handle: *mut c_void = ptr::null_mut();
        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify(
            &mut ctx.simple_text_in_ex as *mut _,
            &mut key_data,
            noop,
            &mut handle,
        );
        assert_eq!(status, efi::Status::SUCCESS);

        let status = SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify(
            &mut ctx.simple_text_in_ex as *mut _,
            handle,
        );
        assert_eq!(status, efi::Status::SUCCESS);
    }

    #[test]
    fn wait_for_key_signals_event_when_key_available() {
        use hidparser::report_data_types::Usage;

        let boot_services = mock_boot_services();
        boot_services.expect_signal_event().times(1).returning(|_| Ok(()));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        handler.set_layout(Some(crate::keyboard::layout::get_default_keyboard_layout()));
        handler.state.lock().keystroke(Usage::from(0x00070028u32), super::super::key_queue::KeyAction::KeyDown);
        let mut ctx = test_context(boot_services, &mut handler);
        let event = 0x1234 as efi::Event;
        SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_wait_for_key(event, &mut ctx);
    }

    #[test]
    fn process_key_notifies_handles_null_handler() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInExFfi::<MockBootServices> {
            simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
                reset: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_reset,
                read_key_stroke_ex: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_read_key_stroke,
                wait_for_key_ex: ptr::null_mut(),
                set_state: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_set_state,
                register_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_register_key_notify,
                unregister_key_notify: SimpleTextInExFfi::<MockBootServices>::simple_text_in_ex_unregister_key_notify,
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let event = 0x1234 as efi::Event;
        // Should return without panic when keyboard_handler is null.
        SimpleTextInExFfi::<MockBootServices>::process_key_notifies(event, &mut ctx);
    }
}
