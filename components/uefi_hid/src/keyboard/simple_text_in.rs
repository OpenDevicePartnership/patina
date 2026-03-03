//! Simple Text In Protocol FFI Support.
//!
//! This module manages the Simple Text In FFI.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!

use alloc::boxed::Box;
use core::ptr;

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

/// FFI context for SimpleTextInput protocol.
///
/// A pointer to KeyboardHidHandler is included in the context so that it can be reclaimed in the simple_text_in API
/// implementations. Mutual exclusion on the KeyboardHidHandler is provided by the TplMutex on its key_queue state.
///
/// The simple_text_in protocol element must be the first element in the structure so that the full structure can be
/// recovered by simple casting.
#[repr(C)]
pub(crate) struct SimpleTextInFfi<T: BootServices + Clone + 'static> {
    simple_text_in: protocols::simple_text_input::Protocol,
    pub(crate) boot_services: &'static T,
    pub(crate) keyboard_handler: *mut KeyboardHidHandler<T>,
}

// SAFETY: SimpleTextInFfi<T> is #[repr(C)] with protocols::simple_text_input::Protocol as its
// first field, so a pointer to SimpleTextInFfi<T> is a valid pointer to Protocol per the
// first-field casting pattern.
unsafe impl<T: BootServices + Clone + 'static> ProtocolInterface for SimpleTextInFfi<T> {
    const PROTOCOL_GUID: efi::Guid = protocols::simple_text_input::PROTOCOL_GUID;
}

impl<T: BootServices + Clone + 'static> SimpleTextInFfi<T> {
    /// Installs the simple text in protocol. Returns the key required to uninstall.
    pub(crate) fn install(
        boot_services: &'static T,
        controller: efi::Handle,
        keyboard_handler: &mut KeyboardHidHandler<T>,
    ) -> Result<PtrMetadata<'static, Box<Self>>, efi::Status> {
        let mut ctx = Box::new(SimpleTextInFfi {
            simple_text_in: protocols::simple_text_input::Protocol {
                reset: Self::simple_text_in_reset,
                read_key_stroke: Self::simple_text_in_read_key_stroke,
                wait_for_key: ptr::null_mut(),
            },
            boot_services,
            keyboard_handler: keyboard_handler as *mut KeyboardHidHandler<T>,
        });

        let ctx_ptr: *mut Self = &mut *ctx;

        let wait_for_key_event = boot_services.create_event(
            EventType::NOTIFY_WAIT,
            Tpl::NOTIFY,
            Some(Self::simple_text_in_wait_for_key as EventNotifyCallback<*mut Self>),
            ctx_ptr,
        )?;

        ctx.simple_text_in.wait_for_key = wait_for_key_event;

        match boot_services.install_protocol_interface(Some(controller), ctx) {
            Ok((_handle, key)) => Ok(key),
            Err(status) => {
                // install_protocol_interface reconstructs and drops the Box on failure,
                // but doesn't know about our event.
                let _ = boot_services.close_event(wait_for_key_event);
                Err(status)
            }
        }
    }

    /// Uninstalls the simple text in protocol using the key from install.
    pub(crate) fn uninstall(
        boot_services: &'static T,
        controller: efi::Handle,
        key: PtrMetadata<'static, Box<Self>>,
    ) -> Result<(), efi::Status> {
        // Save the raw pointer before consuming key, in case uninstall fails.
        let raw_ptr = key.ptr_value as *mut SimpleTextInFfi<T>;

        let ctx = match boot_services.uninstall_protocol_interface(controller, key) {
            Ok(ctx) => ctx,
            Err(status) => {
                log::error!("Failed to uninstall simple_text_in interface, status: {:x?}", status);
                // Protocol is still installed. Null keyboard_handler so callbacks don't access a
                // dropped KeyboardHidHandler.
                unsafe {
                    if let Some(ctx) = raw_ptr.as_mut() {
                        ctx.keyboard_handler = ptr::null_mut();
                    }
                }
                return Err(status);
            }
        };

        if let Err(status) = boot_services.close_event(ctx.simple_text_in.wait_for_key) {
            log::error!("Failed to close simple_text_in.wait_for_key event, status: {:x?}", status);
            return Err(status);
        }

        // ctx drops here, freeing the SimpleTextInFfi allocation.
        Ok(())
    }

    // Resets the keyboard state — part of the simple_text_in protocol interface.
    extern "efiapi" fn simple_text_in_reset(
        this: *mut protocols::simple_text_input::Protocol,
        extended_verification: efi::Boolean,
    ) -> efi::Status {
        let context = unsafe { (this as *mut Self).as_mut() };
        let Some(context) = context else { return efi::Status::INVALID_PARAMETER };
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_reset invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };
        keyboard_handler.reset(extended_verification.into());
        log::trace!("simple_text_in_reset: extended_verification={:?}", bool::from(extended_verification));
        efi::Status::SUCCESS
    }

    // Reads a key stroke — part of the simple_text_in protocol interface.
    extern "efiapi" fn simple_text_in_read_key_stroke(
        this: *mut protocols::simple_text_input::Protocol,
        key: *mut protocols::simple_text_input::InputKey,
    ) -> efi::Status {
        if this.is_null() || key.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
        let Some(context) = (unsafe { (this as *mut Self).as_mut() }) else {
            return efi::Status::INVALID_PARAMETER;
        };
        let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
        let Some(keyboard_handler) = keyboard_handler else {
            log::error!("simple_text_in_read_key_stroke invoked after keyboard dropped.");
            return efi::Status::DEVICE_ERROR;
        };

        let mut kq = keyboard_handler.state.lock();
        loop {
            if let Some(mut key_data) = kq.pop_key() {
                // skip partials
                if key_data.key.unicode_char == 0 && key_data.key.scan_code == 0 {
                    continue;
                }
                const CONTROL_PRESSED: u32 = protocols::simple_text_input_ex::RIGHT_CONTROL_PRESSED
                    | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;
                const LOWERCASE_A: u16 = 0x0061;
                const LOWERCASE_Z: u16 = 0x007a;
                const UPPERCASE_A: u16 = 0x0041;
                const UPPERCASE_Z: u16 = 0x005a;
                if (key_data.key_state.key_shift_state & CONTROL_PRESSED) != 0 {
                    if key_data.key.unicode_char >= LOWERCASE_A && key_data.key.unicode_char <= LOWERCASE_Z {
                        key_data.key.unicode_char = (key_data.key.unicode_char - LOWERCASE_A) + 1;
                    }
                    if key_data.key.unicode_char >= UPPERCASE_A && key_data.key.unicode_char <= UPPERCASE_Z {
                        key_data.key.unicode_char = (key_data.key.unicode_char - UPPERCASE_A) + 1;
                    }
                }
                unsafe { key.write_unaligned(key_data.key) }
                log::trace!(
                    "simple_text_in_read_key_stroke: unicode=0x{:04X} scan=0x{:04X}",
                    key_data.key.unicode_char,
                    key_data.key.scan_code,
                );
                return efi::Status::SUCCESS;
            } else {
                return efi::Status::NOT_READY;
            }
        }
    }

    // Handles the wait_for_key event — part of the simple_text_in protocol interface.
    extern "efiapi" fn simple_text_in_wait_for_key(event: efi::Event, context: *mut Self) {
        let Some(context) = (unsafe { context.as_mut() }) else {
            log::error!("simple_text_in_wait_for_key invoked with invalid context");
            return;
        };
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
}

