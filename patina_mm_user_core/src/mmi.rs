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
