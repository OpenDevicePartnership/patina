//! Protocol services for Patina components.
//!
//! [`ProtocolServices`] exposes UEFI protocol installation and discovery to components. The trait
//! itself is object-safe (and mockable) and works with the opaque [`Handle`] and [`ProtocolPtr`]
//! tokens rather than raw pointers. Type-safe access is provided by the [`ProtocolServicesExt`]
//! extension trait, whose generic methods bind a protocol interface type to its GUID using
//!  [`ProtocolInterface`], so components can install and locate protocols without handling a
//! raw pointer or GUID directly.
//!
//! Most components should prefer consuming higher-level Patina services over using protocols
//! directly. This service exists for the cases where a component must publish or consume a
//! protocol. For example, when providing or using code with C drivers that use protocols.
//!
//! # Consuming a protocol over time
//!
//! A UEFI protocol can be installed and uninstalled at any point, so a plain `&P` handed out once
//! can dangle later. [`ProtocolServicesExt`] offers four access styles, focusing on different
//! use cases:
//!
//! - [`with_protocol`](ProtocolServicesExt::with_protocol) - Run a closure with the interface.
//!   Useful for a single, immediate use. The reference cannot escape the closure.
//! - [`open_protocol`](ProtocolServicesExt::open_protocol) - A [`ProtocolGuard`] that dereferences
//!   to the interface for a block scope. Useful when several statements need the interface. Acts
//!   on a specific handle, obtained with [`locate_first_handle`](ProtocolServicesExt::locate_first_handle)
//!   or [`locate_handles_for`](ProtocolServicesExt::locate_handles_for).
//! - [`locate_token`](ProtocolServicesExt::locate_token) - A [`ProtocolToken`] that stores
//!   only a handle and never dangles. Useful for keeping a reference for the rest of boot.
//!   Re-validate each use with [`resolve`](ProtocolServicesExt::resolve).
//! - [`on_protocol_installed`](ProtocolServicesExt::on_protocol_installed) - A callback that runs
//!   for each present and future install of the protocol. Useful when another component installs the
//!   protocol later.
//!
//! [`open_protocol`](ProtocolServicesExt::open_protocol) records a usage in the protocol database
//! for as long as the guard is held (see [`ProtocolGuard`]), so the access is visible to other
//! agents through `OpenProtocolInformation()`. The other accessors do not record a usage.
//!
//! # Opening with attributes
//!
//! [`open_protocol`](ProtocolServicesExt::open_protocol) takes an [`OpenAttributes`] value and the
//! caller's own agent [`Handle`], obtained once from [`ProtocolServices::register_agent`] and kept
//! by the component (the same "stored dependencies" pattern used for other services). The
//! attribute chosen determines what happens when another agent tries to uninstall the protocol
//! while the guard is held:
//!
//! - [`OpenAttributes::Shared`] is advisory only and the DXE Core force-clears it automatically on
//!   uninstall, so it never blocks.
//! - [`OpenAttributes::Exclusive`] blocks uninstall until closed, and first attempts to disconnect
//!   an existing `ByDriver` holder so the exclusive open can succeed.
//! - [`OpenAttributes::ByDriver`] and [`OpenAttributes::ByDriverExclusive`] mark the caller as
//!   managing a controller with this protocol. Uninstall attempts to release them by calling the
//!   caller's driver binding `Stop()`, if one was installed with
//!   [`install_driver_binding`](ProtocolServicesExt::install_driver_binding). `Stop()` must close the
//!   usage to actually release it (returning `Ok(())` alone does not). See [`super::driver_binding`]
//!   for details.
//! - [`OpenAttributes::ByChildController`] records that a child controller depends on the parent's
//!   protocol. Like `Exclusive`, it blocks uninstall until closed.
//!
//! Dropping the guard releases the usage automatically.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::base::error::EfiError;
use crate::base::guid::BinaryGuid;
use crate::base::protocol::ProtocolInterface;
use crate::standard::efi;

use super::driver_binding;
pub use super::driver_binding::DriverBinding;
pub use super::handle::Handle;
pub use super::tpl::Tpl;

#[cfg(any(test, feature = "mockall"))]
use mockall::automock;

/// An opaque pointer to a protocol interface.
///
/// This token is returned by the erased [`ProtocolServices::locate_interface`] method and consumed
/// by [`ProtocolServices::install_interface`]. Components should generally use the typed methods on
/// [`ProtocolServicesExt`] rather than handling this token directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolPtr(NonNull<c_void>);

impl ProtocolPtr {
    /// Wraps a raw interface pointer.
    ///
    /// This is intended for use by service implementations and the typed extension methods, not
    /// component authors.
    #[doc(hidden)]
    pub fn from_raw(interface: *mut c_void) -> Option<Self> {
        NonNull::new(interface).map(Self)
    }

