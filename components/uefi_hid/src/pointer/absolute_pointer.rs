//! Absolute Pointer Protocol FFI Support.
//!
//! This module manages the Absolute Pointer Protocol FFI.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
use alloc::boxed::Box;
use core::ptr;

use r_efi::{efi, protocols};

use hidparser::report_data_types::Usage;

use patina::{
    boot_services::{
        BootServices,
        c_ptr::PtrMetadata,
        event::{EventNotifyCallback, EventType},
        tpl::Tpl,
    },
    uefi_protocol::ProtocolInterface,
};

use super::{
    AXIS_RESOLUTION, BUTTON_MAX, BUTTON_MIN, DIGITIZER_SWITCH_MAX, DIGITIZER_SWITCH_MIN, GENERIC_DESKTOP_WHEEL,
    GENERIC_DESKTOP_X, GENERIC_DESKTOP_Y, GENERIC_DESKTOP_Z, PointerHidHandler,
};

/// FFI context
/// # Safety
/// A pointer to PointerHidHandler is included in the context so that it can be reclaimed in the absolute_pointer
/// API implementation. Care must be taken to ensure that rust invariants are respected when accessing the
/// PointerHidHandler. In particular, the design must ensure mutual exclusion on the PointerHidHandler between
/// callbacks running at different TPL; this is accomplished by ensuring all access to the structure is at TPL_NOTIFY
/// once initialization is complete.
///
/// The absolute_pointer element must be the first element in the structure so that the full structure can be
/// recovered by simple casting.
#[repr(C)]
pub(crate) struct AbsolutePointerFfi<T: BootServices + Clone + 'static> {
    absolute_pointer: protocols::absolute_pointer::Protocol,
    boot_services: &'static T,
    pointer_handler: *mut PointerHidHandler<T>,
}

// SAFETY: AbsolutePointerFfi<T> is #[repr(C)] with protocols::absolute_pointer::Protocol as its
// first field, so a pointer to AbsolutePointerFfi<T> is a valid pointer to Protocol per the
// first-field casting pattern.
unsafe impl<T: BootServices + Clone + 'static> ProtocolInterface for AbsolutePointerFfi<T> {
    const PROTOCOL_GUID: patina::BinaryGuid = patina::BinaryGuid(protocols::absolute_pointer::PROTOCOL_GUID);
}

impl<T: BootServices + Clone + 'static> Drop for AbsolutePointerFfi<T> {
    fn drop(&mut self) {
        if !self.absolute_pointer.mode.is_null() {
            // SAFETY: mode was created via Box::into_raw during install and is non-null (checked above).
            drop(unsafe { Box::from_raw(self.absolute_pointer.mode) });
        }
    }
}

