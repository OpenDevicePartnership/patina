//! Module for architecture agnostic handling of exceptions. These have to be
//! statically defined
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use crate::{efi_system_context::EfiSystemContext, UefiExceptionHandler};
use spin::rwlock::RwLock;
use uefi_core::error::EfiError;

// Different architecture have a different number of exception types.
const NUM_EXCEPTION_TYPES: usize = if cfg!(target_arch = "x86_64") {
    256
} else if cfg!(target_arch = "aarch64") {
    3
} else {
    panic!("Unimplemented architecture!");
};

// The static exception handlers are needed to track the global state. RwLock is
// used to allow potential nested exceptions.
static EXCEPTION_HANDLERS: [RwLock<Option<UefiExceptionHandler>>; NUM_EXCEPTION_TYPES] = {
    const INIT: RwLock<Option<UefiExceptionHandler>> = RwLock::new(None);
    [INIT; NUM_EXCEPTION_TYPES]
};

/// Registers a handler callback for the provided exception type.
///
/// # Errors
///
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if the exception type is above the expected range.
/// Returns [`AlreadyStarted`](EfiError::AlreadyStarted) if a callback has already been registered.
///
pub fn register_exception_handler(exception_type: usize, handler: UefiExceptionHandler) -> Result<(), EfiError> {
    if exception_type >= NUM_EXCEPTION_TYPES {
        return Err(EfiError::InvalidParameter);
    }

    let mut entry = EXCEPTION_HANDLERS[exception_type].write();
    if (*entry).is_some() {
        return Err(EfiError::AlreadyStarted);
    }

    *entry = Some(handler);
    Ok(())
}

/// Removes a handler callback for the provided exception type.
///
/// # Errors
///
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if the exception type is above the expected range.
/// Returns [`InvalidParameter`](EfiError::InvalidParameter) if no callback currently exists.
///
pub fn unregister_exception_handler(exception_type: usize) -> Result<(), EfiError> {
    if exception_type >= NUM_EXCEPTION_TYPES {
        return Err(EfiError::InvalidParameter);
    }

    let mut entry = EXCEPTION_HANDLERS[exception_type].write();
    if (*entry).is_none() {
        return Err(EfiError::InvalidParameter);
    }

    *entry = None;
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
#[no_mangle]
extern "efiapi" fn exception_handler(exception_type: usize, context: EfiSystemContext) {
    let handler_lock =
        EXCEPTION_HANDLERS[exception_type].try_read().expect("Failed to read lock in exception handler!");
    match *handler_lock {
        Some(handler) => {
            handler(exception_type as u64, context);
        }
        None => {
            log::error!("Unhandled Exception! 0x{:x}", exception_type);
            log::error!("Exception Context: {:#x?}", context.get_arch_context());
            panic! {"Unhandled Exception! 0x{:x}", exception_type};
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::efi_system_context::{EfiSystemContext, EfiSystemContextX64};
    use std::boxed::Box;

    const CALLBACK_EXCEPTION: usize = 0;
    static mut CALLBACK_INVOKED: bool = false;

    extern "efiapi" fn test_callback(exception_type: u64, _context: EfiSystemContext) {
        assert!(exception_type == CALLBACK_EXCEPTION as u64);
        unsafe { CALLBACK_INVOKED = true };
    }

    fn get_empty_system_context() -> EfiSystemContext {
        let mut zeroed_memory: Box<[u8; std::mem::size_of::<EfiSystemContextX64>()]> =
            Box::new([0; std::mem::size_of::<EfiSystemContextX64>()]);

        EfiSystemContext { system_context_x64: zeroed_memory.as_mut_ptr() as *mut EfiSystemContextX64 }
    }

    #[test]
    fn test_exception_registration() {
        let context = get_empty_system_context();
        register_exception_handler(NUM_EXCEPTION_TYPES, test_callback).expect_err("Allowed invalid exception number!");

        register_exception_handler(CALLBACK_EXCEPTION, test_callback).expect("Failed to register exception handler!");
        register_exception_handler(CALLBACK_EXCEPTION, test_callback).expect_err("Allowed double register!");
        exception_handler(CALLBACK_EXCEPTION, context);
        assert!(unsafe { CALLBACK_INVOKED });
        unregister_exception_handler(CALLBACK_EXCEPTION).expect("Failed to unregister handler!");
        unregister_exception_handler(CALLBACK_EXCEPTION).expect_err("Allowed double unregister!");
    }
}
