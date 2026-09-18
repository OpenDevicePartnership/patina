//! Multiprocessor (MP) support for Patina.
//!
//! Provides the architecture abstraction for multiprocessor (MP) support.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::ffi::c_void;

use patina::standard::efi::protocols::mp_services::ApProcedure;

/// A work-item to be executed by an Application Processor (AP).
///
/// This structure acts as a safe wrapper where if the caller satisfies
/// the safety predicates for creating an `ApWorkItem`, then the work item can be
/// safely passed around and executed on the AP.
#[derive(Clone, Copy)]
pub struct ApWorkItem {
    /// The type-specific function wrapper that translates raw pointers back to the
    /// appropriate types to invoke the procedure with the original signature.
    invoke: unsafe fn(*const c_void, *const c_void),
    procedure: *const c_void,
    argument: *const c_void,
}

// SAFETY: The safe constructor requires shared static data that is `Sync`, and
// the unsafe EFI constructor requires the caller to provide equivalent lifetime
// and concurrency guarantees.
unsafe impl Send for ApWorkItem {}
// SAFETY: The same constructor guarantees permit copies of a work item to be
// shared and invoked concurrently on multiple APs.
unsafe impl Sync for ApWorkItem {}

impl ApWorkItem {
    /// Creates an AP work item from a safe Rust function and matching argument.
    ///
    /// The argument is shared because one work item may execute concurrently on
    /// multiple APs. Its static lifetime ensures it remains valid even if a
    /// dispatch times out while an AP is still executing it.
    pub fn new<T>(procedure: fn(&T), argument: &'static T) -> Self
    where
        T: Sync,
    {
        // Create a type-specific invoker for the work item.
        unsafe fn invoke<T>(procedure: *const c_void, argument: *const c_void) {
            // SAFETY: `ApWorkItem::new` stores a `fn(&T)` and `&'static T` together
            // with this invoker, and `T: Sync` permits calls from multiple APs.
            let procedure = unsafe { core::mem::transmute::<*const c_void, fn(&T)>(procedure) };
            // SAFETY: The argument was created from a shared static reference to `T`.
            let argument = unsafe { &*argument.cast::<T>() };
            procedure(argument);
        }

        Self {
            invoke: invoke::<T>,
            procedure: procedure as *const c_void,
            argument: core::ptr::from_ref(argument).cast(),
        }
    }

    /// Creates an AP work item from a raw EFI procedure and argument.
    ///
    /// ## Safety
    ///
    /// The caller must ensure that:
    ///
    /// 1. `argument` is valid for the accesses performed by `procedure`.
    /// 2. `procedure` and `argument` can be used concurrently when copies of the
    ///    work item are dispatched to multiple APs.
    /// 3. `argument` remains valid for every possible invocation of this work
    ///    item and any copies of it, including after a dispatch timeout.
    ///
    pub unsafe fn new_efi(procedure: ApProcedure, argument: *mut c_void) -> Self {
        unsafe fn invoke(procedure: *const c_void, argument: *const c_void) {
            // SAFETY: `ApWorkItem::new_efi` requires the caller to provide a valid
            // EFI procedure and matching argument for the dispatch lifetime.
            let procedure = unsafe { core::mem::transmute::<*const c_void, ApProcedure>(procedure) };
            // SAFETY: The constructor's caller guarantees this argument is valid
            // for the EFI procedure.
            unsafe { procedure(argument.cast_mut()) };
        }

        Self { invoke, procedure: procedure as *const c_void, argument: argument.cast_const() }
    }

    /// Executes this work item on the calling processor.
    pub fn run(self) {
        // SAFETY: The safety guarantees are satisfied by the safety predicates for creating
        //         this `ApWorkItem` instance.
        unsafe { (self.invoke)(self.procedure, self.argument) };
    }
}

#[cfg_attr(coverage, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "efiapi" fn set_boolean(arg: *mut c_void) {
        // SAFETY: The test passes the address of a live `bool` as the argument.
        let value = unsafe { &mut *(arg as *mut bool) };
        *value = true;
    }

    #[test]
    fn create_and_call_efi_work_item() {
        let mut called = false;
        // SAFETY: Procedure and argument match, and argument's lifetime matches work_item.
        let work_item = unsafe { ApWorkItem::new_efi(set_boolean, (&raw mut called).cast()) };
        work_item.run();
        assert!(called);
    }

    #[test]
    fn create_and_call_typed_work_item() {
        static CALLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        CALLED.store(false, core::sync::atomic::Ordering::SeqCst);

        let work_item = ApWorkItem::new(|called| called.store(true, core::sync::atomic::Ordering::SeqCst), &CALLED);
        work_item.run();

        assert!(CALLED.load(core::sync::atomic::Ordering::SeqCst));
    }
}
