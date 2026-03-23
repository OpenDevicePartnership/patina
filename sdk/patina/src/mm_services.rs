//! MM (Management Mode) Services interface.
//!
//! This module defines the [`MmServices`] trait — the safe Rust interface to the
//! PI `EFI_MM_SYSTEM_TABLE` services — and [`StandardMmServices`], a bridge that
//! attaches a concrete `MmServices` implementation to the C
//! [`EfiMmSystemTable`](crate::pi::mm_cis::EfiMmSystemTable).
//!
//! ## Direction
//!
//! A core (e.g. `patina_mm_user_core`) implements [`MmServices`] backed by its
//! own databases and registers that implementation with a [`StandardMmServices`]
//! via [`StandardMmServices::init`]. The `EfiMmSystemTable` function-pointer
//! thunks then resolve that bridge and forward each call **into** the registered
//! implementation.
//!
//! As a result the C function pointers exist purely for C callers: Rust code uses
//! the `MmServices` implementation directly and never dispatches through the
//! shared `EfiMmSystemTable` function pointers.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use core::{ffi::c_void, ptr::NonNull};

use alloc::vec::Vec;

use crate::pi::mm_cis::{MmNotifyFn, MmiHandlerEntryPoint};
use r_efi::efi;
use spin::Once;

/// Opaque token identifying a registered protocol-install notification.
///
/// Returned by [`MmServices::register_protocol_notify`] and passed back to
/// [`MmServices::unregister_protocol_notify`] to remove the registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration(NonNull<c_void>);

impl Registration {
    /// Wraps a non-null registration token.
    pub fn new(token: NonNull<c_void>) -> Self {
        Self(token)
    }

    /// Returns the raw token pointer.
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// The protocol-install event delivered to a native [`ProtocolNotify`] callback.
///
/// Bundles the event data into a single typed context so native callbacks never
/// receive the raw `*mut c_void` interface pointer that the C
/// `EFI_MM_NOTIFY_FN` ABI passes. When the just-installed interface is needed,
/// query it with [`MmServices::handle_protocol`] using [`handle`](Self::handle).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ProtocolInstalled<'a> {
    /// GUID of the protocol interface that was installed.
    pub protocol: &'a efi::Guid,
    /// Opaque handle the interface was installed on.
    pub handle: efi::Handle,
}

/// A protocol-install notification callback passed to
/// [`MmServices::register_protocol_notify`].
///
/// Rust callers register a native closure or function via
/// [`ProtocolNotify::native`]; it receives a typed [`ProtocolInstalled`] event
/// and never has to author an `extern "efiapi"` function or handle raw pointers.
/// The raw C form is still available via [`ProtocolNotify::efi`] for interop
/// with C drivers and the MM System Table thunks.
#[derive(Clone, Copy)]
pub enum ProtocolNotify {
    /// A native Rust callback receiving a typed [`ProtocolInstalled`] event.
    Native(&'static (dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync)),
    /// A raw C `EFI_MM_NOTIFY_FN` callback.
    Efi(MmNotifyFn),
}

impl ProtocolNotify {
    /// Register a native Rust callback.
    ///
    /// The callback receives a [`ProtocolInstalled`] event and must be `'static`
    /// (e.g. a plain function or a reference to a `static` closure) so it can be
    /// stored for the lifetime of MM.
    pub const fn native(callback: &'static (dyn Fn(ProtocolInstalled<'_>) -> efi::Status + Send + Sync)) -> Self {
        Self::Native(callback)
    }

    /// Wrap a raw C `EFI_MM_NOTIFY_FN` callback.
    pub const fn efi(callback: MmNotifyFn) -> Self {
        Self::Efi(callback)
    }

    /// Invoke the callback for a protocol-install event.
    ///
    /// # Safety
    ///
    /// For the [`Efi`](Self::Efi) variant this performs an FFI call into a C
    /// `EFI_MM_NOTIFY_FN`; the caller must ensure `interface` and `handle` are
    /// valid arguments for that callback.
    pub unsafe fn invoke(&self, protocol: &efi::Guid, interface: *mut c_void, handle: efi::Handle) -> efi::Status {
        match self {
            Self::Native(callback) => callback(ProtocolInstalled { protocol, handle }),
            // SAFETY: `protocol` is a valid reference for the duration of the call; the caller
            // upholds the validity of `interface` and `handle` per this method's contract.
            Self::Efi(callback) => unsafe { callback(protocol as *const efi::Guid, interface, handle) },
        }
    }
}

