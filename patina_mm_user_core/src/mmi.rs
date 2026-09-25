//! MMI (Management Mode Interrupt) Handler Database
//!
//! This module manages the registration and dispatch of MMI handlers, following the
//! same patterns as the C `Mmi.c` in `StandaloneMmPkg/Core`.
//!
//! ## Handler Types
//!
//! - **Root handlers**: Registered with `handler_type = None`. Called on every MMI regardless
//!   of the communication buffer contents. Used for hardware-level interrupt sources.
//! - **GUID-specific handlers**: Registered with a specific GUID. Called only when an MMI
//!   communication targets that GUID.
//!
//! ## External vs Internal Handlers
//!
//! The database supports two calling conventions:
//! - **External** (`MmiHandlerEntryPoint`): `unsafe extern "efiapi" fn` — used by drivers
//!   registering through the MMST `MmiHandlerRegister` service.
//! - **Internal** (`InternalMmiHandler`): Safe Rust `fn` — used by the core's own lifecycle
//!   handlers (ready-to-lock, end-of-DXE, etc.) without going through the C ABI.
//!
//! ## Dispatch Flow
//!
//! [`MmiDatabase::mmi_manage`] is the main dispatch entry point:
//! 1. If `handler_type` is `None`, iterate root handlers
//! 2. If `handler_type` is `Some(guid)`, find the `MmiEntry` for that GUID and iterate its handlers
//! 3. Each handler returns a status that determines whether dispatch continues
//!
//! **Lock safety**: The database lock is released before calling handlers and
//! re-acquired afterwards, so handlers may safely call `mmi_handler_register` or
//! `mmi_handler_unregister` without deadlocking.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{vec, vec::Vec};
use core::ffi::c_void;

use patina::standard::efi;
use spin::Mutex;

/// MMI handler entry point signature (external / C ABI).
///
/// Re-exported from [`patina::pi::mm_cis::MmiHandlerEntryPoint`].
use patina::pi::mm_cis::MmiHandlerEntryPoint;

/// `EFI_WARN_INTERRUPT_SOURCE_QUIESCED` — PI spec warning status code.
/// Indicates an interrupt source was quiesced.
const WARN_INTERRUPT_SOURCE_QUIESCED: efi::Status = efi::Status::from_usize(3);

/// `EFI_WARN_INTERRUPT_SOURCE_PENDING` — PI spec warning status code.
/// Indicates an interrupt source was processed but not quiesced.
const WARN_INTERRUPT_SOURCE_PENDING: efi::Status = efi::Status::from_usize(2);

/// `EFI_INTERRUPT_PENDING` — PI spec status for pending interrupts.
const INTERRUPT_PENDING: efi::Status = efi::Status::from_usize(0x80000000 | 0x00000004);

/// Signature for internal (Rust-native) MMI handlers.
///
/// These are registered by the core itself for lifecycle events and do not go
/// through the `unsafe extern "efiapi"` calling convention. `handler_type` is the GUID that
/// triggered the handler (the same GUID used at registration), `comm_buffer` points to the
/// communication data (may be null for async MMIs), and `comm_buffer_size` is a mutable pointer
/// to the communication buffer size. The handler returns an [`efi::Status`] following the
/// standard `MmiManage` return protocol.
pub type InternalMmiHandler =
    fn(handler_type: &efi::Guid, comm_buffer: *mut c_void, comm_buffer_size: *mut usize) -> efi::Status;

/// An MMI handler callback — either an external (C ABI) or internal (Rust) function.
#[derive(Clone, Copy)]
enum HandlerKind {
    /// External handler registered by a driver through the MMST.
    External(MmiHandlerEntryPoint),
    /// Internal handler registered by the MM Core directly.
    Internal(InternalMmiHandler),
}

impl core::fmt::Debug for HandlerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Print only the variant; the wrapped function pointers are not meaningfully
        // formattable and their ABI does not guarantee a `Debug` impl.
        match self {
            HandlerKind::External(_) => f.write_str("External"),
            HandlerKind::Internal(_) => f.write_str("Internal"),
        }
    }
}

/// An MMI entry groups all handlers registered for a specific GUID.
#[derive(Clone)]
struct MmiEntry {
    /// The handler type GUID.
    handler_type: efi::Guid,
    /// All handlers registered for this GUID.
    handlers: Vec<MmiHandler>,
}

