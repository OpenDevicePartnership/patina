//! Protocol / Handle Database
//!
//! The MM handle and protocol database behind the `EfiMmSystemTable` protocol services
//! (`MmInstallProtocolInterface`, `MmLocateProtocol`, `MmHandleProtocol`, and so on).
//!
//! The database belongs to the [`MmUserCore`](crate::MmUserCore) instance. The
//! `extern "efiapi"` thunks in [`crate::mm_services`] reach these methods through the
//! core's `MmServices` implementation.
//!
//! ## Model
//!
//! * A handle is an opaque, non-zero id that owns a list of installed protocol interfaces.
//! * Handles are kept in a [`BTreeMap`] keyed by id, so iteration follows creation order,
//!   the order EFI consumers expect from `MmLocateHandle`.
//! * Across the MM ABI a handle is its id cast to an `efi::Handle` (`*mut c_void`). The
//!   pointer is never dereferenced, so the round trip is sound.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{collections::BTreeMap, vec::Vec};
use core::{ffi::c_void, num::NonZeroUsize, ptr::NonNull};

use patina::standard::efi;
use spin::Mutex;

// The SDK [`ProtocolNotify`] callback type, shared with the `MmServices` trait.
use patina::management_mode::mm_services::ProtocolNotify;

/// A protocol interface installed on a handle.
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
    /// Token returned to the registrant and used to unregister.
    token: NonZeroUsize,
}

/// A notification collected under the lock and fired after it is released.
struct PendingNotify {
    notify: ProtocolNotify,
    guid: efi::Guid,
    interface: *mut c_void,
    handle: efi::Handle,
}

/// The callback address, used to detect duplicate (GUID, callback) registrations.
fn notify_identity(notify: &ProtocolNotify) -> usize {
    match notify {
        ProtocolNotify::Efi(callback) => *callback as usize,
        ProtocolNotify::Native(callback) => core::ptr::from_ref(*callback).cast::<()>() as usize,
    }
}

/// Lock-protected state of the [`ProtocolDatabase`].
struct Inner {
    /// Installed protocols by handle id. Id order is creation order.
    handles: BTreeMap<NonZeroUsize, Vec<ProtocolInterface>>,
    /// Registered protocol-install notifications.
    notifications: Vec<NotifyRegistration>,
    /// Next unique id, shared by handles and notify tokens.
    next_id: NonZeroUsize,
}

impl Inner {
    /// Returns a new id that has never been used, or `OUT_OF_RESOURCES` once the id space is
    /// exhausted.
    fn next_id(&mut self) -> Result<NonZeroUsize, efi::Status> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            log::error!("MM protocol id space exhausted");
            efi::Status::OUT_OF_RESOURCES
        })?;
        Ok(id)
    }
}

/// Handle-aware protocol database for the MM User Core.
pub struct ProtocolDatabase {
    inner: Mutex<Inner>,
}