impl core::fmt::Debug for ProtocolNotify {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Native(_) => f.write_str("ProtocolNotify::Native(..)"),
            Self::Efi(callback) => write!(f, "ProtocolNotify::Efi({:#x})", *callback as usize),
        }
    }
}

/// Bridges a concrete [`MmServices`] implementation to the C `EfiMmSystemTable`.
///
/// A core implements [`MmServices`] (backed by its own state) and registers that
/// implementation here with [`init`](Self::init). The `EfiMmSystemTable` thunks
/// then forward every call into the registered provider, so the implementation
/// is reached directly — never through the C function pointers.
pub struct StandardMmServices {
    provider: Once<&'static dyn MmServices>,
}

// SAFETY: `provider` is written once via `init` and only read afterwards. The
// registered implementation is itself `Sync`, and the bridge is shared
// read-only, so it is safe to share across threads.
unsafe impl Sync for StandardMmServices {}
// SAFETY: Same as above.
unsafe impl Send for StandardMmServices {}

impl StandardMmServices {
    /// Create an uninitialized bridge.
    ///
    /// Register the backing implementation with [`init`](Self::init) before any
    /// service method (or `EfiMmSystemTable` thunk) is invoked.
    pub const fn new_uninit() -> Self {
        Self { provider: Once::new() }
    }

    /// Register the [`MmServices`] implementation to forward calls to.
    ///
    /// The first registration wins; subsequent calls are ignored.
    pub fn init(&self, provider: &'static dyn MmServices) {
        self.provider.call_once(|| provider);
    }

    /// Returns `true` once a provider has been registered.
    pub fn is_init(&self) -> bool {
        self.provider.is_completed()
    }

    /// Returns the registered provider, or `None` if one has not been registered.
    pub fn get(&self) -> Option<&'static dyn MmServices> {
        self.provider.get().copied()
    }

    /// Returns the registered provider.
    ///
    /// # Panics
    ///
    /// Panics if no provider has been registered via [`init`](Self::init).
    fn provider(&self) -> &'static dyn MmServices {
        *self.provider.get().expect("StandardMmServices provider is not initialized!")
    }
}

impl core::fmt::Debug for StandardMmServices {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StandardMmServices").field("initialized", &self.is_init()).finish()
    }
}

/// Safe Rust interface to the MM System Table services.
///
/// This is the MM analogue of
/// [`BootServices`](crate::boot_services::BootServices).
/// Each method maps 1:1 to a function pointer in
/// [`EfiMmSystemTable`](crate::pi::mm_cis::EfiMmSystemTable).
pub trait MmServices {
    // ---- Memory services ------------------------------------------------