#[cfg(test)]
mod test {
    use core::ptr;

    use alloc::boxed::Box;
    use r_efi::{efi, protocols};

    use patina::boot_services::MockBootServices;
    use patina::boot_services::c_ptr::{CPtr, PtrMetadata};

    use super::*;

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| patina::boot_services::tpl::Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    type StiPtr = SimpleTextInFfi<MockBootServices>;

    fn test_context(
        boot_services: &'static MockBootServices,
        handler: &mut KeyboardHidHandler<MockBootServices>,
    ) -> SimpleTextInFfi<MockBootServices> {
        SimpleTextInFfi {
            simple_text_in: protocols::simple_text_input::Protocol {
                reset: SimpleTextInFfi::<MockBootServices>::simple_text_in_reset,
                read_key_stroke: SimpleTextInFfi::<MockBootServices>::simple_text_in_read_key_stroke,
                wait_for_key: ptr::null_mut(),
            },
            boot_services,
            keyboard_handler: handler as *mut KeyboardHidHandler<MockBootServices>,
        }
    }

    fn leaked_context_key(
        boot_services: &'static MockBootServices,
        handler: &mut KeyboardHidHandler<MockBootServices>,
    ) -> PtrMetadata<'static, Box<StiPtr>> {
        let ctx = Box::new(SimpleTextInFfi {
            simple_text_in: protocols::simple_text_input::Protocol {
                reset: SimpleTextInFfi::<MockBootServices>::simple_text_in_reset,
                read_key_stroke: SimpleTextInFfi::<MockBootServices>::simple_text_in_read_key_stroke,
                wait_for_key: 0x42 as efi::Event,
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
        boot_services.expect_create_event::<*mut StiPtr>().times(1).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services.expect_install_protocol_interface::<StiPtr, Box<StiPtr>>().times(1).returning(
            |_, protocol_interface| {
                let key = protocol_interface.metadata();
                let _ = Box::into_raw(protocol_interface);
                Ok((0x1 as efi::Handle, key))
            },
        );

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert!(result.is_ok());

        let key = result.unwrap();
        drop(unsafe { Box::from_raw(key.ptr_value as *mut StiPtr) });
    }

    #[test]
    fn install_returns_error_on_create_event_failure() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_create_event::<*mut StiPtr>()
            .times(1)
            .returning(|_, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));
        boot_services.expect_install_protocol_interface::<StiPtr, Box<StiPtr>>().never();

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::OUT_OF_RESOURCES));
    }

    #[test]
    fn install_cleans_up_event_on_install_protocol_failure() {
        let boot_services = mock_boot_services();
        boot_services.expect_create_event::<*mut StiPtr>().times(1).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services
            .expect_install_protocol_interface::<StiPtr, Box<StiPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));
        boot_services.expect_close_event().times(1).returning(|_| Ok(()));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let result = SimpleTextInFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::ACCESS_DENIED));
    }

    #[test]
    fn uninstall_succeeds() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<StiPtr, Box<StiPtr>>()
            .times(1)
            .returning(|_, key| Ok(unsafe { Box::from_raw(key.ptr_value as *mut StiPtr) }));
        boot_services.expect_close_event().times(1).returning(|_| Ok(()));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);
        let result = SimpleTextInFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn uninstall_failure_nulls_keyboard_handler() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<StiPtr, Box<StiPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);
        let raw_ptr = key.ptr_value as *mut StiPtr;

        let result = SimpleTextInFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Err(efi::Status::ACCESS_DENIED));

        let ctx = unsafe { raw_ptr.as_ref() }.unwrap();
        assert!(ctx.keyboard_handler.is_null());

        drop(unsafe { Box::from_raw(raw_ptr) });
    }

    #[test]
    fn uninstall_returns_error_on_close_event_failure() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<StiPtr, Box<StiPtr>>()
            .times(1)
            .returning(|_, key| Ok(unsafe { Box::from_raw(key.ptr_value as *mut StiPtr) }));
        boot_services.expect_close_event().times(1).returning(|_| Err(efi::Status::INVALID_PARAMETER));

        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);
        let result = SimpleTextInFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));
    }

    // --- FFI callbacks ---

    #[test]
    fn reset_returns_invalid_parameter_on_null() {
        let status = SimpleTextInFfi::<MockBootServices>::simple_text_in_reset(ptr::null_mut(), false.into());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn reset_returns_device_error_when_handler_null() {
        let boot_services = mock_boot_services();
        let mut ctx = SimpleTextInFfi {
            simple_text_in: protocols::simple_text_input::Protocol {
                reset: SimpleTextInFfi::<MockBootServices>::simple_text_in_reset,
                read_key_stroke: SimpleTextInFfi::<MockBootServices>::simple_text_in_read_key_stroke,
                wait_for_key: ptr::null_mut(),
            },
            boot_services,
            keyboard_handler: ptr::null_mut(),
        };
        let status =
            SimpleTextInFfi::<MockBootServices>::simple_text_in_reset(&mut ctx.simple_text_in as *mut _, false.into());
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn read_key_stroke_returns_not_ready_on_empty_queue() {
        let boot_services = mock_boot_services();
        let mut handler = KeyboardHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut key = protocols::simple_text_input::InputKey::default();
        let status = SimpleTextInFfi::<MockBootServices>::simple_text_in_read_key_stroke(
            &mut ctx.simple_text_in as *mut _,
            &mut key,
        );
        assert_eq!(status, efi::Status::NOT_READY);
    }

    #[test]
    fn read_key_stroke_returns_invalid_parameter_on_null() {
        let status =
            SimpleTextInFfi::<MockBootServices>::simple_text_in_read_key_stroke(ptr::null_mut(), ptr::null_mut());
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn wait_for_key_handles_null_context() {
        let event = 0x1234 as efi::Event;
        // Should log an error but not panic.
        SimpleTextInFfi::<MockBootServices>::simple_text_in_wait_for_key(event, ptr::null_mut());
    }
}
