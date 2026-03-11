//! Protocol / Handle Database
//!
//! Idiomatic Rust implementation of the MM handle-and-protocol database that
//! backs the `EfiMmSystemTable` protocol services (`MmInstallProtocolInterface`,
//! `MmLocateProtocol`, `MmHandleProtocol`, …).
//!
//! The database is owned by the [`MmUserCore`](crate::MmUserCore) instance. The
//! `extern "efiapi"` thunks in [`crate::mm_services`] simply locate that
//! instance and call the safe Rust methods below — the table is a thin shim,
//! this module is the implementation.
//!
//! ## Model
//!
//! * Each *handle* is an opaque, non-zero id under which a list of installed
//!   protocol interfaces lives.
//! * Handles are stored in a [`BTreeMap`] keyed by id, so iteration order
//!   matches creation order (the order EFI consumers expect from
//!   `MmLocateHandle`) without any manual bookkeeping.
//! * Across the MM ABI a handle is just its id reinterpreted as an
//!   `efi::Handle` (`*mut c_void`). The pointer is never dereferenced — it is a
//!   token — so the round trip is sound.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{collections::BTreeMap, vec::Vec};
use core::{ffi::c_void, num::NonZeroUsize};

use r_efi::efi;
use spin::Mutex;

/// The SDK [`ProtocolNotify`] callback type, shared with the `MmServices` trait
/// so the user core and the trait agree on a single notify representation.
use patina::mm_services::ProtocolNotify;

/// A single protocol interface installed on a handle.
struct ProtocolInterface {
    /// Protocol GUID.
    guid: efi::Guid,
    /// Opaque interface pointer supplied by the installer.
    interface: *mut c_void,
}

/// A registered protocol-install notification.
struct NotifyRegistration {
    /// GUID whose installation triggers the callback.
    guid: efi::Guid,
    /// The callback to invoke.
    notify: ProtocolNotify,
    /// Unique token returned to the registrant (and used to unregister).
    token: NonZeroUsize,
}

/// A notification captured while the lock is held, fired once it is released.
struct PendingNotify {
    notify: ProtocolNotify,
    guid: efi::Guid,
    interface: *mut c_void,
    handle: efi::Handle,
}

/// A stable address used to deduplicate identical `(GUID, callback)` registrations.
fn notify_identity(notify: &ProtocolNotify) -> usize {
    match notify {
        ProtocolNotify::Efi(callback) => *callback as usize,
        ProtocolNotify::Native(callback) => core::ptr::from_ref(*callback) as *const () as usize,
    }
}

/// Internal, lock-protected state of the [`ProtocolDatabase`].
struct Inner {
    /// Installed protocols grouped by handle id (id order == creation order).
    handles: BTreeMap<NonZeroUsize, Vec<ProtocolInterface>>,
    /// Registered protocol-install notifications.
    notifications: Vec<NotifyRegistration>,
    /// Monotonic source of unique ids for both handles and notify tokens.
    next_id: NonZeroUsize,
}

impl Inner {
    /// Returns a fresh, never-before-used id.
    fn next_id(&mut self) -> NonZeroUsize {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("MM protocol id space exhausted");
        id
    }
}

/// Handle-aware protocol database for the MM User Core.
pub struct ProtocolDatabase {
    inner: Mutex<Inner>,
}

// SAFETY: every interior value (including the raw interface and notify
// pointers) is owned by the database and only accessed while the `Mutex` is
// held. No interior reference escapes — only opaque id-tokens are handed across
// the MM ABI — so sharing a `ProtocolDatabase` across threads cannot race.
unsafe impl Send for ProtocolDatabase {}
unsafe impl Sync for ProtocolDatabase {}

