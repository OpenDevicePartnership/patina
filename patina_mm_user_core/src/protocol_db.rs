//! Protocol / Handle Database
//!
//! Idiomatic Rust implementation of the MM handle-and-protocol database that
//! backs the `EfiMmSystemTable` protocol services (`MmInstallProtocolInterface`,
//! `MmLocateProtocol`, `MmHandleProtocol`, …).
//!
//! The database is owned by the [`MmUserCore`](crate::MmUserCore) instance. The
//! `extern "efiapi"` thunks in [`crate::mm_services`] simply locate that
//! instance and call the safe Rust methods below - the table is a thin shim,
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
//!   `efi::Handle` (`*mut c_void`). The pointer is never dereferenced - it is a
//!   token - so the round trip is sound.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{collections::BTreeMap, vec::Vec};
use core::{ffi::c_void, num::NonZeroUsize};

use patina::standard::efi;
use spin::Mutex;

/// The SDK [`ProtocolNotify`] callback type, shared with the `MmServices` trait
/// so the user core and the trait agree on a single notify representation.
use patina::management_mode::mm_services::ProtocolNotify;

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
        ProtocolNotify::Native(callback) => core::ptr::from_ref(*callback).cast::<()>() as usize,
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
// held. No interior reference escapes - only opaque id-tokens are handed across
// the MM ABI - so sharing a `ProtocolDatabase` across threads cannot race.
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

            let id = if let Some(id) = Self::handle_to_id(handle) {
                // Existing handle: it must exist and must not already carry this protocol.
                let Some(protocols) = inner.handles.get(&id) else {
                    log::error!("MmInstallProtocolInterface: handle {handle:p} is not a known handle");
                    return Err(efi::Status::INVALID_PARAMETER);
                };
                if protocols.iter().any(|p| p.guid == *guid) {
                    log::error!("MmInstallProtocolInterface: {guid:?} is already installed on handle {handle:p}");
                    return Err(efi::Status::INVALID_PARAMETER);
                }
                id
            } else {
                // Null handle: allocate a new one.
                let id = inner.next_id();
                inner.handles.insert(id, Vec::new());
                id
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

        log::debug!("MmInstallProtocolInterface: {guid:?} on handle {installed_handle:p}");
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
            let pos = protocols
                .iter()
                .position(|p| p.guid == *guid && p.interface == interface)
                .ok_or_else(|| {
                    log::error!(
                        "MmUninstallProtocolInterface: {guid:?} with interface {interface:p} is not installed on handle {handle:p}"
                    );
                    efi::Status::NOT_FOUND
                })?;
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
        let token = NonZeroUsize::new(registration as usize).ok_or_else(|| {
            log::error!("MmRegisterProtocolNotify: registration token for {guid:?} is null");
            efi::Status::INVALID_PARAMETER
        })?;
        let mut inner = self.inner.lock();
        let pos = inner.notifications.iter().position(|n| n.guid == *guid && n.token == token).ok_or_else(|| {
            log::error!("MmRegisterProtocolNotify: no notification for {guid:?} with token {token}");
            efi::Status::NOT_FOUND
        })?;
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::management_mode::mm_services::ProtocolInstalled;

    static PROTOCOL_A: efi::Guid = efi::Guid::from_fields(0xA000_0001, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
    static PROTOCOL_B: efi::Guid = efi::Guid::from_fields(0xB000_0002, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 2]);
    static PROTOCOL_C: efi::Guid = efi::Guid::from_fields(0xC000_0003, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 3]);

    /// The database the re-entrant callback below reaches back into.
    static DB: ProtocolDatabase = ProtocolDatabase::new();
    /// Notifications observed, as `(guid, interface, handle)`.
    static EVENTS: Mutex<Vec<(efi::Guid, usize, usize)>> = Mutex::new(Vec::new());

    /// Interface pointers only need to be distinct; they are never dereferenced.
    fn interface(tag: usize) -> *mut c_void {
        core::ptr::without_provenance_mut(tag)
    }

    fn events() -> Vec<(efi::Guid, usize, usize)> {
        EVENTS.lock().clone()
    }

    unsafe extern "efiapi" fn efi_notify(
        protocol: *const efi::Guid,
        interface: *mut c_void,
        handle: efi::Handle,
    ) -> efi::Status {
        // SAFETY: the database passes a reference to a GUID that outlives the call.
        let guid = unsafe { *protocol };
        EVENTS.lock().push((guid, interface as usize, handle as usize));
        efi::Status::SUCCESS
    }

    unsafe extern "efiapi" fn other_efi_notify(
        _protocol: *const efi::Guid,
        _interface: *mut c_void,
        _handle: efi::Handle,
    ) -> efi::Status {
        efi::Status::SUCCESS
    }

    fn record_native(event: ProtocolInstalled<'_>) -> efi::Status {
        EVENTS.lock().push((*event.protocol, 0, event.handle as usize));
        efi::Status::SUCCESS
    }

    /// Installs a second protocol from inside a notification, which only works
    /// because the database releases its lock before invoking callbacks.
    fn install_from_notify(event: ProtocolInstalled<'_>) -> efi::Status {
        EVENTS.lock().push((*event.protocol, 0, event.handle as usize));
        DB.install_protocol(event.handle, &PROTOCOL_C, interface(0x33)).expect("the database is re-entrant");
        efi::Status::SUCCESS
    }

    static NATIVE_NOTIFY: &(dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync) =
        &(record_native as fn(ProtocolInstalled<'_>) -> efi::Status);
    static REENTRANT_NOTIFY: &(dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync) =
        &(install_from_notify as fn(ProtocolInstalled<'_>) -> efi::Status);

    fn install_new(db: &ProtocolDatabase, guid: &efi::Guid, interface: *mut c_void) -> efi::Handle {
        db.install_protocol(core::ptr::null_mut(), guid, interface).expect("installing on a null handle succeeds")
    }

    #[test]
    fn test_a_new_database_reports_nothing_installed() {
        let db = ProtocolDatabase::default();

        assert!(db.all_handles().is_empty());
        assert!(db.registered_protocols().is_empty());
        assert!(!db.is_protocol_installed(&PROTOCOL_A));
        assert_eq!(db.locate_protocol(&PROTOCOL_A), None);
        assert!(db.locate_handle_by_protocol(&PROTOCOL_A).is_empty());
    }

    #[test]
    fn test_installing_on_a_null_handle_allocates_a_fresh_handle() {
        let db = ProtocolDatabase::new();

        let first = install_new(&db, &PROTOCOL_A, interface(0x11));
        let second = install_new(&db, &PROTOCOL_A, interface(0x22));

        assert!(!first.is_null());
        assert_ne!(first, second, "each null handle install gets its own handle");
        assert_eq!(db.all_handles(), alloc::vec![first, second], "handles are listed in creation order");
    }

    #[test]
    fn test_a_second_protocol_joins_an_existing_handle() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(db.install_protocol(handle, &PROTOCOL_B, interface(0x22)), Ok(handle));
        assert_eq!(db.all_handles(), alloc::vec![handle], "no new handle was created");
        assert_eq!(db.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(db.handle_protocol(handle, &PROTOCOL_B), Some(interface(0x22)));
    }

    #[test]
    fn test_the_same_protocol_cannot_be_installed_twice_on_one_handle() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(db.install_protocol(handle, &PROTOCOL_A, interface(0x22)), Err(efi::Status::INVALID_PARAMETER));
        assert_eq!(db.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)), "the original survives");
    }

    #[test]
    fn test_installing_on_an_unknown_handle_is_rejected() {
        let db = ProtocolDatabase::new();

        assert_eq!(
            db.install_protocol(interface(0x999), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert!(db.all_handles().is_empty());
    }

    #[test]
    fn test_handle_protocol_answers_only_for_the_exact_pairing() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));
        let other = install_new(&db, &PROTOCOL_B, interface(0x22));

        assert_eq!(db.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(db.handle_protocol(handle, &PROTOCOL_B), None, "the protocol lives on another handle");
        assert_eq!(db.handle_protocol(other, &PROTOCOL_A), None);
        assert_eq!(db.handle_protocol(core::ptr::null_mut(), &PROTOCOL_A), None);
        assert_eq!(db.handle_protocol(interface(0x999), &PROTOCOL_A), None, "unknown handle");
    }

    #[test]
    fn test_locate_protocol_returns_the_first_interface_installed() {
        let db = ProtocolDatabase::new();
        install_new(&db, &PROTOCOL_A, interface(0x11));
        install_new(&db, &PROTOCOL_A, interface(0x22));

        assert_eq!(db.locate_protocol(&PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(db.locate_protocol(&PROTOCOL_B), None);
    }

    #[test]
    fn test_locate_handle_by_protocol_returns_only_the_matching_handles() {
        let db = ProtocolDatabase::new();
        let first = install_new(&db, &PROTOCOL_A, interface(0x11));
        install_new(&db, &PROTOCOL_B, interface(0x22));
        let third = install_new(&db, &PROTOCOL_A, interface(0x33));

        assert_eq!(db.locate_handle_by_protocol(&PROTOCOL_A), alloc::vec![first, third]);
        assert!(db.locate_handle_by_protocol(&PROTOCOL_C).is_empty());
        assert!(db.is_protocol_installed(&PROTOCOL_A));
        assert!(!db.is_protocol_installed(&PROTOCOL_C));
    }

    #[test]
    fn test_uninstalling_the_last_protocol_retires_the_handle() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));
        db.install_protocol(handle, &PROTOCOL_B, interface(0x22)).expect("second protocol installs");

        assert_eq!(db.uninstall_protocol(handle, &PROTOCOL_A, interface(0x11)), Ok(()));
        assert_eq!(db.all_handles(), alloc::vec![handle], "the handle still carries a protocol");

        assert_eq!(db.uninstall_protocol(handle, &PROTOCOL_B, interface(0x22)), Ok(()));
        assert!(db.all_handles().is_empty(), "the empty handle was retired");
    }

    #[test]
    fn test_uninstall_requires_the_interface_that_was_installed() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(db.uninstall_protocol(handle, &PROTOCOL_A, interface(0x22)), Err(efi::Status::NOT_FOUND));
        assert_eq!(db.uninstall_protocol(handle, &PROTOCOL_B, interface(0x11)), Err(efi::Status::NOT_FOUND));
        assert_eq!(db.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)), "nothing was removed");
    }

    #[test]
    fn test_uninstall_rejects_a_null_or_unknown_handle() {
        let db = ProtocolDatabase::new();
        install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(
            db.uninstall_protocol(core::ptr::null_mut(), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert_eq!(
            db.uninstall_protocol(interface(0x999), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
    }

    #[test]
    fn test_registered_protocols_lists_each_guid_once() {
        let db = ProtocolDatabase::new();
        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));
        db.install_protocol(handle, &PROTOCOL_B, interface(0x22)).expect("second protocol installs");
        install_new(&db, &PROTOCOL_A, interface(0x33));

        assert_eq!(db.registered_protocols(), alloc::vec![PROTOCOL_A, PROTOCOL_B]);
    }

    #[test]
    fn test_a_notification_fires_with_the_interface_and_handle_that_were_installed() {
        let db = ProtocolDatabase::new();
        db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));

        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0x11, handle as usize)]);
    }

    #[test]
    fn test_a_notification_stays_silent_for_other_protocols() {
        let db = ProtocolDatabase::new();
        db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));

        install_new(&db, &PROTOCOL_B, interface(0x22));

        assert!(events().is_empty());
    }

    #[test]
    fn test_a_native_callback_is_told_the_handle_but_not_the_interface() {
        let db = ProtocolDatabase::new();
        db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY));

        let handle = install_new(&db, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0, handle as usize)]);
    }

    #[test]
    fn test_registering_the_same_callback_twice_reuses_its_token() {
        let db = ProtocolDatabase::new();

        let first = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));
        let again = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));
        assert_eq!(first, again, "the duplicate registration was folded into the first");

        let native = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY));
        let native_again = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY));
        assert_eq!(native, native_again);
        assert_ne!(first, native, "a native callback is a distinct registration");

        install_new(&db, &PROTOCOL_A, interface(0x11));
        assert_eq!(events().len(), 2, "each callback ran once, not twice");
    }

    #[test]
    fn test_a_different_guid_or_callback_gets_its_own_token() {
        let db = ProtocolDatabase::new();

        let a_efi = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));
        let b_efi = db.register_protocol_notify(&PROTOCOL_B, ProtocolNotify::efi(efi_notify));
        let a_other = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(other_efi_notify));

        assert_ne!(a_efi, b_efi, "the same callback on another GUID is a separate registration");
        assert_ne!(a_efi, a_other, "another callback on the same GUID is a separate registration");
    }

    #[test]
    fn test_unregistering_stops_the_notification() {
        let db = ProtocolDatabase::new();
        let token = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));

        assert_eq!(db.unregister_protocol_notify(&PROTOCOL_A, token), Ok(()));

        install_new(&db, &PROTOCOL_A, interface(0x11));
        assert!(events().is_empty());
    }

    #[test]
    fn test_unregister_rejects_a_null_unknown_or_mismatched_token() {
        let db = ProtocolDatabase::new();
        let token = db.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify));

        assert_eq!(
            db.unregister_protocol_notify(&PROTOCOL_A, core::ptr::null_mut()),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert_eq!(db.unregister_protocol_notify(&PROTOCOL_A, interface(0x999)), Err(efi::Status::NOT_FOUND));
        assert_eq!(
            db.unregister_protocol_notify(&PROTOCOL_B, token),
            Err(efi::Status::NOT_FOUND),
            "the token belongs to another GUID"
        );

        install_new(&db, &PROTOCOL_A, interface(0x11));
        assert_eq!(events().len(), 1, "the registration survived the failed unregisters");
    }

    #[test]
    fn test_a_notification_may_re_enter_the_database() {
        DB.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(REENTRANT_NOTIFY));

        let handle = install_new(&DB, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0, handle as usize)]);
        assert_eq!(
            DB.handle_protocol(handle, &PROTOCOL_C),
            Some(interface(0x33)),
            "the callback installed a protocol while the install that triggered it was still unwinding"
        );
    }
}
