//! MMI (Management Mode Interrupt) Handler Database
//!
//! Registers and dispatches MMI handlers, following the C `Mmi.c` in
//! `StandaloneMmPkg/Core`.
//!
//! Root handlers are registered with `handler_type = None` and run on every MMI, whatever
//! the communication buffer holds. They serve hardware interrupt sources. GUID handlers
//! run only when an MMI targets their GUID.
//!
//! Handlers come in two calling conventions:
//! - External (`MmiHandlerEntryPoint`): an `unsafe extern "efiapi" fn` registered by a
//!   driver through the MMST `MmiHandlerRegister` service.
//! - Internal (`InternalMmiHandler`): a safe Rust `fn` registered by the core for its own
//!   lifecycle events (ready-to-lock, end-of-DXE, and so on).
//!
//! `MmiDatabase::mmi_manage` dispatches either the root handlers (`handler_type` is
//! `None`) or the handlers for one GUID. A GUID dispatch stops at the first handler that
//! returns success or `INTERRUPT_PENDING`; a root dispatch runs every handler.
//!
//! The database lock is released while handlers run, so a handler may call
//! `mmi_handler_register` or `mmi_handler_unregister` without deadlocking.
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

use patina::pi::mm_cis::MmiHandlerEntryPoint;

/// `MAX_BIT >> 2`, which `PI_ENCODE_WARNING` and `PI_ENCODE_ERROR` set so PI status codes do not
/// collide with UEFI ones (`MdePkg/Include/Pi/PiMultiPhase.h`).
const PI_STATUS_BIT: usize = 1 << (usize::BITS - 3);

/// `MAX_BIT`, which marks a status as an error.
const ERROR_BIT: usize = 1 << (usize::BITS - 1);

/// `EFI_WARN_INTERRUPT_SOURCE_QUIESCED`, `PI_ENCODE_WARNING (1)`: an interrupt source was
/// quiesced.
const WARN_INTERRUPT_SOURCE_QUIESCED: efi::Status = efi::Status::from_usize(PI_STATUS_BIT | 1);

/// `EFI_WARN_INTERRUPT_SOURCE_PENDING`, `PI_ENCODE_WARNING (0)`: an interrupt source was
/// processed but not quiesced.
const WARN_INTERRUPT_SOURCE_PENDING: efi::Status = efi::Status::from_usize(PI_STATUS_BIT);

/// `EFI_INTERRUPT_PENDING`, `PI_ENCODE_ERROR (0)`.
const INTERRUPT_PENDING: efi::Status = efi::Status::from_usize(ERROR_BIT | PI_STATUS_BIT);

/// Signature of an internal (Rust) MMI handler.
///
/// The core registers these for lifecycle events; they do not use the `efiapi` ABI.
/// `handler_type` is the GUID the handler was registered for (a zero GUID for root
/// handlers), `comm_buffer` is the communication data (may be null), and
/// `comm_buffer_size` points to its size. The return value follows the `MmiManage`
/// status rules.
pub type InternalMmiHandler =
    fn(handler_type: &efi::Guid, comm_buffer: *mut c_void, comm_buffer_size: *mut usize) -> efi::Status;

/// An MMI handler callback: an external (C ABI) or internal (Rust) function.
#[derive(Clone, Copy)]
enum HandlerKind {
    /// External handler registered by a driver through the MMST.
    External(MmiHandlerEntryPoint),
    /// Internal handler registered by the MM Core directly.
    Internal(InternalMmiHandler),
}

impl core::fmt::Debug for HandlerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Print only the variant; function pointers have no useful `Debug` output.
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
    /// Monotonic ID, also used as the dispatch handle.
    id: usize,
    /// Set when the handler is unregistered during a dispatch; removal is deferred.
    to_remove: bool,
}

/// The MMI handler database.
///
/// Holds the root handlers and the GUID handlers behind an internal `Mutex`.
pub struct MmiDatabase {
    inner: Mutex<MmiDatabaseInner>,
}