impl<T: BootServices + Clone + 'static> AbsolutePointerFfi<T> {
    /// Installs the absolute pointer protocol. If successful, returns the key required to uninstall.
    pub(crate) fn install(
        boot_services: &'static T,
        controller: efi::Handle,
        pointer_handler: &mut PointerHidHandler<T>,
    ) -> Result<PtrMetadata<'static, Box<Self>>, efi::Status> {
        let mut pointer_ctx = Box::new(AbsolutePointerFfi {
            absolute_pointer: protocols::absolute_pointer::Protocol {
                reset: Self::absolute_pointer_reset,
                get_state: Self::absolute_pointer_get_state,
                mode: Box::into_raw(Box::new(Self::initialize_mode(pointer_handler))),
                wait_for_input: ptr::null_mut(),
            },
            boot_services,
            pointer_handler: pointer_handler as *mut PointerHidHandler<T>,
        });

        let ctx_ptr: *mut Self = &mut *pointer_ctx;

        let wait_for_input_event = boot_services.create_event(
            EventType::NOTIFY_WAIT,
            Tpl::NOTIFY,
            Some(Self::absolute_pointer_wait_for_input as EventNotifyCallback<*mut Self>),
            ctx_ptr,
        )?;

        pointer_ctx.absolute_pointer.wait_for_input = wait_for_input_event;

        match boot_services.install_protocol_interface(Some(controller), pointer_ctx) {
            Ok((_handle, key)) => Ok(key),
            Err(status) => {
                // install_protocol_interface reconstructs and drops the Box on failure (freeing mode
                // via AbsolutePointerFfi::Drop), but doesn't know about our event.
                let _ = boot_services.close_event(wait_for_input_event);
                Err(status)
            }
        }
    }

    // Initializes the absolute_pointer mode structure.
    fn initialize_mode(pointer_handler: &PointerHidHandler<T>) -> protocols::absolute_pointer::Mode {
        let mut mode: protocols::absolute_pointer::Mode = Default::default();

        if pointer_handler.processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_X)) {
            mode.absolute_max_x = AXIS_RESOLUTION;
            mode.absolute_min_x = 0;
        } else {
            log::warn!("No x-axis usages found in the report descriptor.");
        }

        if pointer_handler.processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_Y)) {
            mode.absolute_max_y = AXIS_RESOLUTION;
            mode.absolute_min_y = 0;
        } else {
            log::warn!("No y-axis usages found in the report descriptor.");
        }

        if pointer_handler.processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_Z))
            || pointer_handler.processor.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_WHEEL))
        {
            mode.absolute_max_z = AXIS_RESOLUTION;
            mode.absolute_min_z = 0;
        }

        let has_multiple_buttons = pointer_handler
            .processor
            .supported_usages
            .iter()
            .filter(|x| matches!((**x).into(), BUTTON_MIN..=BUTTON_MAX | DIGITIZER_SWITCH_MIN..=DIGITIZER_SWITCH_MAX))
            .nth(1)
            .is_some();

        if has_multiple_buttons {
            mode.attributes |= protocols::absolute_pointer::SUPPORTS_ALT_ACTIVE;
        }

        mode
    }

    /// Uninstalls the absolute pointer protocol
    pub(crate) fn uninstall(
        boot_services: &'static T,
        controller: efi::Handle,
        key: PtrMetadata<'static, Box<Self>>,
    ) -> Result<(), efi::Status> {
        // Save the raw pointer before consuming key, in case uninstall fails.
        let raw_ptr = key.ptr_value as *mut AbsolutePointerFfi<T>;

        let pointer_ctx = match boot_services.uninstall_protocol_interface(controller, key) {
            Ok(ctx) => ctx,
            Err(status) => {
                log::error!("Failed to uninstall absolute_pointer interface, status: {:x?}", status);
                // Protocol is still installed. Null pointer_handler so callbacks don't access a
                // dropped PointerHidHandler.
                // SAFETY: raw_ptr was saved before uninstall consumed the key; protocol is still installed so memory is valid.
                unsafe {
                    if let Some(ctx) = raw_ptr.as_mut() {
                        ctx.pointer_handler = ptr::null_mut();
                    }
                }
                return Err(status);
            }
        };

        if let Err(status) = boot_services.close_event(pointer_ctx.absolute_pointer.wait_for_input) {
            log::error!("Failed to close absolute_pointer.wait_for_input event, status: {:x?}", status);
            // Leak pointer_ctx so the still-live event callback doesn't use freed memory.
            core::mem::forget(pointer_ctx);
            return Err(status);
        }

        // pointer_ctx drops here, freeing the mode allocation via AbsolutePointerFfi::Drop.
        Ok(())
    }

    // Handles the wait_for_input event — part of the absolute_pointer protocol interface.
    extern "efiapi" fn absolute_pointer_wait_for_input(event: efi::Event, context: *mut Self) {
        if context.is_null() {
            log::error!("absolute_pointer_wait_for_input invoked with invalid context");
            return;
        }
        // SAFETY: context pointer is non-null (checked above) and valid for the lifetime of the UEFI event callback.
        let context = unsafe { context.as_ref() }.expect("context pointer should not be null");
        // SAFETY: pointer_handler validity is ensured by the protocol lifecycle.
        if let Some(pointer_handler) = unsafe { context.pointer_handler.as_ref() } {
            if pointer_handler.state.lock().state_changed {
                let _ = context.boot_services.signal_event(event);
            }
        } else {
            log::error!("absolute_pointer_wait_for_input invoked after pointer dropped.");
        }
    }

    // Resets the pointer state — part of the absolute_pointer protocol interface.
    extern "efiapi" fn absolute_pointer_reset(
        this: *mut protocols::absolute_pointer::Protocol,
        _extended_verification: bool,
    ) -> efi::Status {
        if this.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }
        // SAFETY: this is the first field of #[repr(C)] AbsolutePointerFfi, non-null (checked above).
        let context =
            unsafe { (this as *const AbsolutePointerFfi<T>).as_ref() }.expect("context pointer should not be null");
        // SAFETY: pointer_handler validity is ensured by the protocol lifecycle.
        if let Some(pointer_handler) = unsafe { context.pointer_handler.as_ref() } {
            pointer_handler.state.lock().reset();
            efi::Status::SUCCESS
        } else {
            log::error!("absolute_pointer_reset invoked after pointer dropped.");
            efi::Status::DEVICE_ERROR
        }
    }

    // Returns the current pointer state — part of the absolute_pointer protocol interface.
    extern "efiapi" fn absolute_pointer_get_state(
        this: *mut protocols::absolute_pointer::Protocol,
        state: *mut protocols::absolute_pointer::State,
    ) -> efi::Status {
        if this.is_null() || state.is_null() {
            return efi::Status::INVALID_PARAMETER;
        }

        // SAFETY: this is the first field of #[repr(C)] AbsolutePointerFfi, non-null (checked above).
        let context =
            unsafe { (this as *const AbsolutePointerFfi<T>).as_ref() }.expect("context pointer should not be null");
        // SAFETY: pointer_handler validity is ensured by the protocol lifecycle.
        if let Some(pointer_handler) = unsafe { context.pointer_handler.as_ref() } {
            let mut pointer_state = pointer_handler.state.lock();
            if pointer_state.state_changed {
                // SAFETY: state is non-null (checked above), using write_unaligned to avoid any alignment issues.
                unsafe {
                    state.write_unaligned(pointer_state.current_state);
                }
                pointer_state.state_changed = false;
                efi::Status::SUCCESS
            } else {
                efi::Status::NOT_READY
            }
        } else {
            log::error!("absolute_pointer_get_state invoked after pointer dropped.");
            efi::Status::DEVICE_ERROR
        }
    }
}