    /// Returns the raw interface pointer.
    ///
    /// This is intended for use by service implementations and the typed extension methods, not
    /// component authors.
    #[doc(hidden)]
    pub fn as_raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// A callback invoked with the handle of each interface installed for a watched protocol.
///
/// Supplied to [`ProtocolServices::register_install_notify`] (usually using
/// [`ProtocolServicesExt::on_protocol_installed`]). It runs for handles already present when the
/// notification is registered and for every future install, until the registration is cancelled.
pub type NotifyCallback = Box<dyn FnMut(Handle) + 'static>;

/// An opaque token for an active protocol-installation notification.
///
/// Returned by [`ProtocolServicesExt::on_protocol_installed`]. Keep it for as long as the
/// notification should stay active, then pass it to [`ProtocolServicesExt::cancel`] to stop it.
/// Dropping the token does not cancel the notification. The registration is intentionally long-lived
/// so it can outlive the component that created it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyRegistration {
    event: usize,
    registration: usize,
    context: usize,
}

impl NotifyRegistration {
    /// Wraps the raw pieces of a registration produced by the service implementation.
    ///
    /// This is intended for use by service implementations, not component authors.
    #[cfg(any(test, feature = "core"))]
    #[doc(hidden)]
    pub fn from_raw(event: *mut c_void, registration: *mut c_void, context: *mut c_void) -> Self {
        Self { event: event as usize, registration: registration as usize, context: context as usize }
    }

    /// Returns the raw notification event handle.
    #[cfg(any(test, feature = "core"))]
    #[doc(hidden)]
    pub fn event(&self) -> *mut c_void {
        self.event as *mut c_void
    }

    /// Returns the raw registration key.
    #[cfg(any(test, feature = "core"))]
    #[doc(hidden)]
    pub fn registration(&self) -> *mut c_void {
        self.registration as *mut c_void
    }

    /// Returns the raw notification context pointer.
    #[cfg(any(test, feature = "core"))]
    #[doc(hidden)]
    pub fn context(&self) -> *mut c_void {
        self.context as *mut c_void
    }
}

/// The UEFI `OpenProtocol()` attribute a component is opening a protocol with.
///
/// Each variant matches one of the attribute combinations the UEFI spec defines for
/// `EFI_BOOT_SERVICES.OpenProtocol()`. `BY_HANDLE_PROTOCOL` and `TEST_PROTOCOL` are not
/// represented. The former only matters for the `HandleProtocol()` compatibility, which is
/// served by [`ProtocolServices::interface_on_handle`], and the latter does not return an
/// actual interface, so it does not fit the interface of [`ProtocolServicesExt::open_protocol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAttributes {
    /// A shared, non-exclusive open. Matches `GET_PROTOCOL`.
    Shared,
    /// An exclusive open. Matches `EXCLUSIVE`.
    Exclusive,
    /// Opens on behalf of `controller`, as a driver. Matches `BY_DRIVER`.
    ByDriver {
        /// The controller this driver is managing the protocol on behalf of.
        controller: Handle,
    },
    /// Like [`Self::ByDriver`], but also attempts to disconnect an existing `BY_DRIVER` holder
    /// first. Matches `BY_DRIVER | EXCLUSIVE`.
    ByDriverExclusive {
        /// The controller this driver is managing the protocol on behalf of.
        controller: Handle,
    },
    /// Records that a child controller uses the protocol. Matches `BY_CHILD_CONTROLLER`.
    ByChildController {
        /// The child controller using the protocol. Must differ from the handle being opened.
        controller: Handle,
    },
}

impl OpenAttributes {
    /// Returns the controller handle carried by this attribute, if any.
    #[doc(hidden)]
    pub fn controller(&self) -> Option<Handle> {
        match *self {
            Self::Shared | Self::Exclusive => None,
            Self::ByDriver { controller }
            | Self::ByDriverExclusive { controller }
            | Self::ByChildController { controller } => Some(controller),
        }
    }

    /// Returns the raw `EFI_OPEN_PROTOCOL_*` attribute bits this value represents.
    #[doc(hidden)]
    pub fn bits(&self) -> u32 {
        match self {
            Self::Shared => efi::OPEN_PROTOCOL_GET_PROTOCOL,
            Self::Exclusive => efi::OPEN_PROTOCOL_EXCLUSIVE,
            Self::ByDriver { .. } => efi::OPEN_PROTOCOL_BY_DRIVER,
            Self::ByDriverExclusive { .. } => efi::OPEN_PROTOCOL_BY_DRIVER | efi::OPEN_PROTOCOL_EXCLUSIVE,
            Self::ByChildController { .. } => efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER,
        }
    }
}

/// A view of a protocol interface on a specific handle.
///
/// Returned by [`ProtocolServicesExt::open_protocol`]. It dereferences to the protocol interface
/// and ties that reference to the borrow of the service, so the reference cannot escape the scope
/// in which access is held. For access that must persist across dispatch, store a [`ProtocolToken`]
/// instead.
///
/// Opening the guard records a usage in the protocol database under the caller's agent handle and
/// chosen [`OpenAttributes`], visible to other agents through `OpenProtocolInformation()`. Dropping
/// the guard releases that usage. See the module documentation for what each attribute means for
/// uninstall.
#[must_use]
pub struct ProtocolGuard<'a, P: ProtocolInterface, S: ProtocolServices + ?Sized> {
    interface: &'a P,
    handle: Handle,
    agent: Handle,
    controller: Option<Handle>,
    service: &'a S,
}

