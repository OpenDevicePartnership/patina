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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use patina::management_mode::mm_services::ProtocolInstalled;

    static PROTOCOL_A: efi::Guid = efi::Guid::from_fields(0xA000_0001, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 1]);
    static PROTOCOL_B: efi::Guid = efi::Guid::from_fields(0xB000_0002, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 2]);
    static PROTOCOL_C: efi::Guid = efi::Guid::from_fields(0xC000_0003, 0, 0, 0, 0, &[0, 0, 0, 0, 0, 3]);

    /// The database the re-entrant callback below reaches back into.
    static REENTRANT_DATABASE: ProtocolDatabase = ProtocolDatabase::new();
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
        REENTRANT_DATABASE
            .install_protocol(event.handle, &PROTOCOL_C, interface(0x33))
            .expect("the database is re-entrant");
        efi::Status::SUCCESS
    }

    static NATIVE_NOTIFY: &(dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync) =
        &(record_native as fn(ProtocolInstalled<'_>) -> efi::Status);
    static REENTRANT_NOTIFY: &(dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync) =
        &(install_from_notify as fn(ProtocolInstalled<'_>) -> efi::Status);

    fn install_new(database: &ProtocolDatabase, guid: &efi::Guid, interface: *mut c_void) -> efi::Handle {
        database.install_protocol(core::ptr::null_mut(), guid, interface).expect("installing on a null handle succeeds")
    }

    #[test]
    fn test_a_new_database_reports_nothing_installed() {
        let database = ProtocolDatabase::default();

        assert!(database.all_handles().is_empty());
        assert!(database.registered_protocols().is_empty());
        assert!(!database.is_protocol_installed(&PROTOCOL_A));
        assert_eq!(database.locate_protocol(&PROTOCOL_A), None);
        assert!(database.locate_handle_by_protocol(&PROTOCOL_A).is_empty());
    }

    #[test]
    fn test_installing_on_a_null_handle_allocates_a_fresh_handle() {
        let database = ProtocolDatabase::new();

        let first = install_new(&database, &PROTOCOL_A, interface(0x11));
        let second = install_new(&database, &PROTOCOL_A, interface(0x22));

        assert!(!first.is_null());
        assert_ne!(first, second, "each null handle install gets its own handle");
        assert_eq!(database.all_handles(), alloc::vec![first, second], "handles are listed in creation order");
    }

    #[test]
    fn test_a_second_protocol_joins_an_existing_handle() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(database.install_protocol(handle, &PROTOCOL_B, interface(0x22)), Ok(handle));
        assert_eq!(database.all_handles(), alloc::vec![handle], "no new handle was created");
        assert_eq!(database.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(database.handle_protocol(handle, &PROTOCOL_B), Some(interface(0x22)));
    }

    #[test]
    fn test_the_same_protocol_cannot_be_installed_twice_on_one_handle() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(
            database.install_protocol(handle, &PROTOCOL_A, interface(0x22)),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert_eq!(database.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)), "the original survives");
    }

    #[test]
    fn test_installing_on_an_unknown_handle_is_rejected() {
        let database = ProtocolDatabase::new();

        assert_eq!(
            database.install_protocol(interface(0x999), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert!(database.all_handles().is_empty());
    }

    #[test]
    fn test_handle_protocol_answers_only_for_the_exact_pairing() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));
        let other = install_new(&database, &PROTOCOL_B, interface(0x22));

        assert_eq!(database.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(database.handle_protocol(handle, &PROTOCOL_B), None, "the protocol lives on another handle");
        assert_eq!(database.handle_protocol(other, &PROTOCOL_A), None);
        assert_eq!(database.handle_protocol(core::ptr::null_mut(), &PROTOCOL_A), None);
        assert_eq!(database.handle_protocol(interface(0x999), &PROTOCOL_A), None, "unknown handle");
    }

    #[test]
    fn test_locate_protocol_returns_the_first_interface_installed() {
        let database = ProtocolDatabase::new();
        install_new(&database, &PROTOCOL_A, interface(0x11));
        install_new(&database, &PROTOCOL_A, interface(0x22));

        assert_eq!(database.locate_protocol(&PROTOCOL_A), Some(interface(0x11)));
        assert_eq!(database.locate_protocol(&PROTOCOL_B), None);
    }

    #[test]
    fn test_locate_handle_by_protocol_returns_only_the_matching_handles() {
        let database = ProtocolDatabase::new();
        let first = install_new(&database, &PROTOCOL_A, interface(0x11));
        install_new(&database, &PROTOCOL_B, interface(0x22));
        let third = install_new(&database, &PROTOCOL_A, interface(0x33));

        assert_eq!(database.locate_handle_by_protocol(&PROTOCOL_A), alloc::vec![first, third]);
        assert!(database.locate_handle_by_protocol(&PROTOCOL_C).is_empty());
        assert!(database.is_protocol_installed(&PROTOCOL_A));
        assert!(!database.is_protocol_installed(&PROTOCOL_C));
    }

    #[test]
    fn test_uninstalling_the_last_protocol_retires_the_handle() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));
        database.install_protocol(handle, &PROTOCOL_B, interface(0x22)).expect("second protocol installs");

        assert_eq!(database.uninstall_protocol(handle, &PROTOCOL_A, interface(0x11)), Ok(()));
        assert_eq!(database.all_handles(), alloc::vec![handle], "the handle still carries a protocol");

        assert_eq!(database.uninstall_protocol(handle, &PROTOCOL_B, interface(0x22)), Ok(()));
        assert!(database.all_handles().is_empty(), "the empty handle was retired");
    }

    #[test]
    fn test_uninstall_requires_the_interface_that_was_installed() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(database.uninstall_protocol(handle, &PROTOCOL_A, interface(0x22)), Err(efi::Status::NOT_FOUND));
        assert_eq!(database.uninstall_protocol(handle, &PROTOCOL_B, interface(0x11)), Err(efi::Status::NOT_FOUND));
        assert_eq!(database.handle_protocol(handle, &PROTOCOL_A), Some(interface(0x11)), "nothing was removed");
    }

    #[test]
    fn test_uninstall_rejects_a_null_or_unknown_handle() {
        let database = ProtocolDatabase::new();
        install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(
            database.uninstall_protocol(core::ptr::null_mut(), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert_eq!(
            database.uninstall_protocol(interface(0x999), &PROTOCOL_A, interface(0x11)),
            Err(efi::Status::INVALID_PARAMETER)
        );
    }

    #[test]
    fn test_registered_protocols_lists_each_guid_once() {
        let database = ProtocolDatabase::new();
        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));
        database.install_protocol(handle, &PROTOCOL_B, interface(0x22)).expect("second protocol installs");
        install_new(&database, &PROTOCOL_A, interface(0x33));

        assert_eq!(database.registered_protocols(), alloc::vec![PROTOCOL_A, PROTOCOL_B]);
    }

    #[test]
    fn test_a_notification_fires_with_the_interface_and_handle_that_were_installed() {
        let database = ProtocolDatabase::new();
        database.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify)).expect("registration succeeds");

        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0x11, handle as usize)]);
    }

    #[test]
    fn test_a_notification_stays_silent_for_other_protocols() {
        let database = ProtocolDatabase::new();
        database.register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify)).expect("registration succeeds");

        install_new(&database, &PROTOCOL_B, interface(0x22));

        assert!(events().is_empty());
    }

    #[test]
    fn test_a_native_callback_is_told_the_handle_but_not_the_interface() {
        let database = ProtocolDatabase::new();
        database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY))
            .expect("registration succeeds");

        let handle = install_new(&database, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0, handle as usize)]);
    }

    #[test]
    fn test_registering_the_same_callback_twice_reuses_its_token() {
        let database = ProtocolDatabase::new();

        let first = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();
        let again = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();
        assert_eq!(first, again, "the duplicate registration was folded into the first");

        let native = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY))
            .expect("registration succeeds")
            .as_ptr();
        let native_again = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(NATIVE_NOTIFY))
            .expect("registration succeeds")
            .as_ptr();
        assert_eq!(native, native_again);
        assert_ne!(first, native, "a native callback is a distinct registration");

        install_new(&database, &PROTOCOL_A, interface(0x11));
        assert_eq!(events().len(), 2, "each callback ran once, not twice");
    }

    #[test]
    fn test_a_different_guid_or_callback_gets_its_own_token() {
        let database = ProtocolDatabase::new();

        let a_efi = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();
        let b_efi = database
            .register_protocol_notify(&PROTOCOL_B, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();
        let a_other = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(other_efi_notify))
            .expect("registration succeeds")
            .as_ptr();

        assert_ne!(a_efi, b_efi, "the same callback on another GUID is a separate registration");
        assert_ne!(a_efi, a_other, "another callback on the same GUID is a separate registration");
    }

    #[test]
    fn test_unregistering_stops_the_notification() {
        let database = ProtocolDatabase::new();
        let token = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();

        assert_eq!(database.unregister_protocol_notify(&PROTOCOL_A, token), Ok(()));

        install_new(&database, &PROTOCOL_A, interface(0x11));
        assert!(events().is_empty());
    }

    #[test]
    fn test_unregister_rejects_a_null_unknown_or_mismatched_token() {
        let database = ProtocolDatabase::new();
        let token = database
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::efi(efi_notify))
            .expect("registration succeeds")
            .as_ptr();

        assert_eq!(
            database.unregister_protocol_notify(&PROTOCOL_A, core::ptr::null_mut()),
            Err(efi::Status::INVALID_PARAMETER)
        );
        assert_eq!(database.unregister_protocol_notify(&PROTOCOL_A, interface(0x999)), Err(efi::Status::NOT_FOUND));
        assert_eq!(
            database.unregister_protocol_notify(&PROTOCOL_B, token),
            Err(efi::Status::NOT_FOUND),
            "the token belongs to another GUID"
        );

        install_new(&database, &PROTOCOL_A, interface(0x11));
        assert_eq!(events().len(), 1, "the registration survived the failed unregisters");
    }

    #[test]
    fn test_a_notification_may_re_enter_the_database() {
        REENTRANT_DATABASE
            .register_protocol_notify(&PROTOCOL_A, ProtocolNotify::native(REENTRANT_NOTIFY))
            .expect("registration succeeds");

        let handle = install_new(&REENTRANT_DATABASE, &PROTOCOL_A, interface(0x11));

        assert_eq!(events(), alloc::vec![(PROTOCOL_A, 0, handle as usize)]);
        assert_eq!(
            REENTRANT_DATABASE.handle_protocol(handle, &PROTOCOL_C),
            Some(interface(0x33)),
            "the callback installed a protocol while the install that triggered it was still unwinding"
        );
    }
}
