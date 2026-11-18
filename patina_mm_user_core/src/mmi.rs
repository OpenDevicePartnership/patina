//! MMI (Management Mode Interrupt) Handler Database
//!
//! Registers and dispatches MMI handlers. Root handlers (`handler_type` of `None`) run on
//! every MMI and serve hardware interrupt sources; GUID handlers run only for MMIs that
//! target their GUID.
//!
//! Handlers are either external (`MmiHandlerEntryPoint`, an `efiapi` function registered by a
//! driver through `MmiHandlerRegister`) or internal (`InternalMmiHandler`, a Rust function the
//! core registers for its own lifecycle events).
//!
//! The database lock is released while handlers run, so a handler may register or
//! unregister handlers.
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
/// `handler_type` is the GUID the handler was registered for (a zero GUID for root
/// handlers). The return value follows the `MmiManage` status rules.
pub(crate) type InternalMmiHandler =
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
pub(crate) struct MmiDatabase {
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
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(MmiDatabaseInner {
                root_handlers: Vec::new(),
                entries: Vec::new(),
                manage_calling_depth: 0,
                next_id: 1,
            }),
        }
    }

    /// Registers an external MMI handler and returns its dispatch handle.
    ///
    /// A `handler_type` of `None` registers a root handler.
    pub(crate) fn mmi_handler_register(
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

    /// Registers an internal (Rust) MMI handler, like
    /// [`mmi_handler_register`](Self::mmi_handler_register).
    pub(crate) fn register_internal_handler(
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
    /// Returns `INVALID_PARAMETER` if no handler has this dispatch handle, as the PI
    /// Specification defines for `MmiHandlerUnRegister`.
    pub(crate) fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        let target_id = dispatch_handle as usize;
        let mut inner = self.inner.lock();
        // A handler marked during a dispatch is already unregistered, so a second unregister
        // must not find it.
        let is_target = |handler: &MmiHandler| handler.id == target_id && !handler.to_remove;

        for handler in &mut inner.root_handlers {
            if is_target(handler) {
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
                if is_target(handler) {
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

    /// Dispatches an MMI to the root handlers (`handler_type` of `None`) or to the handlers for
    /// one GUID. A GUID dispatch stops at the first handler that returns `SUCCESS` or
    /// `INTERRUPT_PENDING`; a root dispatch runs every handler.
    ///
    /// Returns:
    /// - `EFI_SUCCESS` if a handler succeeded or returned `EFI_WARN_INTERRUPT_SOURCE_QUIESCED`.
    /// - `EFI_INTERRUPT_PENDING` or `EFI_WARN_INTERRUPT_SOURCE_PENDING` if a handler returned
    ///   it and none succeeded.
    /// - `EFI_NOT_FOUND` if no handler is registered for the type or none claimed the MMI.
    ///
    /// The pointer arguments are passed unchanged to the handlers, which dereference them.
    /// Callers outside this module go through the `unsafe` `MmServices::mmi_manage`.
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

    /// Runs a snapshot of handlers without the database lock held.
    ///
    /// An earlier handler may unregister a later one, so the database is checked again before
    /// each call.
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
                    // SAFETY: External handlers use the PI `efiapi` handler ABI. The dispatch
                    // handle is the handler's ID, and the pointers are the caller's.
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
                    // Other statuses do not change the result.
                }
            }
        }
        log::trace!("Finished dispatching handlers with final status = {:#x}", return_status.as_usize());
        return_status
    }

    /// Returns whether the handler with dispatch handle `id` was unregistered, either marked
    /// during a dispatch or already removed.
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