/// A registered MMI handler.
#[derive(Clone, Copy)]
struct MmiHandler {
    /// The handler callback.
    kind: HandlerKind,
    /// Monotonic ID used as the dispatch handle for unregistering.
    id: usize,
    /// Whether this handler is marked for removal (deferred removal during dispatch).
    to_remove: bool,
}

/// The MMI handler database.
///
/// Manages root handlers (called for all MMIs) and GUID-specific handlers.
/// Thread-safe via internal `Mutex`.
pub struct MmiDatabase {
    /// Internal state protected by a mutex.
    inner: Mutex<MmiDatabaseInner>,
}

struct MmiDatabaseInner {
    /// Root MMI handlers (called for every MMI, regardless of GUID).
    root_handlers: Vec<MmiHandler>,
    /// GUID-specific MMI entries.
    entries: Vec<MmiEntry>,
    /// Re-entrance depth counter for `mmi_manage`.
    manage_calling_depth: usize,
    /// Monotonic ID counter for handler dispatch handles.
    next_id: usize,
}

impl Default for MmiDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl MmiDatabase {
    /// Creates a new empty `MmiDatabase`.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(MmiDatabaseInner {
                root_handlers: Vec::new(),
                entries: Vec::new(),
                manage_calling_depth: 0,
                next_id: 1,
            }),
        }
    }

    /// Register an external (C ABI) MMI handler.
    ///
    /// If `handler_type` is `None`, the handler is registered as a root handler.
    /// If `handler_type` is `Some(guid)`, the handler is registered for that specific GUID.
    ///
    /// Returns `Ok(dispatch_handle)` on success, where `dispatch_handle` is an opaque handle
    /// that can be used to unregister the handler.
    pub fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        let mut inner = self.inner.lock();
        let id = inner.next_id;
        inner.next_id += 1;

        let mmi_handler = MmiHandler { kind: HandlerKind::External(handler), id, to_remove: false };

        Self::insert_handler(&mut inner, handler_type, mmi_handler);

        let handle = id as efi::Handle;
        log::info!("Registered external MMI handler id={} for {:?}", id, handler_type.map(patina::Guid::from_ref));
        Ok(handle)
    }

    /// Register an internal (Rust-native) MMI handler.
    ///
    /// Works like [`mmi_handler_register`](Self::mmi_handler_register) but takes a safe
    /// Rust function pointer instead of an `unsafe extern "efiapi" fn`.
    ///
    /// Returns the dispatch handle (an opaque `usize`-based ID) on success.
    pub fn register_internal_handler(
        &self,
        handler: InternalMmiHandler,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        let mut inner = self.inner.lock();
        let id = inner.next_id;
        inner.next_id += 1;

        let mmi_handler = MmiHandler { kind: HandlerKind::Internal(handler), id, to_remove: false };

        Self::insert_handler(&mut inner, handler_type, mmi_handler);

        let handle = id as efi::Handle;
        log::debug!("Registered internal MMI handler id={} for {:?}", id, handler_type.map(patina::Guid::from_ref));
        Ok(handle)
    }

    /// Insert a handler into the appropriate list (root or GUID-specific).
    fn insert_handler(inner: &mut MmiDatabaseInner, handler_type: Option<&efi::Guid>, handler: MmiHandler) {
        match handler_type {
            None => {
                inner.root_handlers.push(handler);
            }
            Some(guid) => {
                if let Some(entry) = inner.entries.iter_mut().find(|e| e.handler_type == *guid) {
                    entry.handlers.push(handler);
                } else {
                    inner.entries.push(MmiEntry { handler_type: *guid, handlers: vec![handler] });
                }
            }
        }
    }

    /// Unregister an MMI handler by its dispatch handle.
    ///
    /// If we are inside a dispatch (`manage_calling_depth > 0`) the handler is
    /// marked for deferred removal. Otherwise it is removed immediately.
    pub fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        let target_id = dispatch_handle as usize;
        let mut inner = self.inner.lock();

        // Search root handlers
        for handler in &mut inner.root_handlers {
            if handler.id == target_id {
                handler.to_remove = true;
                log::debug!("Marked root MMI handler id={target_id} for removal.");
                if inner.manage_calling_depth == 0 {
                    Self::cleanup_removed_handlers(&mut inner);
                }
                return Ok(());
            }
        }

        // Search GUID-specific handlers
        for entry in &mut inner.entries {
            for handler in &mut entry.handlers {
                if handler.id == target_id {
                    handler.to_remove = true;
                    log::debug!(
                        "Marked MMI handler id={target_id} for removal (GUID: {}).",
                        patina::Guid::from_ref(&entry.handler_type)
                    );
                    if inner.manage_calling_depth == 0 {
                        Self::cleanup_removed_handlers(&mut inner);
                    }
                    return Ok(());
                }
            }
        }

        log::warn!("MMI handler {dispatch_handle:?} not found for unregistering.");
        Err(efi::Status::NOT_FOUND)
    }

    /// Manage (dispatch) an MMI.
    ///
    /// This is the main dispatch function, equivalent to the C `MmiManage`.
    ///
    /// - If `handler_type` is `None`, root handlers are dispatched.
    /// - If `handler_type` is `Some(guid)`, the handlers for that GUID are dispatched.
    ///
    /// **Lock safety**: The database lock is released before calling any handler
    /// and re-acquired afterwards, so handlers may call `mmi_handler_register` /
    /// `mmi_handler_unregister` without deadlocking.
    ///
    /// Returns:
    /// - `EFI_SUCCESS` if at least one handler returned success
    /// - `EFI_WARN_INTERRUPT_SOURCE_QUIESCED` if a source was quiesced
    /// - `EFI_INTERRUPT_PENDING` if a handler indicated the interrupt is still pending
    /// - `EFI_NOT_FOUND` if no handlers are registered for the given type
    pub fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        // ----- Phase 1: snapshot handlers under the lock -----
        let handlers_snapshot = {
            let mut inner = self.inner.lock();
            inner.manage_calling_depth += 1;

            match handler_type {
                None => inner.root_handlers.iter().filter(|h| !h.to_remove).copied().collect::<Vec<_>>(),
                Some(guid) => {
                    if let Some(entry) = inner.entries.iter().find(|e| e.handler_type == *guid) {
                        entry.handlers.iter().filter(|h| !h.to_remove).copied().collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    }
                }
            }
            // lock released here
        };

        log::info!("Dispatching MMI with handler_type = {:?}", handler_type.map(patina::Guid::from_ref));
        // ----- Phase 2: dispatch without the lock held -----
        let return_status =
            Self::dispatch_handler_snapshot(&handlers_snapshot, handler_type, context, comm_buffer, comm_buffer_size);

        // ----- Phase 3: update depth and clean up under the lock -----
        let mut inner = self.inner.lock();
        inner.manage_calling_depth -= 1;

        if inner.manage_calling_depth == 0 {
            Self::cleanup_removed_handlers(&mut inner);
        }

        return_status
    }

    /// Dispatch a snapshot of handlers. The database lock is NOT held.
    fn dispatch_handler_snapshot(
        handlers: &[MmiHandler],
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        if handlers.is_empty() {
            return efi::Status::NOT_FOUND;
        }

        let short_circuit = handler_type.is_some();

        let mut return_status = efi::Status::NOT_FOUND;

        // Provide a dummy GUID for root dispatch (handlers don't use it).
        let null_guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
        let guid_ref = handler_type.unwrap_or(&null_guid);

        for handler in handlers {
            log::info!("Dispatching handler with id = {}, kind = {:?}", handler.id, handler.kind);
            let status = match handler.kind {
                HandlerKind::External(entry_point) => {
                    // SAFETY: External handler follows the PI spec efiapi calling convention.
                    // The dispatch_handle is the monotonic ID cast to a handle.
                    unsafe { entry_point(handler.id as efi::Handle, context, comm_buffer, comm_buffer_size) }
                }
                HandlerKind::Internal(fn_ptr) => fn_ptr(guid_ref, comm_buffer, comm_buffer_size),
            };

            match status {
                efi::Status::SUCCESS => {
                    return_status = efi::Status::SUCCESS;
                    if short_circuit {
                        log::info!("Short-circuiting after successful handler dispatch.");
                        break;
                    }
                }
                s if s == INTERRUPT_PENDING => {
                    if short_circuit {
                        log::info!("Short-circuiting due to pending interrupt.");
                        return INTERRUPT_PENDING;
                    }
                    if return_status != efi::Status::SUCCESS {
                        return_status = status;
                    }
                }
                s if s == WARN_INTERRUPT_SOURCE_QUIESCED => {
                    return_status = efi::Status::SUCCESS;
                }
                s if s == WARN_INTERRUPT_SOURCE_PENDING && return_status != efi::Status::SUCCESS => {
                    return_status = status;
                }
                _ => {
                    // Other statuses are ignored per PI spec
                }
            }
        }
        log::info!("Finished dispatching handlers with final status = {:#x}", return_status.as_usize());
        return_status
    }

    /// Remove handlers marked with `to_remove` and clean up empty entries.
    fn cleanup_removed_handlers(inner: &mut MmiDatabaseInner) {
        inner.root_handlers.retain(|h| !h.to_remove);

        inner.entries.retain_mut(|entry| {
            entry.handlers.retain(|h| !h.to_remove);
            !entry.handlers.is_empty()
        });
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static GUID_X: efi::Guid = efi::Guid::from_fields(0x1111_0001, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
    static GUID_Y: efi::Guid = efi::Guid::from_fields(0x2222_0002, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 2]);

    /// The database the re-entrant handlers below reach back into. One per test process.
    static DB: MmiDatabase = MmiDatabase::new();
    /// Tags recorded by each handler as it runs, in dispatch order.
    static CALLS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    const RAN_A: usize = 1;
    const RAN_B: usize = 2;
    const RAN_PENDING: usize = 3;
    const RAN_QUIESCED: usize = 4;
    const RAN_SOURCE_PENDING: usize = 5;
    const RAN_UNSUPPORTED: usize = 6;
    const RAN_SELF_UNREGISTER: usize = 7;
    const RAN_REDISPATCH: usize = 8;
    const RAN_INTERNAL: usize = 9;
    /// Dispatch handle a re-entrant handler should unregister, and what it observed.
    static VICTIM: AtomicUsize = AtomicUsize::new(0);
    static NESTED_STATUS: AtomicUsize = AtomicUsize::new(0);
    /// GUID the most recent internal handler was invoked with.
    static INTERNAL_GUID: Mutex<Option<efi::Guid>> = Mutex::new(None);

    fn record(tag: usize) {
        CALLS.lock().push(tag);
    }

    fn calls() -> Vec<usize> {
        CALLS.lock().clone()
    }

    macro_rules! external_handler {
        ($name:ident, $tag:expr, $status:expr) => {
            unsafe extern "efiapi" fn $name(
                _dispatch_handle: efi::Handle,
                _context: *const c_void,
                _comm_buffer: *mut c_void,
                _comm_buffer_size: *mut usize,
            ) -> efi::Status {
                record($tag);
                $status
            }
        };
    }

    external_handler!(succeeds_a, RAN_A, efi::Status::SUCCESS);
    external_handler!(succeeds_b, RAN_B, efi::Status::SUCCESS);
    external_handler!(reports_pending, RAN_PENDING, INTERRUPT_PENDING);
    external_handler!(reports_quiesced, RAN_QUIESCED, WARN_INTERRUPT_SOURCE_QUIESCED);
    external_handler!(reports_source_pending, RAN_SOURCE_PENDING, WARN_INTERRUPT_SOURCE_PENDING);
    external_handler!(reports_unsupported, RAN_UNSUPPORTED, efi::Status::UNSUPPORTED);

    /// Doubles the caller's size and stamps the buffer, proving both pointers arrive intact.
    unsafe extern "efiapi" fn echoes_comm_buffer(
        _dispatch_handle: efi::Handle,
        _context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        // SAFETY: the tests below always pass a live `usize` and a live byte buffer.
        unsafe {
            *comm_buffer_size *= 2;
            *comm_buffer.cast::<u8>() = 0xCD;
        }
        efi::Status::SUCCESS
    }

    /// Unregisters itself while it is being dispatched.
    unsafe extern "efiapi" fn unregisters_itself(
        dispatch_handle: efi::Handle,
        _context: *const c_void,
        _comm_buffer: *mut c_void,
        _comm_buffer_size: *mut usize,
    ) -> efi::Status {
        record(RAN_SELF_UNREGISTER);
        DB.mmi_handler_unregister(dispatch_handle).expect("the running handler is registered");
        efi::Status::SUCCESS
    }

    /// Unregisters another handler and immediately re-dispatches its type.
    unsafe extern "efiapi" fn unregisters_victim_then_redispatches(
        _dispatch_handle: efi::Handle,
        _context: *const c_void,
        _comm_buffer: *mut c_void,
        _comm_buffer_size: *mut usize,
    ) -> efi::Status {
        record(RAN_REDISPATCH);
        let victim = VICTIM.load(Ordering::Relaxed) as efi::Handle;
        DB.mmi_handler_unregister(victim).expect("the victim is registered");

        let nested = DB.mmi_manage(Some(&GUID_X), core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut());
        NESTED_STATUS.store(nested.as_usize(), Ordering::Relaxed);
        efi::Status::SUCCESS
    }

    fn internal_recording(handler_type: &efi::Guid, _: *mut c_void, _: *mut usize) -> efi::Status {
        record(RAN_INTERNAL);
        *INTERNAL_GUID.lock() = Some(*handler_type);
        efi::Status::SUCCESS
    }

    /// Dispatches `handler_type` with no context or buffer.
    fn manage(db: &MmiDatabase, handler_type: Option<&efi::Guid>) -> efi::Status {
        db.mmi_manage(handler_type, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut())
    }

    fn register(db: &MmiDatabase, handler: MmiHandlerEntryPoint, guid: Option<&efi::Guid>) -> efi::Handle {
        db.mmi_handler_register(handler, guid).expect("registration succeeds")
    }

    #[test]
    fn test_dispatching_an_empty_database_reports_not_found() {
        let db = MmiDatabase::default();

        assert_eq!(manage(&db, None), efi::Status::NOT_FOUND);
        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_a_guid_handler_only_runs_for_its_own_type() {
        let db = MmiDatabase::new();
        register(&db, succeeds_a, Some(&GUID_X));

        assert_eq!(manage(&db, Some(&GUID_Y)), efi::Status::NOT_FOUND);
        assert_eq!(manage(&db, None), efi::Status::NOT_FOUND, "a typed handler is not a root handler");
        assert!(calls().is_empty());

        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A]);
    }

    #[test]
    fn test_root_handlers_run_for_every_mmi_and_are_not_short_circuited() {
        let db = MmiDatabase::new();
        register(&db, succeeds_a, None);
        register(&db, succeeds_b, None);

        assert_eq!(manage(&db, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A, RAN_B], "every root handler runs even after one succeeds");
    }

    #[test]
    fn test_a_typed_dispatch_stops_at_the_first_successful_handler() {
        let db = MmiDatabase::new();
        register(&db, succeeds_a, Some(&GUID_X));
        register(&db, succeeds_b, Some(&GUID_X));

        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A], "dispatch short-circuits once a handler succeeds");
    }

    #[test]
    fn test_a_typed_dispatch_stops_immediately_on_a_pending_interrupt() {
        let db = MmiDatabase::new();
        register(&db, reports_pending, Some(&GUID_X));
        register(&db, succeeds_a, Some(&GUID_X));

        assert_eq!(manage(&db, Some(&GUID_X)), INTERRUPT_PENDING);
        assert_eq!(calls(), vec![RAN_PENDING], "a pending interrupt short-circuits the rest");
    }

    #[test]
    fn test_a_root_pending_interrupt_is_reported_but_does_not_stop_dispatch() {
        let db = MmiDatabase::new();
        register(&db, reports_pending, None);
        register(&db, reports_unsupported, None);

        assert_eq!(manage(&db, None), INTERRUPT_PENDING);
        assert_eq!(calls(), vec![RAN_PENDING, RAN_UNSUPPORTED]);
    }

    #[test]
    fn test_a_success_outranks_a_pending_interrupt_from_another_root_handler() {
        let db = MmiDatabase::new();
        register(&db, succeeds_a, None);
        register(&db, reports_pending, None);

        assert_eq!(manage(&db, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A, RAN_PENDING]);
    }

    #[test]
    fn test_a_quiesced_source_is_reported_as_success() {
        let db = MmiDatabase::new();
        register(&db, reports_quiesced, None);

        assert_eq!(manage(&db, None), efi::Status::SUCCESS);
    }

    #[test]
    fn test_a_pending_source_is_reported_only_when_nothing_succeeded() {
        let db = MmiDatabase::new();
        register(&db, reports_source_pending, None);
        assert_eq!(manage(&db, None), WARN_INTERRUPT_SOURCE_PENDING);

        register(&db, succeeds_a, None);
        assert_eq!(manage(&db, None), efi::Status::SUCCESS, "a success outranks a pending source");
    }

    #[test]
    fn test_an_unhandled_status_leaves_the_mmi_unclaimed() {
        let db = MmiDatabase::new();
        register(&db, reports_unsupported, None);

        assert_eq!(manage(&db, None), efi::Status::NOT_FOUND, "statuses outside the PI set are ignored");
        assert_eq!(calls(), vec![RAN_UNSUPPORTED]);
    }

    #[test]
    fn test_the_communication_buffer_and_size_reach_the_handler() {
        let db = MmiDatabase::new();
        register(&db, echoes_comm_buffer, Some(&GUID_X));

        let mut buffer = [0u8; 4];
        let mut size = 4usize;
        assert_eq!(
            db.mmi_manage(Some(&GUID_X), core::ptr::null(), buffer.as_mut_ptr().cast(), &raw mut size),
            efi::Status::SUCCESS
        );
        assert_eq!(size, 8);
        assert_eq!(buffer[0], 0xCD);
    }

    #[test]
    fn test_handlers_sharing_a_guid_join_the_same_entry() {
        let db = MmiDatabase::new();
        register(&db, reports_unsupported, Some(&GUID_X));
        register(&db, succeeds_a, Some(&GUID_X));
        register(&db, succeeds_b, Some(&GUID_Y));

        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_UNSUPPORTED, RAN_A], "both handlers for the GUID ran, in registration order");
    }

    #[test]
    fn test_registration_hands_out_distinct_dispatch_handles() {
        let db = MmiDatabase::new();

        let first = register(&db, succeeds_a, None);
        let second = register(&db, succeeds_b, Some(&GUID_X));
        let third = db.register_internal_handler(internal_recording, None).expect("registration succeeds");

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }

    #[test]
    fn test_unregistering_removes_a_root_handler_immediately() {
        let db = MmiDatabase::new();
        let handle = register(&db, succeeds_a, None);
        register(&db, succeeds_b, None);

        assert_eq!(db.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&db, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_B]);
    }

    #[test]
    fn test_unregistering_the_last_handler_for_a_guid_retires_the_entry() {
        let db = MmiDatabase::new();
        let handle = register(&db, succeeds_a, Some(&GUID_X));

        assert_eq!(db.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::NOT_FOUND);
        assert!(calls().is_empty());
    }

    #[test]
    fn test_unregistering_an_unknown_handle_is_reported_not_found() {
        let db = MmiDatabase::new();
        register(&db, succeeds_a, None);
        register(&db, succeeds_b, Some(&GUID_X));

        assert_eq!(db.mmi_handler_unregister(core::ptr::without_provenance_mut(0x999)), Err(efi::Status::NOT_FOUND));
    }

    #[test]
    fn test_a_handler_may_unregister_itself_while_it_is_running() {
        register(&DB, unregisters_itself, None);
        register(&DB, succeeds_a, None);

        assert_eq!(manage(&DB, None), efi::Status::SUCCESS);
        assert_eq!(
            calls(),
            vec![RAN_SELF_UNREGISTER, RAN_A],
            "removal is deferred, so the rest of the snapshot still runs"
        );

        assert_eq!(manage(&DB, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_SELF_UNREGISTER, RAN_A, RAN_A], "the handler was removed once dispatch finished");
    }

    #[test]
    fn test_a_handler_marked_for_removal_is_not_dispatched_by_a_nested_mmi() {
        let victim = register(&DB, succeeds_b, Some(&GUID_X));
        VICTIM.store(victim as usize, Ordering::Relaxed);
        register(&DB, unregisters_victim_then_redispatches, None);

        assert_eq!(manage(&DB, None), efi::Status::SUCCESS);
        assert_eq!(
            NESTED_STATUS.load(Ordering::Relaxed),
            efi::Status::NOT_FOUND.as_usize(),
            "the nested dispatch skipped the handler that was marked for removal"
        );
        assert_eq!(calls(), vec![RAN_REDISPATCH], "the victim never ran");

        assert_eq!(manage(&DB, Some(&GUID_X)), efi::Status::NOT_FOUND, "the victim was cleaned up afterwards");
    }

    #[test]
    fn test_an_internal_handler_receives_the_guid_it_was_registered_for() {
        let db = MmiDatabase::new();
        db.register_internal_handler(internal_recording, Some(&GUID_X)).expect("registration succeeds");

        assert_eq!(manage(&db, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_INTERNAL]);
        assert_eq!(*INTERNAL_GUID.lock(), Some(GUID_X));
    }

    #[test]
    fn test_a_root_internal_handler_is_dispatched_with_a_null_guid() {
        let db = MmiDatabase::new();
        let handle = db.register_internal_handler(internal_recording, None).expect("registration succeeds");

        assert_eq!(manage(&db, None), efi::Status::SUCCESS);
        assert_eq!(*INTERNAL_GUID.lock(), Some(efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6])));

        assert_eq!(db.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&db, None), efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_handler_kinds_are_distinguishable_when_logged() {
        assert_eq!(alloc::format!("{:?}", HandlerKind::External(succeeds_a)), "External");
        assert_eq!(alloc::format!("{:?}", HandlerKind::Internal(internal_recording)), "Internal");
    }
}