    /// Allocate pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePool`
    fn allocate_pool(&self, pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status>;

    /// Free pool memory.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePool`
    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status>;

    /// Allocate pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmAllocatePages`
    fn allocate_pages(
        &self,
        alloc_type: efi::AllocateType,
        memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status>;

    /// Free pages.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmFreePages`
    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status>;

    // ---- Protocol services ----------------------------------------------

    /// Install a protocol interface on a handle.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmInstallProtocolInterface`
    ///
    /// # Safety
    ///
    /// `interface` must be a valid pointer to the protocol structure or null, and
    /// must remain valid for as long as the interface is installed.
    unsafe fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status>;

    /// Uninstall a protocol interface from a handle.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmUninstallProtocolInterface`
    ///
    /// # Safety
    ///
    /// `interface` must match the pointer that was installed.
    unsafe fn uninstall_protocol_interface(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status>;

    /// Query a handle for a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmHandleProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn handle_protocol(&self, handle: efi::Handle, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status>;

    /// Locate the first device that supports a protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmLocateProtocol`
    ///
    /// # Safety
    ///
    /// The returned pointer must be used carefully to avoid aliasing violations.
    unsafe fn locate_protocol(&self, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status>;

    // ---- MMI management -------------------------------------------------

    /// Manage (dispatch) an MMI.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiManage`
    ///
    /// # Safety
    ///
    /// `context`, `comm_buffer`, and `comm_buffer_size` are all optional pointers.
    /// But they must be valid if provided.
    unsafe fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status;

    /// Register an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerRegister`
    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status>;

    /// Unregister an MMI handler.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmiHandlerUnRegister`
    ///
    /// # Safety
    ///
    /// `dispatch_handle` should be a valid handle returned by a previous call to `mmi_handler_register`.
    /// Otherwise, this function will do nothing and return `EFI_NOT_FOUND`.
    /// So this operation is safe to call with an invalid handle, but it will not have any effect.
    unsafe fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status>;

    // ---- Configuration table --------------------------------------------

    /// Add, update, or remove a configuration-table entry.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmInstallConfigurationTable`
    ///
    /// # Safety
    ///
    /// `table` must remain valid for as long as the entry is installed, or be
    /// null to remove an existing entry.
    unsafe fn install_configuration_table(
        &self,
        guid: &efi::Guid,
        table: *mut c_void,
        table_size: usize,
    ) -> Result<(), efi::Status>;

    // ---- Protocol notifications -----------------------------------------

    /// Register a callback invoked when a protocol interface is installed.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmRegisterProtocolNotify` (register form)
    ///
    /// Returns a [`Registration`] token that can be passed to
    /// [`unregister_protocol_notify`](Self::unregister_protocol_notify).
    fn register_protocol_notify(
        &self,
        protocol: &efi::Guid,
        notify: ProtocolNotify,
    ) -> Result<Registration, efi::Status>;

    /// Unregister a previously registered protocol-install notification.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmRegisterProtocolNotify` (unregister form)
    fn unregister_protocol_notify(&self, protocol: &efi::Guid, registration: Registration) -> Result<(), efi::Status>;

    // ---- Handle location ------------------------------------------------

    /// Return the handles matching a search type and optional protocol.
    ///
    /// PI Spec: `EFI_MM_SYSTEM_TABLE.MmLocateHandle`
    fn locate_handle(
        &self,
        search_type: efi::LocateSearchType,
        protocol: Option<&efi::Guid>,
    ) -> Result<Vec<efi::Handle>, efi::Status>;
}

impl MmServices for StandardMmServices {
    fn allocate_pool(&self, pool_type: efi::MemoryType, size: usize) -> Result<*mut u8, efi::Status> {
        self.provider().allocate_pool(pool_type, size)
    }

    fn free_pool(&self, buffer: *mut u8) -> Result<(), efi::Status> {
        self.provider().free_pool(buffer)
    }

    fn allocate_pages(
        &self,
        alloc_type: efi::AllocateType,
        memory_type: efi::MemoryType,
        pages: usize,
    ) -> Result<u64, efi::Status> {
        self.provider().allocate_pages(alloc_type, memory_type, pages)
    }

    fn free_pages(&self, memory: u64, pages: usize) -> Result<(), efi::Status> {
        self.provider().free_pages(memory, pages)
    }

    unsafe fn install_protocol_interface(
        &self,
        handle: Option<efi::Handle>,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<efi::Handle, efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller of this `unsafe fn`
        // upholds the provider's contract.
        unsafe { self.provider().install_protocol_interface(handle, protocol, interface) }
    }

    unsafe fn uninstall_protocol_interface(
        &self,
        handle: efi::Handle,
        protocol: &efi::Guid,
        interface: *mut c_void,
    ) -> Result<(), efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().uninstall_protocol_interface(handle, protocol, interface) }
    }

    unsafe fn handle_protocol(&self, handle: efi::Handle, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().handle_protocol(handle, protocol) }
    }

    unsafe fn locate_protocol(&self, protocol: &efi::Guid) -> Result<*mut c_void, efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().locate_protocol(protocol) }
    }

    unsafe fn mmi_manage(
        &self,
        handler_type: Option<&efi::Guid>,
        context: *const c_void,
        comm_buffer: *mut c_void,
        comm_buffer_size: *mut usize,
    ) -> efi::Status {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().mmi_manage(handler_type, context, comm_buffer, comm_buffer_size) }
    }

    fn mmi_handler_register(
        &self,
        handler: MmiHandlerEntryPoint,
        handler_type: Option<&efi::Guid>,
    ) -> Result<efi::Handle, efi::Status> {
        self.provider().mmi_handler_register(handler, handler_type)
    }

    unsafe fn mmi_handler_unregister(&self, dispatch_handle: efi::Handle) -> Result<(), efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().mmi_handler_unregister(dispatch_handle) }
    }

    unsafe fn install_configuration_table(
        &self,
        guid: &efi::Guid,
        table: *mut c_void,
        table_size: usize,
    ) -> Result<(), efi::Status> {
        // SAFETY: forwarded unchanged to the registered provider; the caller upholds the contract.
        unsafe { self.provider().install_configuration_table(guid, table, table_size) }
    }

    fn register_protocol_notify(
        &self,
        protocol: &efi::Guid,
        notify: ProtocolNotify,
    ) -> Result<Registration, efi::Status> {
        self.provider().register_protocol_notify(protocol, notify)
    }

    fn unregister_protocol_notify(&self, protocol: &efi::Guid, registration: Registration) -> Result<(), efi::Status> {
        self.provider().unregister_protocol_notify(protocol, registration)
    }

    fn locate_handle(
        &self,
        search_type: efi::LocateSearchType,
        protocol: Option<&efi::Guid>,
    ) -> Result<Vec<efi::Handle>, efi::Status> {
        self.provider().locate_handle(search_type, protocol)
    }
}