impl<P: ProtocolInterface, S: ProtocolServices + ?Sized> core::ops::Deref for ProtocolGuard<'_, P, S> {
    type Target = P;

    fn deref(&self) -> &P {
        self.interface
    }
}

impl<P: ProtocolInterface, S: ProtocolServices + ?Sized> Drop for ProtocolGuard<'_, P, S> {
    fn drop(&mut self) {
        let result = self.service.close_interface(self.handle, P::PROTOCOL_GUID, self.agent, self.controller);
        debug_assert!(result.is_ok(), "closing a usage opened by open_protocol should not fail");
    }
}

/// A revocable reference to a protocol interface on a specific handle.
///
/// Unlike a `&P`, a token never dangles. It only stores a handle, so it is safe to keep for the
/// whole boot. Each use is re-validated through [`ProtocolServicesExt::resolve`], which returns
/// `None` if the interface is no longer installed on the handle.
///
/// The token identifies a specific handle. It does not detect handle reuse. For example, if the
/// DXE Core were to recycle a handle value for a different object that also installs `P`, a stale
/// token could resolve to that object. Since the Patina DXE Core does not recycle handle values,
/// this is not expected to occur in practice.
#[derive(Debug)]
pub struct ProtocolToken<P: ProtocolInterface> {
    handle: Handle,
    _marker: PhantomData<fn() -> P>,
}

impl<P: ProtocolInterface> ProtocolToken<P> {
    /// Creates a token referring to `handle`.
    ///
    /// This is intended for use by the typed extension methods. Component authors obtain a token
    /// from [`ProtocolServicesExt::locate_token`].
    #[doc(hidden)]
    pub fn new(handle: Handle) -> Self {
        Self { handle, _marker: PhantomData }
    }

    /// Returns the handle this token refers to.
    pub fn handle(&self) -> Handle {
        self.handle
    }
}

impl<P: ProtocolInterface> Clone for ProtocolToken<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ProtocolInterface> Copy for ProtocolToken<P> {}

/// Errors that can occur when using [`ProtocolServices`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProtocolError {
    /// A provided parameter was invalid.
    InvalidParameter,
    /// The requested protocol or handle was not found.
    NotFound,
    /// The system is out of resources to complete the operation.
    OutOfResources,
    /// The requested access conflicts with an existing exclusive or driver-owned usage.
    AccessDenied,
    /// The same agent and controller already have the protocol open with this attribute. The
    /// interface is still returned to the caller, so this does not surface as an `Err` from
    /// [`ProtocolServicesExt::open_protocol`].
    AlreadyStarted,
    /// An unexpected internal error occurred.
    Internal,
}

impl From<ProtocolError> for EfiError {
    fn from(value: ProtocolError) -> Self {
        match value {
            ProtocolError::InvalidParameter => EfiError::InvalidParameter,
            ProtocolError::NotFound => EfiError::NotFound,
            ProtocolError::OutOfResources => EfiError::OutOfResources,
            ProtocolError::AccessDenied => EfiError::AccessDenied,
            ProtocolError::AlreadyStarted => EfiError::AlreadyStarted,
            ProtocolError::Internal => EfiError::Unsupported,
        }
    }
}

impl From<EfiError> for ProtocolError {
    fn from(value: EfiError) -> Self {
        match value {
            EfiError::InvalidParameter => ProtocolError::InvalidParameter,
            EfiError::NotFound => ProtocolError::NotFound,
            EfiError::OutOfResources => ProtocolError::OutOfResources,
            EfiError::AccessDenied => ProtocolError::AccessDenied,
            EfiError::AlreadyStarted => ProtocolError::AlreadyStarted,
            _ => ProtocolError::Internal,
        }
    }
}

/// Protocol installation and discovery services.
///
/// This trait is object-safe and works with opaque tokens. Component authors should generally use
/// the type-safe methods provided by [`ProtocolServicesExt`] instead of calling these methods
/// directly.
///
/// This service is implemented by the Patina DXE Core. Components consume it by adding a
/// [`Service<dyn ProtocolServices>`](crate::component::service::Service) parameter to their entry
/// point.
#[cfg_attr(any(test, feature = "mockall"), automock)]
pub trait ProtocolServices {
    /// Installs a protocol interface identified by `protocol` on a handle.
    ///
    /// If `handle` is `None`, a new handle is created. The (possibly new) handle is returned.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidParameter`] if the interface could not be installed.
    fn install_interface(
        &self,
        handle: Option<Handle>,
        protocol: BinaryGuid,
        interface: ProtocolPtr,
    ) -> Result<Handle, ProtocolError>;