#[cfg(test)]
mod test {
    use core::ptr;

    use alloc::boxed::Box;
    use r_efi::{efi, protocols};

    use hidparser::report_data_types::Usage;
    use patina::boot_services::MockBootServices;

    use super::*;
    use crate::pointer::{
        AXIS_RESOLUTION, BUTTON_MIN, CENTER, GENERIC_DESKTOP_WHEEL, GENERIC_DESKTOP_X, GENERIC_DESKTOP_Y,
        GENERIC_DESKTOP_Z, PointerHidHandler,
    };

    fn mock_boot_services() -> &'static mut MockBootServices {
        let mut mock = MockBootServices::new();
        mock.expect_raise_tpl().returning(|_| patina::boot_services::tpl::Tpl::APPLICATION);
        mock.expect_restore_tpl().returning(|_| ());
        // SAFETY: Leaked mock for test use with 'static lifetime requirement.
        unsafe { Box::into_raw(Box::new(mock)).as_mut().unwrap() }
    }

    /// Builds a test AbsolutePointerFfi context wired to the given handler. The caller must ensure
    /// `handler` outlives the returned context. The returned context has a null `wait_for_input`
    /// event (not needed for callback unit tests) and a heap-allocated mode.
    fn test_context(
        boot_services: &'static MockBootServices,
        handler: &mut PointerHidHandler<MockBootServices>,
    ) -> AbsolutePointerFfi<MockBootServices> {
        AbsolutePointerFfi {
            absolute_pointer: protocols::absolute_pointer::Protocol {
                reset: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset,
                get_state: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state,
                mode: Box::into_raw(Box::new(AbsolutePointerFfi::initialize_mode(handler))),
                wait_for_input: ptr::null_mut(),
            },
            boot_services,
            pointer_handler: handler as *mut PointerHidHandler<MockBootServices>,
        }
    }

    // --- initialize_mode tests ---

    #[test]
    fn initialize_mode_sets_xy_axes() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.processor.supported_usages.insert(Usage::from(GENERIC_DESKTOP_X));
        handler.processor.supported_usages.insert(Usage::from(GENERIC_DESKTOP_Y));

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_eq!(mode.absolute_max_x, AXIS_RESOLUTION);
        assert_eq!(mode.absolute_min_x, 0);
        assert_eq!(mode.absolute_max_y, AXIS_RESOLUTION);
        assert_eq!(mode.absolute_min_y, 0);
        assert_eq!(mode.absolute_max_z, 0);
        assert_eq!(mode.attributes, 0);
    }

    #[test]
    fn initialize_mode_sets_z_axis_for_z_usage() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.processor.supported_usages.insert(Usage::from(GENERIC_DESKTOP_Z));

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_eq!(mode.absolute_max_z, AXIS_RESOLUTION);
        assert_eq!(mode.absolute_min_z, 0);
    }

    #[test]
    fn initialize_mode_sets_z_axis_for_wheel_usage() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.processor.supported_usages.insert(Usage::from(GENERIC_DESKTOP_WHEEL));

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_eq!(mode.absolute_max_z, AXIS_RESOLUTION);
        assert_eq!(mode.absolute_min_z, 0);
    }

    #[test]
    fn initialize_mode_sets_alt_active_for_multiple_buttons() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.processor.supported_usages.insert(Usage::from(BUTTON_MIN));
        handler.processor.supported_usages.insert(Usage::from(BUTTON_MIN + 1));

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_ne!(mode.attributes & protocols::absolute_pointer::SUPPORTS_ALT_ACTIVE, 0);
    }

    #[test]
    fn initialize_mode_no_alt_active_for_single_button() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.processor.supported_usages.insert(Usage::from(BUTTON_MIN));

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_eq!(mode.attributes & protocols::absolute_pointer::SUPPORTS_ALT_ACTIVE, 0);
    }

    #[test]
    fn initialize_mode_no_usages_returns_zeroed_mode() {
        let boot_services = mock_boot_services();
        let handler = PointerHidHandler::new_for_test(boot_services);

        let mode = AbsolutePointerFfi::initialize_mode(&handler);

        assert_eq!(mode.absolute_max_x, 0);
        assert_eq!(mode.absolute_max_y, 0);
        assert_eq!(mode.absolute_max_z, 0);
        assert_eq!(mode.attributes, 0);
    }

    // --- FFI callback tests ---

    #[test]
    fn reset_returns_invalid_parameter_on_null() {
        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset(ptr::null_mut(), false);
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn reset_clears_state() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        // Set up some non-default state.
        {
            let mut state = handler.state.lock();
            state.current_state.current_x = 100;
            state.state_changed = true;
        }

        let mut ctx = test_context(boot_services, &mut handler);
        let status =
            AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset(&mut ctx.absolute_pointer as *mut _, false);

        assert_eq!(status, efi::Status::SUCCESS);
        let state = handler.state.lock();
        assert_eq!(state.current_state.current_x, CENTER);
        assert_eq!(state.current_state.current_y, CENTER);
        assert!(!state.state_changed);
    }

    #[test]
    fn reset_returns_device_error_after_pointer_dropped() {
        let boot_services = mock_boot_services();
        let mut ctx = AbsolutePointerFfi {
            absolute_pointer: protocols::absolute_pointer::Protocol {
                reset: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset,
                get_state: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state,
                mode: ptr::null_mut(),
                wait_for_input: ptr::null_mut(),
            },
            boot_services,
            pointer_handler: ptr::null_mut(),
        };

        let status =
            AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset(&mut ctx.absolute_pointer as *mut _, false);
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn get_state_returns_invalid_parameter_on_null_this() {
        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state(
            ptr::null_mut(),
            &mut protocols::absolute_pointer::State::default(),
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn get_state_returns_invalid_parameter_on_null_state() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);

        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state(
            &mut ctx.absolute_pointer as *mut _,
            ptr::null_mut(),
        );
        assert_eq!(status, efi::Status::INVALID_PARAMETER);
    }

    #[test]
    fn get_state_returns_not_ready_when_unchanged() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let mut ctx = test_context(boot_services, &mut handler);
        let mut state = protocols::absolute_pointer::State::default();

        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state(
            &mut ctx.absolute_pointer as *mut _,
            &mut state,
        );
        assert_eq!(status, efi::Status::NOT_READY);
    }

    #[test]
    fn get_state_returns_state_and_clears_flag() {
        let boot_services = mock_boot_services();
        let mut handler = PointerHidHandler::new_for_test(boot_services);
        {
            let mut state = handler.state.lock();
            state.current_state.current_x = 200;
            state.current_state.current_y = 300;
            state.current_state.active_buttons = 1;
            state.state_changed = true;
        }

        let mut ctx = test_context(boot_services, &mut handler);
        let mut out_state = protocols::absolute_pointer::State::default();

        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state(
            &mut ctx.absolute_pointer as *mut _,
            &mut out_state,
        );

        assert_eq!(status, efi::Status::SUCCESS);
        assert_eq!(out_state.current_x, 200);
        assert_eq!(out_state.current_y, 300);
        assert_eq!(out_state.active_buttons, 1);
        // Flag should be cleared after read.
        assert!(!handler.state.lock().state_changed);
    }

    #[test]
    fn get_state_returns_device_error_after_pointer_dropped() {
        let boot_services = mock_boot_services();
        let mut ctx = AbsolutePointerFfi {
            absolute_pointer: protocols::absolute_pointer::Protocol {
                reset: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset,
                get_state: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state,
                mode: ptr::null_mut(),
                wait_for_input: ptr::null_mut(),
            },
            boot_services,
            pointer_handler: ptr::null_mut(),
        };

        let status = AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state(
            &mut ctx.absolute_pointer as *mut _,
            &mut protocols::absolute_pointer::State::default(),
        );
        assert_eq!(status, efi::Status::DEVICE_ERROR);
    }

    #[test]
    fn wait_for_input_signals_event_when_state_changed() {
        let boot_services = mock_boot_services();
        boot_services.expect_signal_event().times(1).returning(|_| Ok(()));

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        handler.state.lock().state_changed = true;

        let mut ctx = test_context(boot_services, &mut handler);
        let event = 0x1234 as efi::Event;

        AbsolutePointerFfi::<MockBootServices>::absolute_pointer_wait_for_input(event, &mut ctx);
    }

    #[test]
    fn wait_for_input_does_not_signal_when_unchanged() {
        let boot_services = mock_boot_services();
        boot_services.expect_signal_event().never();

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        // state_changed defaults to false.

        let mut ctx = test_context(boot_services, &mut handler);
        let event = 0x1234 as efi::Event;

        AbsolutePointerFfi::<MockBootServices>::absolute_pointer_wait_for_input(event, &mut ctx);
    }

    #[test]
    fn wait_for_input_handles_null_context() {
        let event = 0x1234 as efi::Event;
        // Should log an error but not panic.
        AbsolutePointerFfi::<MockBootServices>::absolute_pointer_wait_for_input(event, ptr::null_mut());
    }

    // --- install/uninstall tests ---

    /// Creates a leaked `AbsolutePointerFfi` box and returns its `PtrMetadata` key,
    /// simulating the state after a successful `install`. The caller is responsible for
    /// ensuring the leaked allocation is cleaned up (either via `uninstall_protocol_interface`
    /// reconstructing the Box, or manual `Box::from_raw`).
    fn leaked_context_key(
        boot_services: &'static MockBootServices,
        handler: &mut PointerHidHandler<MockBootServices>,
    ) -> PtrMetadata<'static, Box<AbsolutePointerFfi<MockBootServices>>> {
        use patina::boot_services::c_ptr::CPtr;

        let ctx = Box::new(AbsolutePointerFfi {
            absolute_pointer: protocols::absolute_pointer::Protocol {
                reset: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_reset,
                get_state: AbsolutePointerFfi::<MockBootServices>::absolute_pointer_get_state,
                mode: Box::into_raw(Box::new(protocols::absolute_pointer::Mode::default())),
                wait_for_input: 0x42 as efi::Event,
            },
            boot_services,
            pointer_handler: handler as *mut _,
        });
        let key = ctx.metadata();
        let _ = Box::into_raw(ctx);
        key
    }

    type AbsPtr = AbsolutePointerFfi<MockBootServices>;

    #[test]
    fn install_succeeds() {
        let boot_services = mock_boot_services();
        boot_services.expect_create_event::<*mut AbsPtr>().times(1).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services.expect_install_protocol_interface::<AbsPtr, Box<AbsPtr>>().times(1).returning(
            |_, protocol_interface| {
                use patina::boot_services::c_ptr::CPtr;
                let key = protocol_interface.metadata();
                let _ = Box::into_raw(protocol_interface);
                Ok((0x1 as efi::Handle, key))
            },
        );

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let result = AbsolutePointerFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert!(result.is_ok());

        // Clean up the leaked box.
        let key = result.unwrap();
        // SAFETY: Reclaiming the Box leaked by the mock install_protocol_interface.
        drop(unsafe { Box::from_raw(key.ptr_value as *mut AbsPtr) });
    }

    #[test]
    fn install_returns_error_on_create_event_failure() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_create_event::<*mut AbsPtr>()
            .times(1)
            .returning(|_, _, _, _| Err(efi::Status::OUT_OF_RESOURCES));
        boot_services.expect_install_protocol_interface::<AbsPtr, Box<AbsPtr>>().never();

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let result = AbsolutePointerFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::OUT_OF_RESOURCES));
    }

    #[test]
    fn install_cleans_up_event_on_install_protocol_failure() {
        let boot_services = mock_boot_services();
        boot_services.expect_create_event::<*mut AbsPtr>().times(1).returning(|_, _, _, _| Ok(0x42 as efi::Event));
        boot_services
            .expect_install_protocol_interface::<AbsPtr, Box<AbsPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));
        // close_event must be called to clean up the event on install failure.
        boot_services.expect_close_event().times(1).returning(|_| Ok(()));

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let result = AbsolutePointerFfi::install(boot_services, 0x2 as efi::Handle, &mut handler);
        assert_eq!(result.err(), Some(efi::Status::ACCESS_DENIED));
    }

    #[test]
    fn uninstall_succeeds() {
        let boot_services = mock_boot_services();
        boot_services.expect_uninstall_protocol_interface::<AbsPtr, Box<AbsPtr>>().times(1).returning(|_, key| {
            // SAFETY: Reclaiming the Box from the key, mirroring the real uninstall_protocol_interface.
            Ok(unsafe { Box::from_raw(key.ptr_value as *mut AbsPtr) })
        });
        boot_services.expect_close_event().times(1).returning(|_| Ok(()));

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);

        let result = AbsolutePointerFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn uninstall_failure_nulls_pointer_handler() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<AbsPtr, Box<AbsPtr>>()
            .times(1)
            .returning(|_, _| Err(efi::Status::ACCESS_DENIED));

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);
        let raw_ptr = key.ptr_value as *mut AbsPtr;

        let result = AbsolutePointerFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Err(efi::Status::ACCESS_DENIED));

        // Verify pointer_handler was nulled as a safety measure.
        // SAFETY: raw_ptr points to the still-live leaked context (uninstall failed).
        let ctx = unsafe { raw_ptr.as_ref() }.unwrap();
        assert!(ctx.pointer_handler.is_null());

        // SAFETY: Reclaiming the leaked context Box for cleanup.
        drop(unsafe { Box::from_raw(raw_ptr) });
    }

    #[test]
    fn uninstall_returns_error_on_close_event_failure() {
        let boot_services = mock_boot_services();
        boot_services
            .expect_uninstall_protocol_interface::<AbsPtr, Box<AbsPtr>>()
            .times(1)
            // SAFETY: Reclaiming the Box from the key, mirroring the real uninstall_protocol_interface.
            .returning(|_, key| Ok(unsafe { Box::from_raw(key.ptr_value as *mut AbsPtr) }));
        boot_services.expect_close_event().times(1).returning(|_| Err(efi::Status::INVALID_PARAMETER));

        let mut handler = PointerHidHandler::new_for_test(boot_services);
        let key = leaked_context_key(boot_services, &mut handler);

        let result = AbsolutePointerFfi::uninstall(boot_services, 0x2 as efi::Handle, key);
        assert_eq!(result, Err(efi::Status::INVALID_PARAMETER));
    }
}