/// Global bridge exposing the MM services to Patina components.
///
/// The MM User Core registers its [`MmServices`] provider here during startup
/// via [`register_component_mm_services`]; the [`MmServiceProvider`] component
/// parameter then resolves to it. This is kept separate from the
/// `EfiMmSystemTable` thunk bridge so component access does not depend on the C
/// system-table plumbing.
static COMPONENT_MM_SERVICES: StandardMmServices = StandardMmServices::new_uninit();

/// Register the [`MmServices`] provider that [`MmServiceProvider`] resolves to.
///
/// Called once by the MM User Core during startup. The first registration wins.
pub fn register_component_mm_services(provider: &'static dyn MmServices) {
    COMPONENT_MM_SERVICES.init(provider);
}

/// Returns the component [`MmServices`] provider, if one has been registered.
pub fn component_mm_services() -> Option<&'static dyn MmServices> {
    COMPONENT_MM_SERVICES.get()
}

/// A component parameter granting access to the MM System Table services
/// ([`MmServices`]).
///
/// This is the MM analogue of
/// [`StandardBootServices`](crate::boot_services::StandardBootServices) in the
/// component model. The provider must be registered via [`register_component_mm_services`]
/// (done by the MM User Core) before a component requesting this parameter is
/// dispatched; until then the parameter is unavailable and the component is not
/// scheduled.
#[derive(Clone, Copy)]
pub struct MmServiceProvider {
    /// The registered MM services implementation.
    services: &'static dyn MmServices,
}

impl MmServiceProvider {
    /// Returns the underlying [`MmServices`] implementation.
    #[inline(always)]
    pub fn get(&self) -> &'static dyn MmServices {
        self.services
    }
}

impl core::ops::Deref for MmServiceProvider {
    type Target = dyn MmServices + 'static;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.services
    }
}

// SAFETY: `MmServiceProvider` reads only the process-global component MM services
// provider (a `&'static dyn MmServices`) and never accesses component `Storage`,
// so it registers no storage access and cannot conflict with any other parameter.
unsafe impl crate::component::params::Param for MmServiceProvider {
    type State = ();
    type Item<'storage, 'state> = Self;

    unsafe fn get_param<'storage, 'state>(
        _state: &'state Self::State,
        _storage: crate::component::UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // `validate` guarantees the provider is registered before this is called.
        MmServiceProvider { services: component_mm_services().expect("component MM services not registered") }
    }

    fn validate(_state: &Self::State, _storage: crate::component::UnsafeStorageCell) -> bool {
        component_mm_services().is_some()
    }

    fn init_state(
        _storage: &mut crate::component::Storage,
        _meta: &mut crate::component::MetaData,
    ) -> Result<Self::State, alloc::borrow::Cow<'static, str>> {
        Ok(())
    }
}