    /// Uninstalls a protocol interface identified by `protocol` from `handle`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if the handle/protocol/interface is not present.
    fn uninstall_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        interface: ProtocolPtr,
    ) -> Result<(), ProtocolError>;

    /// Locates the first interface installed for `protocol`, from any handle.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no matching interface is installed.
    fn locate_interface(&self, protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError>;

    /// Returns all handles that have `protocol` installed.
    ///
    /// Returns an empty list if no handle has the protocol installed.
    fn locate_handles(&self, protocol: BinaryGuid) -> Result<Vec<Handle>, ProtocolError>;

    /// Returns the interface for `protocol` installed on a specific `handle`.
    ///
    /// This is a raw interface lookup. Unlike the UEFI `OpenProtocol()`/`HandleProtocol()` boot
    /// services, it does not record a usage (`AgentHandle`/`ControllerHandle`/`Attributes`) in the
    /// protocol database, so it has no effect on whether the protocol can later be uninstalled.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if `handle` does not have `protocol` installed.
    fn interface_on_handle(&self, handle: Handle, protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError>;

    /// Opens `protocol` on `handle`, recording a usage in the protocol database so `protocol` is
    /// visible to other agents through `OpenProtocolInformation()`.
    ///
    /// `agent` should be a handle obtained once from [`Self::register_agent`] and kept by the
    /// caller. `attributes` selects the UEFI `OpenProtocol()` semantics to use. See the module
    /// documentation for what each variant means for uninstall. Component authors should
    /// generally use [`ProtocolServicesExt::open_protocol`], which pairs this with
    /// [`Self::close_interface`] automatically through a [`ProtocolGuard`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if `handle` does not have `protocol` installed.
    /// Returns [`ProtocolError::AccessDenied`] if `attributes` conflicts with an existing usage
    /// held by a different agent (for example, another agent already holds `Exclusive` or
    /// `ByDriver`).
    fn open_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        agent: Handle,
        attributes: OpenAttributes,
    ) -> Result<ProtocolPtr, ProtocolError>;

    /// Releases a usage previously recorded by [`Self::open_interface`] for the same `agent` and
    /// `controller`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no matching usage is currently open.
    fn close_interface(
        &self,
        handle: Handle,
        protocol: BinaryGuid,
        agent: Handle,
        controller: Option<Handle>,
    ) -> Result<(), ProtocolError>;

    /// Creates a fresh, stable handle a component can use as its own agent identity.
    ///
    /// Call this once and keep the returned [`Handle`] as a component field (the same "stored
    /// dependencies" pattern used for other services), then pass it as `agent` to
    /// [`Self::open_interface`]/[`Self::close_interface`]. The handle has no protocols installed on
    /// it beyond an internal marker, so it exists only as an identity.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::Internal`] if a handle could not be created.
    fn register_agent(&self) -> Result<Handle, ProtocolError>;

    /// Registers `callback` to run for each handle on which `protocol` is installed.
    ///
    /// The callback runs once for each handle that already has the protocol installed, and again
    /// for every future install, until the returned [`NotifyRegistration`] is cancelled with
    /// [`Self::cancel_install_notify`]. The callback runs at `notify_tpl`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::OutOfResources`] if the notification could not be registered.
    fn register_install_notify(
        &self,
        protocol: BinaryGuid,
        notify_tpl: Tpl,
        callback: NotifyCallback,
    ) -> Result<NotifyRegistration, ProtocolError>;

    /// Cancels a notification previously registered with [`Self::register_install_notify`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidParameter`] if `registration` is not an active registration.
    fn cancel_install_notify(&self, registration: NotifyRegistration) -> Result<(), ProtocolError>;
}

// `Sealed` is inside the private `sealed` module so that it is `pub` (can be a supertrait of `IntoStaticInterface`)
// but it cannot be implemented itself outside the module (https://rust-lang.github.io/api-guidelines/future-proofing.html).
mod sealed {
    //! Restricts [`IntoStaticInterface`] to the two forms `install_protocol` supports.
    pub trait Sealed {}
    impl<P> Sealed for &'static P {}
    impl<P> Sealed for &'static mut P {}
    impl<P> Sealed for alloc::boxed::Box<P> {}
}

/// A value that [`ProtocolServicesExt::install_protocol`] can commit as a permanent interface.
///
/// Implemented for `&'static P` (and `&'static mut P`), which install at no extra cost since they
/// are already permanent, and for `Box<P>`, which is only leaked once installation actually
/// succeeds. A failed install drops the box instead of leaking it.
pub trait IntoStaticInterface<P>: sealed::Sealed {
    /// Returns a pointer to the interface without giving up ownership yet.
    #[doc(hidden)]
    fn as_interface_ptr(&self) -> *const P;

    /// Called once installation succeeds, committing the interface as permanently `'static`.
    #[doc(hidden)]
    fn commit(self);
}

impl<P> IntoStaticInterface<P> for &'static P {
    fn as_interface_ptr(&self) -> *const P {
        *self
    }

    fn commit(self) {}
}

impl<P> IntoStaticInterface<P> for &'static mut P {
    fn as_interface_ptr(&self) -> *const P {
        &raw const **self
    }

    fn commit(self) {}
}

impl<P> IntoStaticInterface<P> for Box<P> {
    fn as_interface_ptr(&self) -> *const P {
        &raw const **self
    }

    fn commit(self) {
        Box::leak(self);
    }
}