impl Default for ProtocolDatabase {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolDatabase {
    /// Creates a new, empty protocol database.
    pub const fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                handles: BTreeMap::new(),
                notifications: Vec::new(),
                next_id: NonZeroUsize::MIN,
            }),
        }
    }

    /// Reinterprets a handle id as an `efi::Handle` token.
    fn id_to_handle(id: NonZeroUsize) -> efi::Handle {
        id.get() as efi::Handle
    }

    /// Reinterprets an `efi::Handle` token back into a handle id, or `None` if null.
    fn handle_to_id(handle: efi::Handle) -> Option<NonZeroUsize> {
        NonZeroUsize::new(handle as usize)
    }

    /// Install a protocol interface onto a handle.
    ///
    /// A null `handle` allocates a fresh handle. Any matching protocol-install
    /// notifications are fired after the internal lock is released (so the
    /// callbacks may freely re-enter the database). Returns the handle the
    /// interface was installed on.
    pub fn install_protocol(
        &self,
        handle: efi::Handle,
        guid: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        let (installed_handle, pending) = {
            let mut inner = self.inner.lock();

            let id = match Self::handle_to_id(handle) {
                // Existing handle: it must exist and must not already carry this protocol.
                Some(id) => match inner.handles.get(&id) {
                    None => return Err(efi::Status::INVALID_PARAMETER),
                    Some(protocols) if protocols.iter().any(|p| p.guid == *guid) => {
                        return Err(efi::Status::INVALID_PARAMETER);
                    }
                    Some(_) => id,
                },
                // Null handle: allocate a new one.
                None => {
                    let id = inner.next_id();
                    inner.handles.insert(id, Vec::new());
                    id
                }
            };

            inner.handles.entry(id).or_default().push(ProtocolInterface { guid: *guid, interface });

            let installed_handle = Self::id_to_handle(id);
            let pending: Vec<PendingNotify> = inner
                .notifications
                .iter()
                .filter(|n| n.guid == *guid)
                .map(|n| PendingNotify { notify: n.notify, guid: *guid, interface, handle: installed_handle })
                .collect();

            (installed_handle, pending)
        };

        for event in pending {
            // SAFETY: the callback was previously registered by a driver. The GUID reference is to a
            // local that outlives the call, and the interface/handle are passed through unchanged.
            unsafe {
                event.notify.invoke(&event.guid, event.interface, event.handle);
            }
        }

        log::debug!("MmInstallProtocolInterface: {:?} on handle {:p}", guid, installed_handle);
        Ok(installed_handle)
    }

    /// Uninstall a protocol interface from a handle.
    ///
    /// When the handle has no remaining protocols it is removed entirely
    /// (matching the C `MmUninstallProtocolInterface` behaviour).
    pub fn uninstall_protocol(
        &self,
        handle: efi::Handle,
        guid: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        let id = Self::handle_to_id(handle).ok_or(efi::Status::INVALID_PARAMETER)?;
        let mut inner = self.inner.lock();

        let now_empty = {
            let protocols = inner.handles.get_mut(&id).ok_or(efi::Status::INVALID_PARAMETER)?;
            let pos = protocols
                .iter()
                .position(|p| p.guid == *guid && p.interface == interface)
                .ok_or(efi::Status::NOT_FOUND)?;
            protocols.remove(pos);
            protocols.is_empty()
        };

        if now_empty {
            inner.handles.remove(&id);
        }
        Ok(())
    }

    /// Look up a specific protocol on a specific handle (`MmHandleProtocol`).
    pub fn handle_protocol(&self, handle: efi::Handle, guid: &efi::Guid) -> Option<*mut c_void> {
        let id = Self::handle_to_id(handle)?;
        let inner = self.inner.lock();
        inner.handles.get(&id)?.iter().find(|p| p.guid == *guid).map(|p| p.interface)
    }

    /// Locate the first installed interface for a GUID across all handles.
    pub fn locate_protocol(&self, guid: &efi::Guid) -> Option<*mut c_void> {
        let inner = self.inner.lock();
        inner.handles.values().flatten().find(|p| p.guid == *guid).map(|p| p.interface)
    }

    /// Return every handle that carries a given protocol.
    pub fn locate_handle_by_protocol(&self, guid: &efi::Guid) -> Vec<efi::Handle> {
        let inner = self.inner.lock();
        inner
            .handles
            .iter()
            .filter(|(_, protocols)| protocols.iter().any(|p| p.guid == *guid))
            .map(|(id, _)| Self::id_to_handle(*id))
            .collect()
    }

    /// Return every handle in the database.
    pub fn all_handles(&self) -> Vec<efi::Handle> {
        let inner = self.inner.lock();
        inner.handles.keys().map(|id| Self::id_to_handle(*id)).collect()
    }

    /// Register a notification callback for a protocol GUID.
    ///
    /// Registering the same `(GUID, function)` pair twice returns the existing
    /// token rather than creating a duplicate (matching the C implementation).
    pub fn register_protocol_notify(&self, guid: &efi::Guid, notify: ProtocolNotify) -> *mut c_void {
        let mut inner = self.inner.lock();
        let identity = notify_identity(&notify);

        if let Some(existing) =
            inner.notifications.iter().find(|n| n.guid == *guid && notify_identity(&n.notify) == identity)
        {
            return existing.token.get() as *mut c_void;
        }

        let token = inner.next_id();
        inner.notifications.push(NotifyRegistration { guid: *guid, notify, token });
        token.get() as *mut c_void
    }

    /// Unregister a notification by its registration token.
    pub fn unregister_protocol_notify(&self, guid: &efi::Guid, registration: *mut c_void) -> Result<(), efi::Status> {
        let token = NonZeroUsize::new(registration as usize).ok_or(efi::Status::INVALID_PARAMETER)?;
        let mut inner = self.inner.lock();
        let pos = inner
            .notifications
            .iter()
            .position(|n| n.guid == *guid && n.token == token)
            .ok_or(efi::Status::NOT_FOUND)?;
        inner.notifications.remove(pos);
        Ok(())
    }

    /// Check whether a protocol GUID is installed on any handle.
    pub fn is_protocol_installed(&self, guid: &efi::Guid) -> bool {
        let inner = self.inner.lock();
        inner.handles.values().flatten().any(|p| p.guid == *guid)
    }

    /// Return all unique installed protocol GUIDs (used for depex evaluation).
    pub fn registered_protocols(&self) -> Vec<efi::Guid> {
        let inner = self.inner.lock();
        let mut guids = Vec::new();
        for protocol in inner.handles.values().flatten() {
            if !guids.contains(&protocol.guid) {
                guids.push(protocol.guid);
            }
        }
        guids
    }
}
