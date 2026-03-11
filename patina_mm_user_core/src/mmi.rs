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

use r_efi::efi;
use spin::Mutex;

/// MMI handler entry point signature (external / C ABI).
///
/// Re-exported from [`patina::pi::mm_cis::MmiHandlerEntryPoint`].
use patina::pi::mm_cis::MmiHandlerEntryPoint;

/// EFI_WARN_INTERRUPT_SOURCE_QUIESCED — PI spec warning status code.
/// Indicates an interrupt source was quiesced.
const WARN_INTERRUPT_SOURCE_QUIESCED: efi::Status = efi::Status::from_usize(3);

/// EFI_WARN_INTERRUPT_SOURCE_PENDING — PI spec warning status code.
/// Indicates an interrupt source was processed but not quiesced.
const WARN_INTERRUPT_SOURCE_PENDING: efi::Status = efi::Status::from_usize(2);

/// EFI_INTERRUPT_PENDING — PI spec status for pending interrupts.
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
        for handler in inner.root_handlers.iter_mut() {
            if handler.id == target_id {
                handler.to_remove = true;
                log::debug!("Marked root MMI handler id={} for removal.", target_id);
                if inner.manage_calling_depth == 0 {
                    Self::cleanup_removed_handlers(&mut inner);
                }
                return Ok(());
            }
        }

        // Search GUID-specific handlers
        for entry in inner.entries.iter_mut() {
            for handler in entry.handlers.iter_mut() {
                if handler.id == target_id {
                    handler.to_remove = true;
                    log::debug!(
                        "Marked MMI handler id={} for removal (GUID: {}).",
                        target_id,
                        patina::Guid::from_ref(&entry.handler_type)
                    );
                    if inner.manage_calling_depth == 0 {
                        Self::cleanup_removed_handlers(&mut inner);
                    }
                    return Ok(());
                }
            }
        }

        log::warn!("MMI handler {:?} not found for unregistering.", dispatch_handle);
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
                None => inner.root_handlers.iter().filter(|h| !h.to_remove).cloned().collect::<Vec<_>>(),
                Some(guid) => {
                    if let Some(entry) = inner.entries.iter().find(|e| e.handler_type == *guid) {
                        entry.handlers.iter().filter(|h| !h.to_remove).cloned().collect::<Vec<_>>()
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
        log::info!("Finished dispatching handlers with final status = {:?}", return_status);
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