/// Type-safe extension methods for [`ProtocolServices`].
///
/// These generic methods bind a protocol interface type `P` to its GUID with [`ProtocolInterface`],
/// so callers do not need to handle a raw pointer or GUID. The trait is implemented for every
/// [`ProtocolServices`] implementor (including [`Service<dyn ProtocolServices>`]).
///
/// [`Service<dyn ProtocolServices>`]: crate::component::service::Service
///
/// # Examples
///
/// ```rust,no_run
/// use patina::component::service::{Service, uefi_services::protocol::{ProtocolServices, ProtocolServicesExt}};
/// use patina::error::Result;
/// use patina::standard::efi::protocols::graphics_output::Protocol as GraphicsOutput;
///
/// fn entry_point(protocols: Service<dyn ProtocolServices>) -> Result<()> {
///     if let Ok(gop) = protocols.locate_protocol::<GraphicsOutput>() {
///         let mode = gop.mode;
///         // ... use the protocol ...
///     }
///     Ok(())
/// }
/// ```
pub trait ProtocolServicesExt: ProtocolServices {
    /// Locates the first interface for protocol `P` and returns a typed reference to it.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no interface for `P` is installed.
    fn locate_protocol<P: ProtocolInterface>(&self) -> Result<&'static P, ProtocolError> {
        let ptr = self.locate_interface(P::PROTOCOL_GUID)?;
        // SAFETY: `ProtocolInterface` is an `unsafe trait` whose contract guarantees that the
        // interface installed for `P::PROTOCOL_GUID` has the memory layout of `P`. The core
        // returns a non-null pointer to that interface.
        Ok(unsafe { &*(ptr.as_raw() as *const P) })
    }

    /// Installs `interface` for protocol `P` on a handle, creating a new handle if `handle` is
    /// `None`.
    ///
    /// Accepts a `&'static P`, which installs directly, or a `Box<P>`, which is only leaked once
    /// installation succeeds, so a failed install does not leak memory.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidParameter`] if the interface could not be installed.
    fn install_protocol<P: ProtocolInterface>(
        &self,
        handle: Option<Handle>,
        interface: impl IntoStaticInterface<P>,
    ) -> Result<Handle, ProtocolError> {
        let ptr = ProtocolPtr::from_raw(interface.as_interface_ptr() as *mut c_void)
            .ok_or(ProtocolError::InvalidParameter)?;
        let handle = self.install_interface(handle, P::PROTOCOL_GUID, ptr)?;
        interface.commit();
        Ok(handle)
    }

    /// Returns all handles that have protocol `P` installed.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no handle has protocol `P` installed.
    fn locate_handles_for<P: ProtocolInterface>(&self) -> Result<Vec<Handle>, ProtocolError> {
        self.locate_handles(P::PROTOCOL_GUID)
    }

    /// Locates the first handle with protocol `P` installed.
    ///
    /// Useful for obtaining a [`Handle`] to pass to [`Self::open_protocol`] or
    /// [`Self::with_protocol_on`], which act on a specific handle rather than any installed
    /// interface.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no handle has `P` installed.
    fn locate_first_handle<P: ProtocolInterface>(&self) -> Result<Handle, ProtocolError> {
        self.locate_handles(P::PROTOCOL_GUID)?.into_iter().next().ok_or(ProtocolError::NotFound)
    }

    /// Runs `f` with the first installed interface for protocol `P`.
    ///
    /// Use this for a single, immediate use of a protocol. The reference passed to `f` cannot
    /// escape the closure, so it cannot dangle.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no interface for `P` is installed.
    fn with_protocol<P: ProtocolInterface, R>(&self, f: impl FnOnce(&P) -> R) -> Result<R, ProtocolError> {
        let ptr = self.locate_interface(P::PROTOCOL_GUID)?;
        // SAFETY: `ProtocolInterface` guarantees the interface installed for `P::PROTOCOL_GUID` has
        // the layout of `P`. The pointer is non-null and valid for the duration of the call.
        let interface = unsafe { &*(ptr.as_raw() as *const P) };
        Ok(f(interface))
    }

    /// Runs `f` with the interface for protocol `P` installed on a specific `handle`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if `handle` does not have `P` installed.
    fn with_protocol_on<P: ProtocolInterface, R>(
        &self,
        handle: Handle,
        f: impl FnOnce(&P) -> R,
    ) -> Result<R, ProtocolError> {
        let ptr = self.interface_on_handle(handle, P::PROTOCOL_GUID)?;
        // SAFETY: as in `with_protocol`.
        let interface = unsafe { &*(ptr.as_raw() as *const P) };
        Ok(f(interface))
    }

    /// Opens protocol `P` on `handle` for `agent` with `attributes`, returning a [`ProtocolGuard`]
    /// for block-scoped access.
    ///
    /// The guard dereferences to the interface and keeps the reference tied to this service
    /// borrow. Use it when several statements need the interface. For a single expression prefer
    /// [`Self::with_protocol_on`].
    ///
    /// `agent` should be a handle obtained once from [`ProtocolServices::register_agent`] and kept
    /// by the caller. `attributes` selects the UEFI `OpenProtocol()` semantics to use. See the
    /// module documentation for what each [`OpenAttributes`] variant means for uninstall. Dropping
    /// the guard releases the usage automatically.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if `handle` does not have `P` installed.
    /// Returns [`ProtocolError::AccessDenied`] if `attributes` conflicts with an existing usage
    /// held by a different agent.
    fn open_protocol<P: ProtocolInterface>(
        &self,
        handle: Handle,
        agent: Handle,
        attributes: OpenAttributes,
    ) -> Result<ProtocolGuard<'_, P, Self>, ProtocolError> {
        let controller = attributes.controller();
        let ptr = self.open_interface(handle, P::PROTOCOL_GUID, agent, attributes)?;
        // SAFETY: as in `with_protocol`. The reference is bound to `&self`, so it cannot outlive
        // the borrow of the service.
        let interface = unsafe { &*(ptr.as_raw() as *const P) };
        Ok(ProtocolGuard { interface, handle, agent, controller, service: self })
    }

    /// Locates the first handle with protocol `P` and returns a [`ProtocolToken`].
    ///
    /// The token can be stored and re-validated later with [`Self::resolve`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::NotFound`] if no handle has `P` installed.
    fn locate_token<P: ProtocolInterface>(&self) -> Result<ProtocolToken<P>, ProtocolError> {
        Ok(ProtocolToken::new(self.locate_first_handle::<P>()?))
    }

    /// Resolves `token`, returning the interface if `P` is still installed on the token's handle.
    ///
    /// Returns `None` if the interface has since been uninstalled. Re-validating on each use is
    /// what makes a token safe to hold across time.
    fn resolve<P: ProtocolInterface>(&self, token: &ProtocolToken<P>) -> Option<&P> {
        let ptr = self.interface_on_handle(token.handle(), P::PROTOCOL_GUID).ok()?;
        // SAFETY: as in `with_protocol`. The reference is bound to `&self`.
        Some(unsafe { &*(ptr.as_raw() as *const P) })
    }

    /// Registers `callback` to run for each present and future install of protocol `P`.
    ///
    /// Store the returned [`NotifyRegistration`] for as long as the notification should stay active
    /// (for example in a service), and cancel it with [`Self::cancel`] when done. Dropping the
    /// registration does not cancel it. The callback runs at `notify_tpl`.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::OutOfResources`] if the notification could not be registered.
    fn on_protocol_installed<P: ProtocolInterface>(
        &self,
        notify_tpl: Tpl,
        callback: impl FnMut(Handle) + 'static,
    ) -> Result<NotifyRegistration, ProtocolError> {
        self.register_install_notify(P::PROTOCOL_GUID, notify_tpl, Box::new(callback))
    }

    /// Cancels a notification returned by [`Self::on_protocol_installed`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::InvalidParameter`] if `registration` is not active.
    fn cancel(&self, registration: NotifyRegistration) -> Result<(), ProtocolError> {
        self.cancel_install_notify(registration)
    }

    /// Builds and installs a driver binding protocol from `binding`, using a generated agent
    /// handle as both the driver binding's image handle and its own handle.
    ///
    /// This is how a component participates in the UEFI driver model. Once installed, `binding`
    /// receives `Supported()`/`Start()`/`Stop()` calls from `ConnectController()`/`DisconnectController()`,
    /// and an existing [`OpenAttributes::ByDriver`] or [`OpenAttributes::ByDriverExclusive`] usage recorded
    /// under the returned handle can be released or preempted, since there is now a `Stop()` for the core to
    /// call. See [`super::driver_binding`] for details.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::Internal`] if a handle could not be created, or
    /// [`ProtocolError::InvalidParameter`] if the driver binding protocol could not be installed.
    fn install_driver_binding<B: DriverBinding + 'static>(&self, binding: B) -> Result<Handle, ProtocolError> {
        let handle = self.register_agent()?;
        let protocol = efi::protocols::driver_binding::Protocol {
            version: B::VERSION,
            supported: driver_binding::supported_trampoline::<B>,
            start: driver_binding::start_trampoline::<B>,
            stop: driver_binding::stop_trampoline::<B>,
            driver_binding_handle: handle.as_raw(),
            image_handle: handle.as_raw(),
        };
        let holder = Box::new(driver_binding::DriverBindingHolder { protocol, binding });
        self.install_protocol::<driver_binding::DriverBindingHolder<B>>(Some(handle), holder)
    }
}

