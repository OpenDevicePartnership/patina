//! Shared internal state for the DXE Core's UEFI Services implementations.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;
use core::ffi::c_void;

use patina::standard::efi;

use crate::tpl_mutex::TplMutex;

/// One entry per dispatch of a reentrant, LIFO function currently executing, outermost first.
struct ActiveDispatch {
    /// The context of the dispatch running at this stack depth.
    context: *mut c_void,
    /// Set when this dispatch closes or cancels its own registration.
    close_requested: bool,
}

// SAFETY: `ActiveDispatch` values are only ever pushed, inspected, and popped while holding their
// `ActiveDispatchStack`'s lock.
unsafe impl Send for ActiveDispatch {}

/// Tracks contexts currently executing for one dispatch mechanism, so a context can defer freeing
/// itself if closed or cancelled while it, or a nested dispatch acting on its behalf, is still executing.
pub(crate) struct ActiveDispatchStack {
    entries: TplMutex<Vec<ActiveDispatch>>,
}

impl ActiveDispatchStack {
    const fn new(tpl: efi::Tpl, name: &'static str) -> Self {
        Self { entries: TplMutex::new(tpl, Vec::new(), name) }
    }

    /// Records that `context`'s dispatch is now executing.
    pub(crate) fn enter(&self, context: *mut c_void) {
        self.entries.lock().push(ActiveDispatch { context, close_requested: false });
    }

    /// Records that `context`'s dispatch has returned, and reports whether it (or a nested dispatch
    /// acting on its behalf) requested its own context be freed.
    pub(crate) fn exit(&self, context: *mut c_void) -> bool {
        // This is the entry pushed by the matching `enter` call since entries are pushed and popped
        // in call order, so nothing pushed after it would still be on the stack.
        match self.entries.lock().pop() {
            Some(active) => {
                debug_assert_eq!(active.context, context, "active dispatch stack popped out of order");
                active.close_requested
            }
            None => false,
        }
    }

    /// If `context`'s dispatch (or an ancestor's, if called from a nested dispatch) is currently
    /// executing, then this requests to defer freeing its context until its dispatch returns, and
    /// returns `true`. Returns `false` if no matching dispatch is active, so the caller can free the
    /// context immediately instead.
    pub(crate) fn request_close(&self, context: *mut c_void) -> bool {
        let mut entries = self.entries.lock();
        match entries.iter_mut().rev().find(|entry| entry.context == context) {
            Some(active) => {
                active.close_requested = true;
                true
            }
            None => false,
        }
    }
}

/// Shared internal state for the DXE Core's UEFI Services implementations.
pub(crate) struct UefiServicesState {
    /// Tracks reentrant dispatch of `CoreEventServices` notification callbacks.
    pub(crate) event_notify: ActiveDispatchStack,
    /// Tracks reentrant dispatch of `CoreProtocolServices` install-notify callbacks.
    pub(crate) protocol_install_notify: ActiveDispatchStack,
}

impl UefiServicesState {
    const fn new() -> Self {
        Self {
            event_notify: ActiveDispatchStack::new(efi::TPL_HIGH_LEVEL, "ActiveNotifyContexts"),
            protocol_install_notify: ActiveDispatchStack::new(efi::TPL_HIGH_LEVEL, "ActiveInstallNotifies"),
        }
    }
}

pub(crate) static UEFI_SERVICES_STATE: UefiServicesState = UefiServicesState::new();

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::{events::restore_tpl, test_support};

    /// Runs `f` serialized against other tests, with the shared TPL restored to `TPL_APPLICATION`.
    fn with_locked_state<F: Fn() + std::panic::RefUnwindSafe>(f: F) {
        test_support::with_global_lock(|| {
            restore_tpl(efi::TPL_APPLICATION);
            f();
        })
        .unwrap();
    }

    #[test]
    fn test_active_dispatch_stack_new_starts_with_no_active_entries() {
        with_locked_state(|| {
            let stack = ActiveDispatchStack::new(efi::TPL_NOTIFY, "test stack");
            let context = core::ptr::null_mut();

            // A newly constructed stack has nothing to pop or flag as closing.
            assert!(!stack.exit(context));
            assert!(!stack.request_close(context));
        });
    }

    #[test]
    fn test_uefi_services_state_new_starts_with_empty_stacks() {
        with_locked_state(|| {
            let state = UefiServicesState::new();
            let context = core::ptr::null_mut();

            assert!(!state.event_notify.exit(context));
            assert!(!state.protocol_install_notify.exit(context));
        });
    }
}