struct MmiDatabaseInner {
    /// Root MMI handlers, run for every MMI.
    root_handlers: Vec<MmiHandler>,
    /// GUID-specific MMI entries.
    entries: Vec<MmiEntry>,
    /// Nesting depth of `mmi_manage` calls.
    manage_calling_depth: usize,
    /// Next dispatch handle ID.
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

    /// Registers an external (C ABI) MMI handler.
    ///
    /// A `handler_type` of `None` registers a root handler; `Some(guid)` registers a handler
    /// for that GUID. Returns an opaque dispatch handle for unregistering the handler.
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
        log::debug!("Registered external MMI handler id={} for {:?}", id, handler_type.map(patina::Guid::from_ref));
        Ok(handle)
    }

    /// Registers an internal (Rust) MMI handler.
    ///
    /// Works like [`mmi_handler_register`](Self::mmi_handler_register) but takes a safe
    /// Rust function pointer instead of an `unsafe extern "efiapi" fn`.
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

    /// Inserts a handler into the root list or its GUID entry.
    fn insert_handler(inner: &mut MmiDatabaseInner, handler_type: Option<&efi::Guid>, handler: MmiHandler) {
        match handler_type {
            None => {
                inner.root_handlers.push(handler);
            }
            Some(guid) => {
                if let Some(entry) = inner.entries.iter_mut().find(|entry| entry.handler_type == *guid) {
                    entry.handlers.push(handler);
                } else {
                    inner.entries.push(MmiEntry { handler_type: *guid, handlers: vec![handler] });
                }
            }
        }
    }

    /// Unregisters an MMI handler by its dispatch handle.
    ///
    /// During a dispatch (`manage_calling_depth > 0`) the handler is only marked, and it is
    /// removed when the outermost dispatch finishes. Otherwise it is removed at once.
    ///
    /// ## Errors
    ///
    /// Returns `INVALID_PARAMETER` if no handler has this dispatch handle, as PI and EDK2 `Mmi.c`
    /// define for `MmiHandlerUnRegister`.
    pub fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        let target_id = dispatch_handle as usize;
        let mut inner = self.inner.lock();

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
        Err(efi::Status::INVALID_PARAMETER)
    }

    /// Dispatches an MMI, like the C `MmiManage`.
    ///
    /// A `handler_type` of `None` dispatches the root handlers; `Some(guid)` dispatches the
    /// handlers for that GUID. The database lock is released while handlers run, so they
    /// may register or unregister handlers.
    ///
    /// Returns:
    /// - `EFI_SUCCESS` if a handler succeeded or quiesced its source
    ///   (`EFI_WARN_INTERRUPT_SOURCE_QUIESCED` is reported as `EFI_SUCCESS`).
    /// - `EFI_INTERRUPT_PENDING` if a handler reported the interrupt still pending and no
    ///   handler succeeded.
    /// - `EFI_WARN_INTERRUPT_SOURCE_PENDING` if a source was processed but not quiesced and
    ///   no handler succeeded.
    /// - `EFI_NOT_FOUND` if no handler is registered for the type or none claimed the MMI.
    ///
    /// `context`, `comm_buffer` and `comm_buffer_size` are passed unchanged to the handlers,
    /// which dereference them. Callers outside this module go through the `unsafe`
    /// `MmServices::mmi_manage`, which carries that contract, so this stays inside the crate.
    pub(crate) fn mmi_manage(
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
                None => inner.root_handlers.iter().filter(|handler| !handler.to_remove).copied().collect::<Vec<_>>(),
                Some(guid) => {
                    if let Some(entry) = inner.entries.iter().find(|entry| entry.handler_type == *guid) {
                        entry.handlers.iter().filter(|handler| !handler.to_remove).copied().collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    }
                }
            }
            // lock released here
        };

        log::trace!(
            "Dispatching MMI with handler_type = {:?} ({} handler(s))",
            handler_type.map(patina::Guid::from_ref),
            handlers_snapshot.len()
        );
        // ----- Phase 2: dispatch without the lock held -----
        let return_status =
            self.dispatch_handler_snapshot(&handlers_snapshot, handler_type, context, comm_buffer, comm_buffer_size);

        // ----- Phase 3: update depth and clean up under the lock -----
        let mut inner = self.inner.lock();
        inner.manage_calling_depth -= 1;

        if inner.manage_calling_depth == 0 {
            Self::cleanup_removed_handlers(&mut inner);
        }

        return_status
    }

    /// Runs a snapshot of handlers. Called without the database lock held.
    ///
    /// A handler that runs earlier in the snapshot may unregister a later one. The snapshot copy
    /// does not see that mark, so the database is checked again before each call.
    fn dispatch_handler_snapshot(
        &self,
        handlers: &[MmiHandler],
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        if handlers.is_empty() {
            log::debug!("No MMI handler registered for {:?}", handler_type.map(patina::Guid::from_ref));
            return efi::Status::NOT_FOUND;
        }

        let short_circuit = handler_type.is_some();

        let mut return_status = efi::Status::NOT_FOUND;

        // Root dispatch passes internal handlers a zero GUID.
        let null_guid = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
        let guid_ref = handler_type.unwrap_or(&null_guid);

        for handler in handlers {
            if self.is_marked_for_removal(handler.id) {
                log::trace!("Skipping handler id = {}, unregistered during this dispatch", handler.id);
                continue;
            }
            log::trace!("Dispatching handler with id = {}, kind = {:?}", handler.id, handler.kind);
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
                        log::trace!("Short-circuiting after successful handler dispatch (id = {}).", handler.id);
                        break;
                    }
                }
                _ if status == INTERRUPT_PENDING => {
                    if short_circuit {
                        log::trace!("Short-circuiting due to pending interrupt (id = {}).", handler.id);
                        return INTERRUPT_PENDING;
                    }
                    if return_status != efi::Status::SUCCESS {
                        return_status = status;
                    }
                }
                _ if status == WARN_INTERRUPT_SOURCE_QUIESCED => {
                    return_status = efi::Status::SUCCESS;
                }
                _ if status == WARN_INTERRUPT_SOURCE_PENDING && return_status != efi::Status::SUCCESS => {
                    return_status = status;
                }
                _ => {
                    // The PI spec ignores other statuses.
                }
            }
        }
        log::trace!("Finished dispatching handlers with final status = {:#x}", return_status.as_usize());
        return_status
    }

    /// Returns whether the handler with dispatch handle `id` was unregistered.
    ///
    /// Removal is deferred while a dispatch runs, so a marked handler is still in the database.
    /// One that is no longer there is treated as removed.
    fn is_marked_for_removal(&self, id: usize) -> bool {
        let inner = self.inner.lock();
        inner
            .root_handlers
            .iter()
            .chain(inner.entries.iter().flat_map(|entry| entry.handlers.iter()))
            .find(|handler| handler.id == id)
            .is_none_or(|handler| handler.to_remove)
    }

    /// Removes the handlers marked `to_remove` and drops empty GUID entries.
    fn cleanup_removed_handlers(inner: &mut MmiDatabaseInner) {
        inner.root_handlers.retain(|handler| !handler.to_remove);

        inner.entries.retain_mut(|entry| {
            entry.handlers.retain(|handler| !handler.to_remove);
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

    #[test]
    fn test_mmi_manage_reports_that_no_handler_is_registered() {
        crate::test_support::init_test_logger();
        let database = MmiDatabase::new();

        // Nothing is registered for this type, so the dispatch reports not found.
        let status =
            database.mmi_manage(Some(&GUID_X), core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut());
        assert_eq!(status, efi::Status::NOT_FOUND);

        // The root chain is empty too.
        let status = database.mmi_manage(None, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut());
        assert_eq!(status, efi::Status::NOT_FOUND);
    }

    /// The database the re-entrant handlers below reach back into. One per test process.
    static REENTRANT_DATABASE: MmiDatabase = MmiDatabase::new();
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
    const RAN_UNREGISTER_VICTIM: usize = 10;
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
        REENTRANT_DATABASE.mmi_handler_unregister(dispatch_handle).expect("the running handler is registered");
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
        REENTRANT_DATABASE.mmi_handler_unregister(victim).expect("the victim is registered");

        let nested = REENTRANT_DATABASE.mmi_manage(
            Some(&GUID_X),
            core::ptr::null(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        NESTED_STATUS.store(nested.as_usize(), Ordering::Relaxed);
        efi::Status::SUCCESS
    }

    /// Unregisters another handler without dispatching again.
    unsafe extern "efiapi" fn unregisters_victim(
        _dispatch_handle: efi::Handle,
        _context: *const c_void,
        _comm_buffer: *mut c_void,
        _comm_buffer_size: *mut usize,
    ) -> efi::Status {
        record(RAN_UNREGISTER_VICTIM);
        let victim = VICTIM.load(Ordering::Relaxed) as efi::Handle;
        REENTRANT_DATABASE.mmi_handler_unregister(victim).expect("the victim is registered");
        efi::Status::SUCCESS
    }

    fn internal_recording(handler_type: &efi::Guid, _: *mut c_void, _: *mut usize) -> efi::Status {
        record(RAN_INTERNAL);
        *INTERNAL_GUID.lock() = Some(*handler_type);
        efi::Status::SUCCESS
    }

    /// Dispatches `handler_type` with no context or buffer.
    fn manage(database: &MmiDatabase, handler_type: Option<&efi::Guid>) -> efi::Status {
        database.mmi_manage(handler_type, core::ptr::null(), core::ptr::null_mut(), core::ptr::null_mut())
    }

    fn register(database: &MmiDatabase, handler: MmiHandlerEntryPoint, guid: Option<&efi::Guid>) -> efi::Handle {
        database.mmi_handler_register(handler, guid).expect("registration succeeds")
    }

    #[test]
    fn test_pi_interrupt_status_codes_match_pi_multi_phase() {
        // `PI_ENCODE_WARNING (a)` is `(MAX_BIT >> 2) | a`, and `PI_ENCODE_ERROR (a)` also sets `MAX_BIT`.
        assert_eq!(WARN_INTERRUPT_SOURCE_PENDING.as_usize() as u64, 0x2000_0000_0000_0000);
        assert_eq!(WARN_INTERRUPT_SOURCE_QUIESCED.as_usize() as u64, 0x2000_0000_0000_0001);
        assert_eq!(INTERRUPT_PENDING.as_usize() as u64, 0xA000_0000_0000_0000);
        assert!(INTERRUPT_PENDING.is_error());
        assert!(!WARN_INTERRUPT_SOURCE_PENDING.is_error());
    }

    #[test]
    fn test_dispatching_an_empty_database_reports_not_found() {
        let database = MmiDatabase::default();

        assert_eq!(manage(&database, None), efi::Status::NOT_FOUND);
        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_a_guid_handler_only_runs_for_its_own_type() {
        let database = MmiDatabase::new();
        register(&database, succeeds_a, Some(&GUID_X));

        assert_eq!(manage(&database, Some(&GUID_Y)), efi::Status::NOT_FOUND);
        assert_eq!(manage(&database, None), efi::Status::NOT_FOUND, "a typed handler is not a root handler");
        assert!(calls().is_empty());

        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A]);
    }

    #[test]
    fn test_root_handlers_run_for_every_mmi_and_are_not_short_circuited() {
        let database = MmiDatabase::new();
        register(&database, succeeds_a, None);
        register(&database, succeeds_b, None);

        assert_eq!(manage(&database, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A, RAN_B], "every root handler runs even after one succeeds");
    }

    #[test]
    fn test_a_typed_dispatch_stops_at_the_first_successful_handler() {
        let database = MmiDatabase::new();
        register(&database, succeeds_a, Some(&GUID_X));
        register(&database, succeeds_b, Some(&GUID_X));

        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A], "dispatch short-circuits once a handler succeeds");
    }

    #[test]
    fn test_a_typed_dispatch_stops_immediately_on_a_pending_interrupt() {
        let database = MmiDatabase::new();
        register(&database, reports_pending, Some(&GUID_X));
        register(&database, succeeds_a, Some(&GUID_X));

        assert_eq!(manage(&database, Some(&GUID_X)), INTERRUPT_PENDING);
        assert_eq!(calls(), vec![RAN_PENDING], "a pending interrupt short-circuits the rest");
    }

    #[test]
    fn test_a_root_pending_interrupt_is_reported_but_does_not_stop_dispatch() {
        let database = MmiDatabase::new();
        register(&database, reports_pending, None);
        register(&database, reports_unsupported, None);

        assert_eq!(manage(&database, None), INTERRUPT_PENDING);
        assert_eq!(calls(), vec![RAN_PENDING, RAN_UNSUPPORTED]);
    }

    #[test]
    fn test_a_success_outranks_a_pending_interrupt_from_another_root_handler() {
        let database = MmiDatabase::new();
        register(&database, succeeds_a, None);
        register(&database, reports_pending, None);

        assert_eq!(manage(&database, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_A, RAN_PENDING]);
    }

    #[test]
    fn test_a_quiesced_source_is_reported_as_success() {
        let database = MmiDatabase::new();
        register(&database, reports_quiesced, None);

        assert_eq!(manage(&database, None), efi::Status::SUCCESS);
    }

    #[test]
    fn test_a_pending_source_is_reported_only_when_nothing_succeeded() {
        let database = MmiDatabase::new();
        register(&database, reports_source_pending, None);
        assert_eq!(manage(&database, None), WARN_INTERRUPT_SOURCE_PENDING);

        register(&database, succeeds_a, None);
        assert_eq!(manage(&database, None), efi::Status::SUCCESS, "a success outranks a pending source");
    }

    #[test]
    fn test_an_unhandled_status_leaves_the_mmi_unclaimed() {
        let database = MmiDatabase::new();
        register(&database, reports_unsupported, None);

        assert_eq!(manage(&database, None), efi::Status::NOT_FOUND, "statuses outside the PI set are ignored");
        assert_eq!(calls(), vec![RAN_UNSUPPORTED]);
    }

    #[test]
    fn test_the_communication_buffer_and_size_reach_the_handler() {
        let database = MmiDatabase::new();
        register(&database, echoes_comm_buffer, Some(&GUID_X));

        let mut buffer = [0u8; 4];
        let mut size = 4usize;
        assert_eq!(
            database.mmi_manage(Some(&GUID_X), core::ptr::null(), buffer.as_mut_ptr().cast(), &raw mut size),
            efi::Status::SUCCESS
        );
        assert_eq!(size, 8);
        assert_eq!(buffer[0], 0xCD);
    }

    #[test]
    fn test_handlers_sharing_a_guid_join_the_same_entry() {
        let database = MmiDatabase::new();
        register(&database, reports_unsupported, Some(&GUID_X));
        register(&database, succeeds_a, Some(&GUID_X));
        register(&database, succeeds_b, Some(&GUID_Y));

        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_UNSUPPORTED, RAN_A], "both handlers for the GUID ran, in registration order");
    }

    #[test]
    fn test_registration_hands_out_distinct_dispatch_handles() {
        let database = MmiDatabase::new();

        let first = register(&database, succeeds_a, None);
        let second = register(&database, succeeds_b, Some(&GUID_X));
        let third = database.register_internal_handler(internal_recording, None).expect("registration succeeds");

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);
    }

    #[test]
    fn test_unregistering_removes_a_root_handler_immediately() {
        let database = MmiDatabase::new();
        let handle = register(&database, succeeds_a, None);
        register(&database, succeeds_b, None);

        assert_eq!(database.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&database, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_B]);
    }

    #[test]
    fn test_unregistering_the_last_handler_for_a_guid_retires_the_entry() {
        let database = MmiDatabase::new();
        let handle = register(&database, succeeds_a, Some(&GUID_X));

        assert_eq!(database.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::NOT_FOUND);
        assert!(calls().is_empty());
    }

    #[test]
    fn test_unregistering_an_unknown_handle_is_reported_not_found() {
        let database = MmiDatabase::new();
        register(&database, succeeds_a, None);
        register(&database, succeeds_b, Some(&GUID_X));

        assert_eq!(
            database.mmi_handler_unregister(core::ptr::without_provenance_mut(0x999)),
            Err(efi::Status::INVALID_PARAMETER)
        );
    }

    #[test]
    fn test_a_handler_may_unregister_itself_while_it_is_running() {
        register(&REENTRANT_DATABASE, unregisters_itself, None);
        register(&REENTRANT_DATABASE, succeeds_a, None);

        assert_eq!(manage(&REENTRANT_DATABASE, None), efi::Status::SUCCESS);
        assert_eq!(
            calls(),
            vec![RAN_SELF_UNREGISTER, RAN_A],
            "removal is deferred, so the rest of the snapshot still runs"
        );

        assert_eq!(manage(&REENTRANT_DATABASE, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_SELF_UNREGISTER, RAN_A, RAN_A], "the handler was removed once dispatch finished");
    }

    #[test]
    fn test_a_handler_marked_for_removal_is_not_dispatched_by_a_nested_mmi() {
        let victim = register(&REENTRANT_DATABASE, succeeds_b, Some(&GUID_X));
        VICTIM.store(victim as usize, Ordering::Relaxed);
        register(&REENTRANT_DATABASE, unregisters_victim_then_redispatches, None);

        assert_eq!(manage(&REENTRANT_DATABASE, None), efi::Status::SUCCESS);
        assert_eq!(
            NESTED_STATUS.load(Ordering::Relaxed),
            efi::Status::NOT_FOUND.as_usize(),
            "the nested dispatch skipped the handler that was marked for removal"
        );
        assert_eq!(calls(), vec![RAN_REDISPATCH], "the victim never ran");

        assert_eq!(
            manage(&REENTRANT_DATABASE, Some(&GUID_X)),
            efi::Status::NOT_FOUND,
            "the victim was cleaned up afterwards"
        );
    }

    #[test]
    fn test_a_handler_unregistered_by_an_earlier_one_is_skipped_in_the_same_dispatch() {
        // Root handlers do not short-circuit, so without the check the victim would run from the
        // snapshot taken before it was unregistered.
        register(&REENTRANT_DATABASE, unregisters_victim, None);
        let victim = register(&REENTRANT_DATABASE, succeeds_b, None);
        VICTIM.store(victim as usize, Ordering::Relaxed);

        assert_eq!(manage(&REENTRANT_DATABASE, None), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_UNREGISTER_VICTIM], "the unregistered handler did not run");
    }

    #[test]
    fn test_an_internal_handler_receives_the_guid_it_was_registered_for() {
        let database = MmiDatabase::new();
        database.register_internal_handler(internal_recording, Some(&GUID_X)).expect("registration succeeds");

        assert_eq!(manage(&database, Some(&GUID_X)), efi::Status::SUCCESS);
        assert_eq!(calls(), vec![RAN_INTERNAL]);
        assert_eq!(*INTERNAL_GUID.lock(), Some(GUID_X));
    }

    #[test]
    fn test_a_root_internal_handler_is_dispatched_with_a_null_guid() {
        let database = MmiDatabase::new();
        let handle = database.register_internal_handler(internal_recording, None).expect("registration succeeds");

        assert_eq!(manage(&database, None), efi::Status::SUCCESS);
        assert_eq!(*INTERNAL_GUID.lock(), Some(efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6])));

        assert_eq!(database.mmi_handler_unregister(handle), Ok(()));
        assert_eq!(manage(&database, None), efi::Status::NOT_FOUND);
    }

    #[test]
    fn test_handler_kinds_are_distinguishable_when_logged() {
        assert_eq!(alloc::format!("{:?}", HandlerKind::External(succeeds_a)), "External");
        assert_eq!(alloc::format!("{:?}", HandlerKind::Internal(internal_recording)), "Internal");
    }
}