impl<T: ProtocolServices + ?Sized> ProtocolServicesExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct FakeProtocol {
        value: u32,
    }

    // SAFETY: This test-only type declares a fixed GUID that is used consistently for both the
    // (mocked) install and locate paths, so the GUID to layout binding holds within the test.
    unsafe impl ProtocolInterface for FakeProtocol {
        const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("abcdabcd-1234-5678-9abc-def012345678");
    }

    static FAKE_INSTANCE: FakeProtocol = FakeProtocol { value: 42 };

    #[test]
    fn test_protocol_services_error_to_efi() {
        assert_eq!(EfiError::from(ProtocolError::InvalidParameter), EfiError::InvalidParameter);
        assert_eq!(EfiError::from(ProtocolError::NotFound), EfiError::NotFound);
        assert_eq!(EfiError::from(ProtocolError::OutOfResources), EfiError::OutOfResources);
        assert_eq!(EfiError::from(ProtocolError::AccessDenied), EfiError::AccessDenied);
        assert_eq!(EfiError::from(ProtocolError::AlreadyStarted), EfiError::AlreadyStarted);
        assert_eq!(EfiError::from(ProtocolError::Internal), EfiError::Unsupported);
    }

    #[test]
    fn test_protocol_services_error_from_efi() {
        assert_eq!(ProtocolError::from(EfiError::NotFound), ProtocolError::NotFound);
        assert_eq!(ProtocolError::from(EfiError::OutOfResources), ProtocolError::OutOfResources);
        assert_eq!(ProtocolError::from(EfiError::AccessDenied), ProtocolError::AccessDenied);
        assert_eq!(ProtocolError::from(EfiError::AlreadyStarted), ProtocolError::AlreadyStarted);
        assert_eq!(ProtocolError::from(EfiError::DeviceError), ProtocolError::Internal);
    }

    #[test]
    fn test_protocol_services_handle_null() {
        assert!(Handle::from_raw(core::ptr::null_mut()).is_none());
        assert!(ProtocolPtr::from_raw(core::ptr::null_mut()).is_none());
    }

    #[test]
    fn test_protocol_services_ext_locate_protocol() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_interface().times(1).returning(|guid| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            Ok(ProtocolPtr::from_raw(&raw const FAKE_INSTANCE as *mut c_void).unwrap())
        });

        let located = mock.locate_protocol::<FakeProtocol>().unwrap();
        assert_eq!(located.value, 42);
    }

    #[test]
    fn test_protocol_services_ext_locate_handles_for() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_handles().times(1).returning(|guid| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            Ok(Vec::new())
        });

        assert!(mock.locate_handles_for::<FakeProtocol>().unwrap().is_empty());
    }

    #[test]
    fn test_protocol_services_ext_locate_first_handle() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_handles().times(1).returning(|guid| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            Ok(alloc::vec![fake_handle(), fake_controller()])
        });

        assert_eq!(mock.locate_first_handle::<FakeProtocol>().unwrap(), fake_handle());
    }

    #[test]
    fn test_protocol_services_ext_locate_first_handle_not_found() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_handles().times(1).returning(|_| Ok(Vec::new()));

        assert_eq!(mock.locate_first_handle::<FakeProtocol>().unwrap_err(), ProtocolError::NotFound);
    }

    fn fake_handle() -> Handle {
        Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()
    }

    /// A second, distinct deterministic handle, so tests can tell an agent apart from a controller.
    fn fake_controller() -> Handle {
        Handle::from_raw(0x1000_usize as *mut c_void).unwrap()
    }

    fn fake_ptr() -> ProtocolPtr {
        ProtocolPtr::from_raw(&raw const FAKE_INSTANCE as *mut c_void).unwrap()
    }

    #[test]
    fn test_protocol_services_ext_install_protocol_static_ref() {
        let mut mock = MockProtocolServices::new();
        mock.expect_install_interface().times(1).returning(|handle, guid, _| {
            assert!(handle.is_none());
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            Ok(fake_handle())
        });

        let handle = mock.install_protocol(None, &FAKE_INSTANCE).unwrap();
        assert_eq!(handle, fake_handle());
    }

    // A protocol type that records whether it was dropped, so tests can tell a leak apart from a drop.
    #[repr(C)]
    struct TrackedProtocol {
        _value: u32,
        dropped: alloc::sync::Arc<core::sync::atomic::AtomicBool>,
    }

    impl Drop for TrackedProtocol {
        fn drop(&mut self) {
            self.dropped.store(true, core::sync::atomic::Ordering::SeqCst);
        }
    }

    // SAFETY: This test-only type is only ever installed and located under its own GUID.
    unsafe impl ProtocolInterface for TrackedProtocol {
        const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("11111111-2222-3333-4444-555555555555");
    }

    #[test]
    fn test_protocol_services_ext_install_protocol_boxed_leaks_on_success() {
        let dropped = alloc::sync::Arc::new(core::sync::atomic::AtomicBool::new(false));
        let protocol = Box::new(TrackedProtocol { _value: 7, dropped: dropped.clone() });

        let mut mock = MockProtocolServices::new();
        mock.expect_install_interface().times(1).returning(|_, guid, _| {
            assert_eq!(guid, TrackedProtocol::PROTOCOL_GUID);
            Ok(fake_handle())
        });

        let handle = mock.install_protocol(None, protocol).unwrap();
        assert_eq!(handle, fake_handle());
        assert!(!dropped.load(core::sync::atomic::Ordering::SeqCst), "a successful install must leak, not drop");
    }

    #[test]
    fn test_protocol_services_ext_install_protocol_boxed_drops_on_failure() {
        let dropped = alloc::sync::Arc::new(core::sync::atomic::AtomicBool::new(false));
        let protocol = Box::new(TrackedProtocol { _value: 7, dropped: dropped.clone() });

        let mut mock = MockProtocolServices::new();
        mock.expect_install_interface().times(1).returning(|_, _, _| Err(ProtocolError::InvalidParameter));

        let result = mock.install_protocol(None, protocol);
        assert!(result.is_err());
        assert!(dropped.load(core::sync::atomic::Ordering::SeqCst), "a failed install must drop, not leak");
    }

    #[test]
    fn test_protocol_services_ext_with_protocol() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_interface().times(1).returning(|_| Ok(fake_ptr()));

        let value = mock.with_protocol::<FakeProtocol, _>(|p| p.value).unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn test_open_attributes_controller() {
        let controller = fake_handle();
        assert_eq!(OpenAttributes::Shared.controller(), None);
        assert_eq!(OpenAttributes::Exclusive.controller(), None);
        assert_eq!(OpenAttributes::ByDriver { controller }.controller(), Some(controller));
        assert_eq!(OpenAttributes::ByDriverExclusive { controller }.controller(), Some(controller));
        assert_eq!(OpenAttributes::ByChildController { controller }.controller(), Some(controller));
    }

    #[test]
    fn test_open_attributes_bits() {
        let controller = fake_handle();
        assert_eq!(OpenAttributes::Shared.bits(), efi::OPEN_PROTOCOL_GET_PROTOCOL);
        assert_eq!(OpenAttributes::Exclusive.bits(), efi::OPEN_PROTOCOL_EXCLUSIVE);
        assert_eq!(OpenAttributes::ByDriver { controller }.bits(), efi::OPEN_PROTOCOL_BY_DRIVER);
        assert_eq!(
            OpenAttributes::ByDriverExclusive { controller }.bits(),
            efi::OPEN_PROTOCOL_BY_DRIVER | efi::OPEN_PROTOCOL_EXCLUSIVE
        );
        assert_eq!(OpenAttributes::ByChildController { controller }.bits(), efi::OPEN_PROTOCOL_BY_CHILD_CONTROLLER);
    }

    #[test]
    fn test_protocol_services_ext_open_protocol() {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(1).returning(|_, guid, opened_agent, attributes| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            assert_eq!(opened_agent, fake_handle());
            assert_eq!(attributes, OpenAttributes::Shared);
            Ok(fake_ptr())
        });
        mock.expect_close_interface().times(1).returning(|_, guid, closed_agent, controller| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            assert_eq!(closed_agent, fake_handle());
            assert_eq!(controller, None);
            Ok(())
        });

        let guard = mock.open_protocol::<FakeProtocol>(fake_handle(), fake_handle(), OpenAttributes::Shared).unwrap();
        assert_eq!(guard.value, 42);
        drop(guard);
    }

    #[test]
    fn test_protocol_services_ext_open_protocol_by_driver_carries_controller() {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(1).returning(|_, _, _, attributes| {
            assert_eq!(attributes, OpenAttributes::ByDriver { controller: fake_controller() });
            Ok(fake_ptr())
        });
        mock.expect_close_interface().times(1).returning(|_, _, _, closed_controller| {
            assert_eq!(closed_controller, Some(fake_controller()));
            Ok(())
        });

        let guard = mock
            .open_protocol::<FakeProtocol>(
                fake_handle(),
                fake_handle(),
                OpenAttributes::ByDriver { controller: fake_controller() },
            )
            .unwrap();
        drop(guard);
    }

    #[test]
    fn test_protocol_services_ext_register_agent() {
        let mut mock = MockProtocolServices::new();
        mock.expect_register_agent().times(1).returning(|| Ok(fake_handle()));

        assert_eq!(mock.register_agent().unwrap(), fake_handle());
    }

    #[test]
    fn test_protocol_services_ext_install_driver_binding() {
        struct NoOpBinding;
        impl DriverBinding for NoOpBinding {
            fn stop(&self, _agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
                Ok(())
            }
        }

        let mut mock = MockProtocolServices::new();
        mock.expect_register_agent().times(1).returning(|| Ok(fake_handle()));
        mock.expect_install_interface().times(1).returning(|handle, guid, _| {
            assert_eq!(handle, Some(fake_handle()));
            assert_eq!(guid, super::driver_binding::DriverBindingHolder::<NoOpBinding>::PROTOCOL_GUID);
            Ok(fake_handle())
        });

        let handle = mock.install_driver_binding(NoOpBinding).unwrap();
        assert_eq!(handle, fake_handle());
    }

    #[test]
    fn test_protocol_services_ext_token_resolve() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_handles().times(1).returning(|_| Ok(alloc::vec![fake_handle()]));
        mock.expect_interface_on_handle().times(1).returning(|_, _| Ok(fake_ptr()));

        let token = mock.locate_token::<FakeProtocol>().unwrap();
        let resolved = mock.resolve(&token).unwrap();
        assert_eq!(resolved.value, 42);
    }

    #[test]
    fn test_protocol_services_ext_token_resolve_gone() {
        let mut mock = MockProtocolServices::new();
        mock.expect_interface_on_handle().times(1).returning(|_, _| Err(ProtocolError::NotFound));

        let token = ProtocolToken::<FakeProtocol>::new(fake_handle());
        assert!(mock.resolve(&token).is_none());
    }

    #[test]
    fn test_protocol_services_ext_notify() {
        let mut mock = MockProtocolServices::new();
        mock.expect_register_install_notify().times(1).returning(|_, _, _| {
            Ok(NotifyRegistration::from_raw(core::ptr::dangling_mut::<c_void>(), 2 as *mut c_void, 3 as *mut c_void))
        });
        mock.expect_cancel_install_notify().times(1).returning(|reg| {
            assert_eq!(reg.event(), core::ptr::dangling_mut::<c_void>());
            Ok(())
        });

        let registration = mock.on_protocol_installed::<FakeProtocol>(Tpl::Callback, |_handle| {}).unwrap();
        assert!(mock.cancel(registration).is_ok());
    }
}
