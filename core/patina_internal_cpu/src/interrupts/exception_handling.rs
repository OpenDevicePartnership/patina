//! Module for architecture agnostic handling of exceptions. These have to be
//! statically defined
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use patina::error::EfiError;
use patina::pi::protocols::cpu_arch::EfiExceptionType;
use spin::rwlock::RwLock;

use crate::interrupts::EfiExceptionStackTrace;

use super::{EfiSystemContextFactory, ExceptionContext, ExceptionType, HandlerType};

// Different architecture have a different number of exception types.
const NUM_EXCEPTION_TYPES: ExceptionType = if cfg!(test) {
    8
} else if cfg!(target_arch = "x86_64") {
    256
} else if cfg!(target_arch = "aarch64") {
    3
} else {
    panic!("Unimplemented architecture!");
};

// The static exception handlers are needed to track the global state. RwLock is
// used to allow potential nested exceptions.
static EXCEPTION_HANDLERS: [RwLock<HandlerType>; NUM_EXCEPTION_TYPES] = {
    // This clippy warning can be ignored. We are purposefully generating a different `INIT` const for each element.
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: RwLock<HandlerType> = RwLock::new(HandlerType::None);
    [INIT; NUM_EXCEPTION_TYPES]
};

/// Registers a handler callback for the provided exception type.
///
/// # Errors
///
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if the exception type is above the expected range.
/// Returns [`AlreadyStarted`](EfiError::AlreadyStarted) if a callback has already been registered.
///
pub(crate) fn register_exception_handler(exception_type: ExceptionType, handler: HandlerType) -> Result<(), EfiError> {
    if handler.is_none() {
        return Err(EfiError::InvalidParameter);
    }

    if exception_type >= NUM_EXCEPTION_TYPES {
        return Err(EfiError::InvalidParameter);
    }

    let mut entry = EXCEPTION_HANDLERS[exception_type].write();
    if !(*entry).is_none() {
        return Err(EfiError::AlreadyStarted);
    }

    *entry = handler;
    Ok(())
}

/// Removes a handler callback for the provided exception type.
///
/// # Errors
///
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if the exception type is above the expected range.
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if no callback currently exists.
///
pub(crate) fn unregister_exception_handler(exception_type: ExceptionType) -> Result<(), EfiError> {
    if exception_type >= NUM_EXCEPTION_TYPES {
        return Err(EfiError::InvalidParameter);
    }

    let mut entry = EXCEPTION_HANDLERS[exception_type].write();
    if (*entry).is_none() {
        return Err(EfiError::InvalidParameter);
    }

    *entry = HandlerType::None;
    Ok(())
}

/// The architecture agnostic entry of the exception handler stack.
///
/// This will be invoked by the architectures assembly entry and so requires
/// EFIAPI for a consistent calling convention.
///
/// # Panics
///
/// Panics if no callback has been registered for a given exception or the handler
/// read lock cannot be acquired.
///
#[unsafe(no_mangle)]
extern "efiapi" fn exception_handler(exception_type: usize, context: &mut ExceptionContext) {
    let handler_lock =
        EXCEPTION_HANDLERS[exception_type].try_read().expect("Failed to read lock in exception handler!");

    match *handler_lock {
        HandlerType::UefiRoutine(handler) => {
            let efi_system_context = context.create_efi_system_context();
            handler(exception_type as EfiExceptionType, efi_system_context);
        }
        HandlerType::Handler(handler) => {
            handler.handle_interrupt(exception_type, context);
        }
        HandlerType::None => {
            log::error!("Unhandled Exception! {exception_type:#X}");
            log::error!("");
            context.dump_system_context_registers();
            log::error!("");
            context.dump_stack_trace();
            panic!("Unhandled Exception! {exception_type:#X}");
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    extern crate std;

    use patina::pi::protocols::cpu_arch::EfiSystemContext;

    use super::*;
    use core::sync::atomic::AtomicBool;

    const CALLBACK_EXCEPTION: usize = 0;
    const HANDLER_EXCEPTION: usize = 1;
    static mut CALLBACK_INVOKED: bool = false;

    struct TestHandler {
        pub invoked: AtomicBool,
    }

    impl crate::interrupts::InterruptHandler for TestHandler {
        fn handle_interrupt(&'static self, exception_type: usize, _context: &mut ExceptionContext) {
            assert!(exception_type == HANDLER_EXCEPTION);
            self.invoked.store(true, core::sync::atomic::Ordering::SeqCst);
        }
    }

    extern "efiapi" fn test_callback(exception_type: EfiExceptionType, _context: EfiSystemContext) {
        assert!(exception_type == CALLBACK_EXCEPTION as EfiExceptionType);
        // SAFETY: This is a test only static mutable variable.
        unsafe { CALLBACK_INVOKED = true };
    }

    #[test]
    fn test_uefi_routine() {
        let mut context = crate::interrupts::null::ExceptionContextNull {};
        register_exception_handler(NUM_EXCEPTION_TYPES, HandlerType::UefiRoutine(test_callback))
            .expect_err("Allowed invalid exception number!");

        register_exception_handler(CALLBACK_EXCEPTION, HandlerType::UefiRoutine(test_callback))
            .expect("Failed to register exception handler!");
        register_exception_handler(CALLBACK_EXCEPTION, HandlerType::UefiRoutine(test_callback))
            .expect_err("Allowed double register!");
        exception_handler(CALLBACK_EXCEPTION, &mut context);
        // SAFETY: This is a test only static mutable variable.
        assert!(unsafe { CALLBACK_INVOKED });
        unregister_exception_handler(CALLBACK_EXCEPTION).expect("Failed to unregister handler!");
        unregister_exception_handler(CALLBACK_EXCEPTION).expect_err("Allowed double unregister!");
    }

    #[test]
    fn test_handler() {
        let mut context = crate::interrupts::null::ExceptionContextNull {};
        let handler = Box::leak(Box::new(TestHandler { invoked: AtomicBool::new(false) }));

        register_exception_handler(NUM_EXCEPTION_TYPES, HandlerType::Handler(handler))
            .expect_err("Allowed invalid exception number!");

        register_exception_handler(HANDLER_EXCEPTION, HandlerType::Handler(handler))
            .expect("Failed to register exception handler!");
        register_exception_handler(HANDLER_EXCEPTION, HandlerType::Handler(handler))
            .expect_err("Allowed double register!");

        exception_handler(HANDLER_EXCEPTION, &mut context);
        assert!(handler.invoked.load(core::sync::atomic::Ordering::SeqCst));

        unregister_exception_handler(HANDLER_EXCEPTION).expect("Failed to unregister handler!");
        unregister_exception_handler(HANDLER_EXCEPTION).expect_err("Allowed double unregister!");
    }

    #[test]
    fn test_invalid_input() {
        register_exception_handler(NUM_EXCEPTION_TYPES, HandlerType::UefiRoutine(test_callback))
            .expect_err("Allowed N+1 exception type registration!");

        register_exception_handler(0, HandlerType::None).expect_err("Allowed none exception handler registration!");
    }
}