// SAFETY: Every value inside, including the raw interface and notify pointers, is owned by
// the database and only accessed with the `Mutex` held. Only opaque id tokens cross the MM
// ABI, so sharing a `ProtocolDatabase` across threads cannot race.
unsafe impl Send for ProtocolDatabase {}
// SAFETY: As above.
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

    /// Converts a handle id to an `efi::Handle` token.
    fn id_to_handle(id: NonZeroUsize) -> efi::Handle {
        id.get() as efi::Handle
    }

    /// Converts a notification token to the opaque, never null pointer the caller holds.
    fn token_to_ptr(token: NonZeroUsize) -> NonNull<c_void> {
        NonNull::without_provenance(token)
    }

    /// Converts an `efi::Handle` token back to a handle id, or `None` if it is null.
    fn handle_to_id(handle: efi::Handle) -> Option<NonZeroUsize> {
        NonZeroUsize::new(handle as usize)
    }

    /// Installs a protocol interface on a handle and returns that handle.
    ///
    /// A null `handle` allocates a new handle. Matching protocol-install notifications run
    /// after the lock is released, so callbacks may re-enter the database.
    ///
    /// ## Errors
    ///
    /// Returns `INVALID_PARAMETER` if `handle` is not a known handle or already carries
    /// `guid`, and `OUT_OF_RESOURCES` if no new handle id is left.
    pub fn install_protocol(
        &self,
        handle: efi::Handle,
        guid: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        let (installed_handle, pending) = {
            let mut inner = self.inner.lock();

            let id = if let Some(id) = Self::handle_to_id(handle) {
                // An existing handle must be known and must not already carry this protocol.
                let Some(protocols) = inner.handles.get(&id) else {
                    log::error!("MmInstallProtocolInterface: handle {handle:p} is not a known handle");
                    return Err(efi::Status::INVALID_PARAMETER);
                };
                if protocols.iter().any(|protocol| protocol.guid == *guid) {
                    log::error!("MmInstallProtocolInterface: {guid:?} is already installed on handle {handle:p}");
                    return Err(efi::Status::INVALID_PARAMETER);
                }
                id
            } else {
                inner.next_id()?
            };

            inner.handles.entry(id).or_default().push(ProtocolInterface { guid: *guid, interface });

            let installed_handle = Self::id_to_handle(id);
            let pending: Vec<PendingNotify> = inner
                .notifications
                .iter()
                .filter(|notification| notification.guid == *guid)
                .map(|notification| PendingNotify {
                    notify: notification.notify,
                    guid: *guid,
                    interface,
                    handle: installed_handle,
                })
                .collect();

            (installed_handle, pending)
        };

        for event in pending {
            // SAFETY: The callback was registered by a driver. The GUID reference points to a
            // local that outlives the call, and the interface and handle are passed unchanged.
            unsafe {
                event.notify.invoke(&event.guid, event.interface, event.handle);
            }
        }

        log::debug!("MmInstallProtocolInterface: {guid:?} on handle {installed_handle:p}");
        Ok(installed_handle)
    }

    /// Uninstalls a protocol interface from a handle.
    ///
    /// A handle left with no protocols is removed, as in the C
    /// `MmUninstallProtocolInterface`.
    ///
    /// ## Errors
    ///
    /// Returns `INVALID_PARAMETER` if `handle` is null or unknown, and `NOT_FOUND` if the
    /// handle does not carry `guid` with this `interface`.
    pub fn uninstall_protocol(
        &self,
        handle: efi::Handle,
        guid: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        let id = Self::handle_to_id(handle).ok_or_else(|| {
            log::error!("MmUninstallProtocolInterface: handle {handle:p} is null");
            efi::Status::INVALID_PARAMETER
        })?;
        let mut inner = self.inner.lock();

        let now_empty = {
            let protocols = inner.handles.get_mut(&id).ok_or_else(|| {
                log::error!("MmUninstallProtocolInterface: handle {handle:p} is not a known handle");
                efi::Status::INVALID_PARAMETER
            })?;
            let index = protocols
                .iter()
                .position(|protocol| protocol.guid == *guid && protocol.interface == interface)
                .ok_or_else(|| {
                    log::error!(
                        "MmUninstallProtocolInterface: {guid:?} with interface {interface:p} is not installed on handle {handle:p}"
                    );
                    efi::Status::NOT_FOUND
                })?;
            protocols.remove(index);
            protocols.is_empty()
        };

        if now_empty {
            inner.handles.remove(&id);
        }
        Ok(())
    }

    /// Returns the interface for `guid` on `handle` (`MmHandleProtocol`).
    pub fn handle_protocol(&self, handle: efi::Handle, guid: &efi::Guid) -> Option<*mut c_void> {
        let id = Self::handle_to_id(handle)?;
        let inner = self.inner.lock();
        inner.handles.get(&id)?.iter().find(|protocol| protocol.guid == *guid).map(|protocol| protocol.interface)
    }

    /// Returns the first installed interface for `guid` across all handles.
    pub fn locate_protocol(&self, guid: &efi::Guid) -> Option<*mut c_void> {
        let inner = self.inner.lock();
        inner.handles.values().flatten().find(|protocol| protocol.guid == *guid).map(|protocol| protocol.interface)
    }

    /// Returns every handle that carries `guid`.
    pub fn locate_handle_by_protocol(&self, guid: &efi::Guid) -> Vec<efi::Handle> {
        let inner = self.inner.lock();
        inner
            .handles
            .iter()
            .filter(|(_, protocols)| protocols.iter().any(|protocol| protocol.guid == *guid))
            .map(|(id, _)| Self::id_to_handle(*id))
            .collect()
    }

    /// Returns every handle in the database.
    pub fn all_handles(&self) -> Vec<efi::Handle> {
        let inner = self.inner.lock();
        inner.handles.keys().map(|id| Self::id_to_handle(*id)).collect()
    }

    /// Registers a notification callback for `guid` and returns its token.
    ///
    /// Registering the same (GUID, callback) pair again returns the existing token, as the
    /// C implementation does.
    ///
    /// ## Errors
    ///
    /// Returns `OUT_OF_RESOURCES` if no new token is left.
    pub fn register_protocol_notify(
        &self,
        guid: &efi::Guid,
        notify: ProtocolNotify,
    ) -> Result<NonNull<c_void>, efi::Status> {
        let mut inner = self.inner.lock();
        let identity = notify_identity(&notify);

        if let Some(existing) = inner
            .notifications
            .iter()
            .find(|notification| notification.guid == *guid && notify_identity(&notification.notify) == identity)
        {
            return Ok(Self::token_to_ptr(existing.token));
        }

        let token = inner.next_id()?;
        inner.notifications.push(NotifyRegistration { guid: *guid, notify, token });
        Ok(Self::token_to_ptr(token))
    }

    /// Unregisters a notification by its registration token.
    ///
    /// ## Errors
    ///
    /// Returns `INVALID_PARAMETER` for a null token and `NOT_FOUND` if no notification for
    /// `guid` has this token.
    pub fn unregister_protocol_notify(&self, guid: &efi::Guid, registration: *mut c_void) -> Result<(), efi::Status> {
        let token = NonZeroUsize::new(registration as usize).ok_or_else(|| {
            log::error!("MmRegisterProtocolNotify: registration token for {guid:?} is null");
            efi::Status::INVALID_PARAMETER
        })?;
        let mut inner = self.inner.lock();
        let index = inner
            .notifications
            .iter()
            .position(|notification| notification.guid == *guid && notification.token == token)
            .ok_or_else(|| {
                log::error!("MmRegisterProtocolNotify: no notification for {guid:?} with token {token}");
                efi::Status::NOT_FOUND
            })?;
        inner.notifications.remove(index);
        Ok(())
    }

    /// Returns whether `guid` is installed on any handle.
    pub fn is_protocol_installed(&self, guid: &efi::Guid) -> bool {
        let inner = self.inner.lock();
        inner.handles.values().flatten().any(|protocol| protocol.guid == *guid)
    }

    /// Returns each installed protocol GUID once, for depex evaluation.
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
