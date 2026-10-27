//! An optional, simple way for a component to manage controllers through the UEFI Driver Model.
//!
//! [`DriverBinding`] exposes the raw `Supported()`/`Start()`/`Stop()` calls. Most drivers repeat the same kind of
//! activities around them: open a protocol `BY_DRIVER` to probe and release it, open it again to start managing the
//! controller, keep some per-controller state, and close everything again when stopped. [`ControllerDriver`] lets the
//! SDK do that work, so a driver only describes the protocol it manages, the state it keeps per controller, and what
//! to do on start and stop. This helps reduce code in the component and makes its actual functionality easier to
//! understand.
//!
//! [`install_controller_driver`] installs the driver and returns a [`ControllerDriverRef`], which reads and modifies
//! a controller's state at any time, for example from an event callback.
//!
//! ## Example
//!
//! ```rust,no_run
//! use patina::component::service::{
//!     Service,
//!     uefi_services::{
//!         controller_driver::{ControllerDriver, StartContext, SupportedContext, install_controller_driver},
//!         driver::DriverServices,
//!         protocol::{ProtocolError, ProtocolServices, ProtocolServicesExt},
//!     },
//! };
//! use patina::standard::efi::protocols::block_io;
//!
//! /// Manages every controller with a Block I/O protocol and counts how often it is used.
//! struct BlockCounter;
//!
//! impl ControllerDriver for BlockCounter {
//!     type Protocol = block_io::Protocol;
//!     type State = u64;
//!
//!     fn supported(&self, _context: &SupportedContext<'_, Self>) -> Result<(), ProtocolError> {
//!         Ok(())
//!     }
//!
//!     fn start(&self, _context: &StartContext<'_, Self>) -> Result<u64, ProtocolError> {
//!         Ok(0)
//!     }
//! }
//!
//! fn entry_point(
//!     protocols: Service<dyn ProtocolServices>,
//!     drivers: Service<dyn DriverServices>,
//! ) -> patina::error::Result<()> {
//!     let driver = install_controller_driver(protocols, BlockCounter)?;
//!     for controller in protocols.locate_handles_for::<block_io::Protocol>()? {
//!         // ConnectController() calls BlockCounter's supported() and start().
//!         let _ = drivers.connect_controller(controller, false);
//!     }
//!     for controller in driver.controllers() {
//!         driver.with_state(controller, |uses| *uses += 1)?;
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Opening other protocols for a controller
//!
//! A driver can depend on protocols on other handles, for example a child it created through a service binding
//! protocol. [`StartContext::open_by_driver`] opens such a protocol `BY_DRIVER` on behalf of the controller being
//! started. If that protocol is later uninstalled or reinstalled, or another agent opens it exclusively, the core
//! calls `Stop()` with the other handle. The SDK routes that request to every controller that opened a protocol on
//! the handle and calls [`ControllerDriver::dependency_lost`] for each:
//!
//! - [`DependencyAction::Stop`] (the default) stops the whole controller.
//! - [`DependencyAction::Release`] keeps managing the controller and only releases the protocols opened on the handle.
//!
//! A controller stopped this way is not restarted automatically. `ReinstallProtocolInterface()` reconnects the
//! handle whose protocol changed, but not the controller that depended on it, so a driver that needs to keep running
//! across a reinstall of a dependency should return [`DependencyAction::Release`], or connect the controller again
//! with [`DriverServices::connect_controller`](super::driver::DriverServices::connect_controller).
//!
//! While a controller is being stopped, the driver may destroy the children it created. The SDK releases the
//! driver's usage of a child's protocols when the core asks, so the child can be destroyed from within
//! [`ControllerDriver::stop`].
//!
//! ## Bus drivers
//!
//! A bus driver records each child it creates with [`StartContext::open_for_child`], which opens
//! [`ControllerDriver::Protocol`] `BY_CHILD_CONTROLLER` for the child. `DisconnectController()` then asks the driver
//! to destroy specific children through [`ControllerDriver::stop_children`] before, or instead of, stopping the
//! controller itself.
//!
//! A bus driver that creates children over several `ConnectController()` calls, for example one child per
//! `RemainingDevicePath`, sets [`ControllerDriver::RESTARTABLE`]. [`ControllerDriver::supported`] is then also called
//! for a controller the driver already manages (see [`SupportedContext::is_managed`]), and
//! [`ControllerDriver::restart`] runs in place of [`ControllerDriver::start`].
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    fmt,
};

use crate::base::error::EfiError;
use crate::base::guid::BinaryGuid;
use crate::base::protocol::ProtocolInterface;
use crate::component::service::Service;
use crate::uefi::device_path::walker::BorrowedDevicePathWalker;

use super::driver_binding::DriverBinding;
use super::handle::Handle;
use super::protocol::{OpenAttributes, ProtocolError, ProtocolServices, ProtocolServicesExt};

/// A driver that manages controllers exposing [`Self::Protocol`].
///
/// Install it with [`install_controller_driver`]. The SDK opens `Self::Protocol` `BY_DRIVER` on each controller,
/// keeps it open while the controller is managed, stores the [`Self::State`] returned by [`Self::start`], and closes
/// everything after [`Self::stop`].
pub trait ControllerDriver: Sized + 'static {
    /// The protocol this driver manages a controller through.
    type Protocol: ProtocolInterface;

    /// The state kept for each managed controller, from [`Self::start`] until [`Self::stop`].
    type State: 'static;

    /// The driver binding protocol's version, used by `ConnectController()` to order candidate drivers.
    const VERSION: u32 = 0x10;

    /// Whether `ConnectController()` may call this driver again for a controller it already manages.
    ///
    /// Leave this `false` for a device driver or a bus driver that creates all its children on the first start, so
    /// `Supported()` reports `EFI_ALREADY_STARTED` for a managed controller. Set it for a bus driver that creates
    /// children over several calls, which then receives them through [`Self::restart`].
    const RESTARTABLE: bool = false;

    /// Checks whether this driver can manage a controller.
    ///
    /// This is only called after it has confirmed that [`Self::Protocol`] can be opened `BY_DRIVER` on the
    /// controller, or, for a [`Self::RESTARTABLE`] driver, that the driver already manages it.
    fn supported(&self, context: &SupportedContext<'_, Self>) -> Result<(), ProtocolError>;

    /// Starts managing a controller and returns the state to keep for it.
    ///
    /// Returning `Err` releases everything opened through `context`.
    fn start(&self, context: &StartContext<'_, Self>) -> Result<Self::State, ProtocolError>;

    /// Handles another `Start()` for a controller this [`Self::RESTARTABLE`] driver already manages, typically to
    /// create the children named by [`StartContext::remaining_device_path`].
    ///
    /// Returning `Err` releases everything opened through `context` during this call. The default reports
    /// [`ProtocolError::AlreadyStarted`].
    fn restart(&self, context: &StartContext<'_, Self>, state: &mut Self::State) -> Result<(), ProtocolError> {
        let _ = (context, state);
        Err(ProtocolError::AlreadyStarted)
    }

    /// Destroys `children` of `controller` while it stays managed.
    ///
    /// Called when `DisconnectController()` stops specific children recorded with [`StartContext::open_for_child`].
    /// On `Ok`, the protocols opened for those children are closed. Returning `Err` refuses the request. A driver
    /// that creates children must override this. The default reports [`ProtocolError::Unsupported`].
    fn stop_children(
        &self,
        controller: Handle,
        children: &[Handle],
        state: &mut Self::State,
    ) -> Result<(), ProtocolError> {
        let _ = (controller, children, state);
        Err(ProtocolError::Unsupported)
    }

    /// Stops managing `controller`.
    ///
    /// Returning `Err` refuses the request, and the controller stays managed with its state. On `Ok`, every protocol
    /// opened for the controller is closed and drops its state. The default always accepts.
    fn stop(&self, controller: Handle, state: &mut Self::State) -> Result<(), ProtocolError> {
        let _ = (controller, state);
        Ok(())
    }

    /// Decides what happens to `controller` when the core asks this driver to release `handle`, a handle it opened a
    /// protocol on with [`StartContext::open_by_driver`].
    ///
    /// The default, [`DependencyAction::Stop`], stops `controller` through [`Self::stop`].
    fn dependency_lost(&self, controller: Handle, handle: Handle, state: &mut Self::State) -> DependencyAction {
        let _ = (controller, handle, state);
        DependencyAction::Stop
    }
}

/// What [`ControllerDriver::dependency_lost`] does with a controller that loses access to a protocol it opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyAction {
    /// Stop managing the controller.
    Stop,
    /// Keep managing the controller and release only the protocols opened on the lost handle.
    ///
    /// The driver must not use those protocols afterwards.
    Release,
}

/// Information passed to [`ControllerDriver::supported`].
pub struct SupportedContext<'a, D: ControllerDriver> {
    controller: Handle,
    protocol: &'a D::Protocol,
    remaining_device_path: Option<BorrowedDevicePathWalker<'a>>,
    managed: bool,
}

impl<'a, D: ControllerDriver> SupportedContext<'a, D> {
    /// The controller being checked.
    pub fn controller(&self) -> Handle {
        self.controller
    }

    /// Returns `true` if the driver already manages the controller, which only happens for a
    /// [`ControllerDriver::RESTARTABLE`] driver.
    pub fn is_managed(&self) -> bool {
        self.managed
    }

    /// The controller's [`ControllerDriver::Protocol`] interface.
    pub fn protocol(&self) -> &'a D::Protocol {
        self.protocol
    }

    /// The unconsumed portion of the device path passed to `ConnectController()`, if any.
    pub fn remaining_device_path(&self) -> Option<BorrowedDevicePathWalker<'a>> {
        self.remaining_device_path.clone()
    }
}

/// Information and actions available to [`ControllerDriver::start`].
pub struct StartContext<'a, D: ControllerDriver> {
    controller: Handle,
    protocol: &'a D::Protocol,
    remaining_device_path: Option<BorrowedDevicePathWalker<'a>>,
    usages: &'a RefCell<Vec<Usage>>,
    inner: &'static Inner<D>,
}

impl<'a, D: ControllerDriver> StartContext<'a, D> {
    /// The controller being started.
    pub fn controller(&self) -> Handle {
        self.controller
    }

    /// The controller's [`ControllerDriver::Protocol`] interface.
    pub fn protocol(&self) -> &'a D::Protocol {
        self.protocol
    }

    /// The unconsumed portion of the device path passed to `ConnectController()`, if any.
    ///
    /// A bus driver that receives a path holding only an end node should start without creating children. See
    /// [`BorrowedDevicePathWalker::is_end_only`].
    pub fn remaining_device_path(&self) -> Option<BorrowedDevicePathWalker<'a>> {
        self.remaining_device_path.clone()
    }

    /// Opens protocol `Q` on `handle` `BY_DRIVER` for the controller being started.
    ///
    /// The usage is kept until the controller is stopped. If the core asks to release `handle` first,
    /// [`ControllerDriver::dependency_lost`] decides whether the controller is stopped.
    ///
    /// # Errors
    ///
    /// Returns the [`ProtocolError`] from opening the protocol, for example [`ProtocolError::AccessDenied`] if
    /// another driver already manages `handle` through `Q`.
    pub fn open_by_driver<Q: ProtocolInterface>(&self, handle: Handle) -> Result<&'a Q, ProtocolError> {
        let attributes = OpenAttributes::ByDriver { controller: Some(self.controller) };
        let ptr = self.inner.services.open_interface(handle, Q::PROTOCOL_GUID, self.inner.agent, attributes)?;
        self.usages.borrow_mut().push(Usage { handle, protocol: Q::PROTOCOL_GUID, controller: self.controller });
        // SAFETY: `ptr` is the interface installed for `Q`'s GUID, which `ProtocolInterface` binds to `Q`'s layout.
        // The `BY_DRIVER` usage cannot be released without this driver's `Stop()`, which is not called for this
        // controller before `start` returns, so the interface outlives `'a`.
        Ok(unsafe { &*ptr.as_raw().cast::<Q>() })
    }

    /// Records `child` as a child of the controller being started, by opening [`ControllerDriver::Protocol`] on the
    /// controller `BY_CHILD_CONTROLLER` for it.
    ///
    /// `DisconnectController()` uses this record to find the children to pass to
    /// [`ControllerDriver::stop_children`]. It is closed once the child or controller is stopped.
    ///
    /// # Errors
    ///
    /// Returns the [`ProtocolError`] from opening the protocol, for example [`ProtocolError::InvalidParameter`] if
    /// `child` is the controller itself.
    pub fn open_for_child(&self, child: Handle) -> Result<(), ProtocolError> {
        let protocol = D::Protocol::PROTOCOL_GUID;
        let attributes = OpenAttributes::ByChildController { controller: child };
        self.inner.services.open_interface(self.controller, protocol, self.inner.agent, attributes)?;
        self.usages.borrow_mut().push(Usage { handle: self.controller, protocol, controller: child });
        Ok(())
    }

    /// Returns a reference to the state of the controller being started, usable once `start` returns.
    ///
    /// The reference can be moved into a callback, such as an event notification, that needs the state later.
    pub fn state_ref(&self) -> ControllerStateRef<D> {
        ControllerStateRef { inner: self.inner, controller: self.controller }
    }
}

/// An error accessing a controller's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerError {
    /// The driver is not managing the controller, or the controller is being stopped.
    NotManaged,
    /// The controller's state is already in use further up the call stack.
    Busy,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotManaged => f.write_str("the driver is not managing the controller"),
            Self::Busy => f.write_str("the controller state is already in use"),
        }
    }
}

impl core::error::Error for ControllerError {}

impl From<ControllerError> for ProtocolError {
    fn from(value: ControllerError) -> Self {
        match value {
            ControllerError::NotManaged => ProtocolError::NotFound,
            ControllerError::Busy => ProtocolError::AccessDenied,
        }
    }
}

impl From<ControllerError> for EfiError {
    fn from(value: ControllerError) -> Self {
        ProtocolError::from(value).into()
    }
}

/// A handle to an installed [`ControllerDriver`], used to access the state of the controllers it manages.
pub struct ControllerDriverRef<D: ControllerDriver> {
    inner: &'static Inner<D>,
}

impl<D: ControllerDriver> Clone for ControllerDriverRef<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: ControllerDriver> Copy for ControllerDriverRef<D> {}

impl<D: ControllerDriver> ControllerDriverRef<D> {
    /// The driver's agent handle, which is also its driver binding handle.
    pub fn agent(&self) -> Handle {
        self.inner.agent
    }

    /// The controllers the driver currently manages.
    pub fn controllers(&self) -> Vec<Handle> {
        self.inner.controllers.borrow().iter().filter(|entry| !entry.stopping.get()).map(|e| e.controller).collect()
    }

    /// Returns `true` if the driver currently manages `controller`.
    pub fn is_managing(&self, controller: Handle) -> bool {
        self.inner.managed_entry(controller).is_some()
    }

    /// Runs `f` with mutable access to `controller`'s state.
    ///
    /// While `f` runs, a request to stop `controller` is refused.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError::NotManaged`] if the driver is not managing `controller`, or
    /// [`ControllerError::Busy`] if its state is already in use further up the call stack.
    pub fn with_state<R>(&self, controller: Handle, f: impl FnOnce(&mut D::State) -> R) -> Result<R, ControllerError> {
        let entry = self.inner.managed_entry(controller).ok_or(ControllerError::NotManaged)?;
        let mut state = entry.state.try_borrow_mut().map_err(|_| ControllerError::Busy)?;
        Ok(f(&mut state))
    }

    /// Runs `f` with mutable access to `controller`'s state and its [`ControllerDriver::Protocol`] interface.
    ///
    /// # Errors
    ///
    /// Same as [`Self::with_state`].
    pub fn with_controller<R>(
        &self,
        controller: Handle,
        f: impl FnOnce(&mut D::State, &D::Protocol) -> R,
    ) -> Result<R, ControllerError> {
        let entry = self.inner.managed_entry(controller).ok_or(ControllerError::NotManaged)?;
        let mut state = entry.state.try_borrow_mut().map_err(|_| ControllerError::Busy)?;
        let ptr = self
            .inner
            .services
            .interface_on_handle(controller, D::Protocol::PROTOCOL_GUID)
            .map_err(|_| ControllerError::NotManaged)?;
        // SAFETY: `ptr` is the interface installed for `D::Protocol`'s GUID. This driver holds it `BY_DRIVER`, and
        // uninstalling it requires this driver's `Stop()`, which is refused while `state` is borrowed.
        let protocol = unsafe { &*ptr.as_raw().cast::<D::Protocol>() };
        Ok(f(&mut state, protocol))
    }

    /// Returns a reference to `controller`'s state that can be stored and used later.
    pub fn state_ref(&self, controller: Handle) -> ControllerStateRef<D> {
        ControllerStateRef { inner: self.inner, controller }
    }
}

/// A reference to one controller's state, which can be stored, for example in an event callback.
///
/// The reference checks on each use that the controller is still managed. If the controller is stopped and started
/// again, the reference refers to the new state.
pub struct ControllerStateRef<D: ControllerDriver> {
    inner: &'static Inner<D>,
    controller: Handle,
}

impl<D: ControllerDriver> Clone for ControllerStateRef<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: ControllerDriver> Copy for ControllerStateRef<D> {}

impl<D: ControllerDriver> ControllerStateRef<D> {
    /// The controller this reference refers to.
    pub fn controller(&self) -> Handle {
        self.controller
    }

    /// Runs `f` with mutable access to the controller's state. See [`ControllerDriverRef::with_state`].
    ///
    /// # Errors
    ///
    /// Same as [`ControllerDriverRef::with_state`].
    pub fn with<R>(&self, f: impl FnOnce(&mut D::State) -> R) -> Result<R, ControllerError> {
        ControllerDriverRef { inner: self.inner }.with_state(self.controller, f)
    }

    /// Runs `f` with the controller's state and protocol. See [`ControllerDriverRef::with_controller`].
    ///
    /// # Errors
    ///
    /// Same as [`ControllerDriverRef::with_state`].
    pub fn with_protocol<R>(&self, f: impl FnOnce(&mut D::State, &D::Protocol) -> R) -> Result<R, ControllerError> {
        ControllerDriverRef { inner: self.inner }.with_controller(self.controller, f)
    }
}

/// Installs `driver` and returns a [`ControllerDriverRef`] to access the state of the controllers it manages.
///
/// The driver is installed on a new agent handle. Controllers are connected to it through `ConnectController()`,
/// for example with [`DriverServices::connect_controller`](super::driver::DriverServices::connect_controller).
///
/// # Errors
///
/// Returns an error if the agent handle or the driver binding protocol could not be created.
pub fn install_controller_driver<D: ControllerDriver>(
    protocols: Service<dyn ProtocolServices>,
    driver: D,
) -> Result<ControllerDriverRef<D>, ProtocolError> {
    let services: &'static dyn ProtocolServices = *protocols;
    let agent = services.register_agent()?;
    // Driver bindings are not uninstalled, so the driver lives for the rest of boot.
    let inner: &'static Inner<D> =
        Box::leak(Box::new(Inner { driver, services, agent, controllers: RefCell::new(Vec::new()) }));
    services.install_driver_binding_on(agent, ManagedDriver(inner))?;
    Ok(ControllerDriverRef { inner })
}

/// A protocol usage opened `BY_DRIVER` for a managed controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Usage {
    handle: Handle,
    protocol: BinaryGuid,
    controller: Handle,
}

struct Entry<D: ControllerDriver> {
    controller: Handle,
    usages: RefCell<Vec<Usage>>,
    state: RefCell<D::State>,
    stopping: Cell<bool>,
    stopping_children: Cell<bool>,
}

struct Inner<D: ControllerDriver> {
    driver: D,
    services: &'static dyn ProtocolServices,
    agent: Handle,
    controllers: RefCell<Vec<Rc<Entry<D>>>>,
}

impl<D: ControllerDriver> Inner<D> {
    fn managed_entry(&self, controller: Handle) -> Option<Rc<Entry<D>>> {
        self.controllers.borrow().iter().find(|entry| entry.controller == controller && !entry.stopping.get()).cloned()
    }

    /// Finds every entry holding a protocol on `handle`, either as its controller or through `open_by_driver`.
    fn entries_using(&self, handle: Handle) -> Vec<Rc<Entry<D>>> {
        self.controllers
            .borrow()
            .iter()
            .filter(|entry| entry.controller == handle || entry.usages.borrow().iter().any(|u| u.handle == handle))
            .cloned()
            .collect()
    }

    /// Releases everything `entry` holds on `handle`, stopping the entry if needed.
    fn release_handle(&self, entry: &Rc<Entry<D>>, handle: Handle) -> Result<(), ProtocolError> {
        if entry.stopping.get() || entry.stopping_children.get() {
            // The driver's own stop() or stop_children() is destroying a handle it opened a protocol on.
            self.close_usages_on(entry, handle);
            return Ok(());
        }

        let mut state = entry.state.try_borrow_mut().map_err(|_| ProtocolError::AccessDenied)?;
        if entry.controller != handle
            && self.driver.dependency_lost(entry.controller, handle, &mut state) == DependencyAction::Release
        {
            drop(state);
            self.close_usages_on(entry, handle);
            return Ok(());
        }

        entry.stopping.set(true);
        let result = self.driver.stop(entry.controller, &mut state);
        drop(state);
        if let Err(err) = result {
            entry.stopping.set(false);
            return Err(err);
        }

        self.controllers.borrow_mut().retain(|e| !Rc::ptr_eq(e, entry));
        let usages = entry.usages.take();
        self.close_all(&usages);
        Ok(())
    }

    /// Destroys `children` of `entry`'s controller through the driver, then closes what was opened for them.
    fn stop_children(&self, entry: &Entry<D>, children: &[Handle]) -> Result<(), ProtocolError> {
        let mut state = entry.state.try_borrow_mut().map_err(|_| ProtocolError::AccessDenied)?;
        entry.stopping_children.set(true);
        let result = self.driver.stop_children(entry.controller, children, &mut state);
        entry.stopping_children.set(false);
        drop(state);
        result?;

        let released: Vec<Usage> = {
            let mut usages = entry.usages.borrow_mut();
            let (released, kept) = core::mem::take(&mut *usages).into_iter().partition(|u| {
                children.contains(&u.handle) || (u.controller != entry.controller && children.contains(&u.controller))
            });
            *usages = kept;
            released
        };
        self.close_all(&released);
        Ok(())
    }

    fn close_usages_on(&self, entry: &Entry<D>, handle: Handle) {
        let released: Vec<Usage> = {
            let mut usages = entry.usages.borrow_mut();
            let (released, kept) = core::mem::take(&mut *usages).into_iter().partition(|u| u.handle == handle);
            *usages = kept;
            released
        };
        self.close_all(&released);
    }

    fn close_all(&self, usages: &[Usage]) {
        for usage in usages.iter().rev() {
            // A usage may already be gone, for example if its handle was destroyed while stopping.
            let _ = self.services.close_interface(usage.handle, usage.protocol, self.agent, Some(usage.controller));
        }
    }
}

/// The [`DriverBinding`] installed for a [`ControllerDriver`].
struct ManagedDriver<D: ControllerDriver>(&'static Inner<D>);

/// Runs [`ControllerDriver::restart`] for a controller the driver already manages.
fn restart<D: ControllerDriver>(
    inner: &'static Inner<D>,
    controller: Handle,
    remaining_device_path: Option<BorrowedDevicePathWalker<'_>>,
) -> Result<(), ProtocolError> {
    let entry = inner.managed_entry(controller).ok_or(ProtocolError::AlreadyStarted)?;
    let ptr = inner.services.interface_on_handle(controller, D::Protocol::PROTOCOL_GUID)?;
    // SAFETY: `ptr` is the interface installed for `D::Protocol`'s GUID, held `BY_DRIVER` by this driver, which
    // keeps it installed until this driver's `Stop()` runs. That is refused while `state` is borrowed below.
    let protocol = unsafe { &*ptr.as_raw().cast::<D::Protocol>() };
    let mut state = entry.state.try_borrow_mut().map_err(|_| ProtocolError::AccessDenied)?;
    let opened = entry.usages.borrow().len();
    let context = StartContext { controller, protocol, remaining_device_path, usages: &entry.usages, inner };
    let result = inner.driver.restart(&context, &mut state);
    drop(state);
    if result.is_err() {
        let added = entry.usages.borrow_mut().split_off(opened);
        inner.close_all(&added);
    }
    result
}

impl<D: ControllerDriver> DriverBinding for ManagedDriver<D> {
    const VERSION: u32 = D::VERSION;

    fn supported(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<BorrowedDevicePathWalker<'_>>,
    ) -> Result<(), ProtocolError> {
        let inner = self.0;
        let protocol = inner.services.open_protocol_or_started::<D::Protocol>(
            controller,
            agent,
            OpenAttributes::ByDriver { controller: None },
        )?;
        let managed = protocol.is_already_started();
        if managed && !(D::RESTARTABLE && inner.managed_entry(controller).is_some()) {
            return Err(ProtocolError::AlreadyStarted);
        }
        let context = SupportedContext { controller, protocol: &*protocol, remaining_device_path, managed };
        inner.driver.supported(&context)
    }

    fn start(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<BorrowedDevicePathWalker<'_>>,
    ) -> Result<(), ProtocolError> {
        let inner = self.0;
        let guid = D::Protocol::PROTOCOL_GUID;
        let ptr =
            match inner.services.open_interface(controller, guid, agent, OpenAttributes::ByDriver { controller: None })
            {
                Ok(ptr) => ptr,
                Err(ProtocolError::AlreadyStarted) if D::RESTARTABLE => {
                    return restart(inner, controller, remaining_device_path);
                }
                Err(err) => return Err(err),
            };
        // SAFETY: `ptr` is the interface installed for `D::Protocol`'s GUID. The `BY_DRIVER` usage keeps it installed
        // until this driver's `Stop()` runs, which cannot happen for this controller before `start` returns.
        let protocol = unsafe { &*ptr.as_raw().cast::<D::Protocol>() };
        let usages = RefCell::new(vec![Usage { handle: controller, protocol: guid, controller }]);
        let context = StartContext { controller, protocol, remaining_device_path, usages: &usages, inner };

        match inner.driver.start(&context) {
            Ok(state) => {
                let entry = Entry {
                    controller,
                    usages,
                    state: RefCell::new(state),
                    stopping: Cell::new(false),
                    stopping_children: Cell::new(false),
                };
                inner.controllers.borrow_mut().push(Rc::new(entry));
                Ok(())
            }
            Err(err) => {
                inner.close_all(&usages.into_inner());
                Err(err)
            }
        }
    }

    fn stop(&self, _agent: Handle, handle: Handle, children: &[Handle]) -> Result<(), ProtocolError> {
        let inner = self.0;
        if !children.is_empty()
            && let Some(entry) = inner.managed_entry(handle)
        {
            return inner.stop_children(&entry, children);
        }

        let entries = inner.entries_using(handle);
        if entries.is_empty() {
            return Err(ProtocolError::NotFound);
        }

        // Every entry must let go of `handle`, so a refusal does not skip the others.
        let mut result = Ok(());
        for entry in &entries {
            let released = inner.release_handle(entry, handle);
            if result.is_ok() {
                result = released;
            }
        }
        result
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::component::service::uefi_services::protocol::{MockProtocolServices, ProtocolPtr};
    use core::ffi::c_void;
    use std::sync::{Arc, Mutex};

    #[repr(C)]
    struct FakeProtocol {
        value: u32,
    }

    // SAFETY: test-only type whose GUID is only used with `FAKE_INSTANCE`, which has this layout.
    unsafe impl ProtocolInterface for FakeProtocol {
        const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("3f1f6bd6-7a52-4c1c-a2d4-4b0f8b5f1c11");
    }

    static FAKE_INSTANCE: FakeProtocol = FakeProtocol { value: 7 };

    const AGENT: usize = 0x100;
    const CONTROLLER: usize = 0x200;
    const CHILD: usize = 0x300;

    fn h(value: usize) -> Handle {
        Handle::from_raw(value as *mut c_void).unwrap()
    }

    fn fake_ptr() -> ProtocolPtr {
        ProtocolPtr::from_raw(&raw const FAKE_INSTANCE as *mut c_void).unwrap()
    }

    /// Records closed usages as (handle, controller) raw values.
    type Closed = Arc<Mutex<Vec<(usize, Option<usize>)>>>;

    fn mock_services(closed: &Closed) -> &'static dyn ProtocolServices {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().returning(|_, _, _, _| Ok(fake_ptr()));
        mock.expect_interface_on_handle().returning(|_, _| Ok(fake_ptr()));
        let closed = Arc::clone(closed);
        mock.expect_close_interface().returning(move |handle, _, _, controller| {
            closed.lock().unwrap().push((handle.as_raw() as usize, controller.map(|c| c.as_raw() as usize)));
            Ok(())
        });
        Box::leak(Box::new(mock))
    }

    #[derive(Default)]
    #[allow(clippy::struct_excessive_bools)]
    struct TestDriver {
        open_child: bool,
        child_handle: Option<usize>,
        release_dependencies: bool,
        fail_start: bool,
        fail_stop: bool,
        destroy_child_on_stop: Cell<Option<&'static Inner<TestDriver>>>,
        nested_stop_result: Cell<Option<Result<(), ProtocolError>>>,
    }

    impl ControllerDriver for TestDriver {
        type Protocol = FakeProtocol;
        type State = u32;

        fn supported(&self, context: &SupportedContext<'_, Self>) -> Result<(), ProtocolError> {
            if context.protocol().value == 7 { Ok(()) } else { Err(ProtocolError::Unsupported) }
        }

        fn start(&self, context: &StartContext<'_, Self>) -> Result<u32, ProtocolError> {
            if self.open_child {
                context.open_by_driver::<FakeProtocol>(h(self.child_handle.unwrap_or(CHILD)))?;
            }
            if self.fail_start {
                return Err(ProtocolError::Unsupported);
            }
            Ok(context.protocol().value)
        }

        fn stop(&self, _controller: Handle, _state: &mut u32) -> Result<(), ProtocolError> {
            if self.fail_stop {
                return Err(ProtocolError::AccessDenied);
            }
            if let Some(inner) = self.destroy_child_on_stop.get() {
                // Simulates the core calling Stop() for the child while this driver destroys it.
                self.nested_stop_result.set(Some(ManagedDriver(inner).stop(h(AGENT), h(CHILD), &[])));
            }
            Ok(())
        }

        fn dependency_lost(&self, _controller: Handle, _handle: Handle, state: &mut u32) -> DependencyAction {
            if self.release_dependencies {
                *state = 99;
                DependencyAction::Release
            } else {
                DependencyAction::Stop
            }
        }
    }

    fn setup(driver: TestDriver, closed: &Closed) -> (ManagedDriver<TestDriver>, ControllerDriverRef<TestDriver>) {
        let inner: &'static Inner<TestDriver> = Box::leak(Box::new(Inner {
            driver,
            services: mock_services(closed),
            agent: h(AGENT),
            controllers: RefCell::new(Vec::new()),
        }));
        (ManagedDriver(inner), ControllerDriverRef { inner })
    }

    #[test]
    fn test_controller_driver_supported_checks_protocol_and_closes_probe() {
        let closed = Closed::default();
        let (binding, _) = setup(TestDriver::default(), &closed);

        assert_eq!(binding.supported(h(AGENT), h(CONTROLLER), None), Ok(()));
        assert_eq!(*closed.lock().unwrap(), vec![(CONTROLLER, Some(CONTROLLER))]);
    }

    #[test]
    fn test_controller_driver_start_stores_state_that_can_be_modified() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver::default(), &closed);

        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(driver.controllers(), vec![h(CONTROLLER)]);
        assert!(driver.is_managing(h(CONTROLLER)));
        assert_eq!(driver.with_state(h(CONTROLLER), |state| *state += 1), Ok(()));
        assert_eq!(driver.state_ref(h(CONTROLLER)).with(|state| *state), Ok(8));
        assert_eq!(driver.with_controller(h(CONTROLLER), |state, protocol| *state + protocol.value), Ok(15));
        assert_eq!(driver.state_ref(h(CONTROLLER)).with_protocol(|_, protocol| protocol.value), Ok(7));
        assert!(closed.lock().unwrap().is_empty());
    }

    #[test]
    fn test_controller_driver_with_state_reports_not_managed_and_busy() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver::default(), &closed);

        assert_eq!(driver.with_state(h(CONTROLLER), |_| ()), Err(ControllerError::NotManaged));

        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();
        let nested = driver.with_state(h(CONTROLLER), |_| driver.with_state(h(CONTROLLER), |_| ())).unwrap();
        assert_eq!(nested, Err(ControllerError::Busy));
    }

    #[test]
    fn test_controller_driver_failed_start_closes_everything_opened() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver { open_child: true, fail_start: true, ..Default::default() }, &closed);

        assert_eq!(binding.start(h(AGENT), h(CONTROLLER), None), Err(ProtocolError::Unsupported));
        assert!(driver.controllers().is_empty());
        assert_eq!(*closed.lock().unwrap(), vec![(CHILD, Some(CONTROLLER)), (CONTROLLER, Some(CONTROLLER))]);
    }

    #[test]
    fn test_controller_driver_stop_closes_usages_and_drops_state() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver { open_child: true, ..Default::default() }, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Ok(()));
        assert!(!driver.is_managing(h(CONTROLLER)));
        assert_eq!(driver.with_state(h(CONTROLLER), |_| ()), Err(ControllerError::NotManaged));
        assert_eq!(*closed.lock().unwrap(), vec![(CHILD, Some(CONTROLLER)), (CONTROLLER, Some(CONTROLLER))]);
    }

    #[test]
    fn test_controller_driver_stop_on_opened_handle_stops_owning_controller() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver { open_child: true, ..Default::default() }, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CHILD), &[]), Ok(()));
        assert!(!driver.is_managing(h(CONTROLLER)));
        assert_eq!(closed.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_controller_driver_stop_releases_every_controller_using_handle() {
        const OTHER: usize = 0x400;
        let closed = Closed::default();
        let driver = TestDriver { open_child: true, child_handle: Some(CONTROLLER), ..Default::default() };
        let (binding, driver) = setup(driver, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();
        binding.start(h(AGENT), h(OTHER), None).unwrap();

        // CONTROLLER is one controller's own handle and the other's dependency, so both must let go of it.
        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Ok(()));
        assert!(driver.controllers().is_empty());
        assert!(closed.lock().unwrap().iter().all(|(_, controller)| controller.is_some()));
        assert_eq!(closed.lock().unwrap().len(), 4);
    }

    #[test]
    fn test_controller_driver_dependency_release_keeps_controller() {
        let closed = Closed::default();
        let driver = TestDriver { open_child: true, release_dependencies: true, ..Default::default() };
        let (binding, driver) = setup(driver, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CHILD), &[]), Ok(()));
        assert!(driver.is_managing(h(CONTROLLER)));
        assert_eq!(driver.with_state(h(CONTROLLER), |state| *state), Ok(99));
        assert_eq!(*closed.lock().unwrap(), vec![(CHILD, Some(CONTROLLER))]);

        // The released handle is no longer associated with the controller.
        assert_eq!(binding.stop(h(AGENT), h(CHILD), &[]), Err(ProtocolError::NotFound));
        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Ok(()));
        assert_eq!(*closed.lock().unwrap(), vec![(CHILD, Some(CONTROLLER)), (CONTROLLER, Some(CONTROLLER))]);
    }

    #[test]
    fn test_controller_driver_dependency_lost_refused_while_state_in_use() {
        let closed = Closed::default();
        let driver = TestDriver { open_child: true, release_dependencies: true, ..Default::default() };
        let (binding, driver) = setup(driver, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        let result = driver.with_state(h(CONTROLLER), |_| binding.stop(h(AGENT), h(CHILD), &[])).unwrap();
        assert_eq!(result, Err(ProtocolError::AccessDenied));
        assert!(closed.lock().unwrap().is_empty());
    }

    #[test]
    fn test_controller_driver_stop_unknown_handle_is_not_found() {
        let closed = Closed::default();
        let (binding, _) = setup(TestDriver::default(), &closed);

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Err(ProtocolError::NotFound));
    }

    #[test]
    fn test_controller_driver_stop_refused_by_driver_keeps_controller() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver { fail_stop: true, ..Default::default() }, &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Err(ProtocolError::AccessDenied));
        assert!(driver.is_managing(h(CONTROLLER)));
        assert!(closed.lock().unwrap().is_empty());
    }

    #[test]
    fn test_controller_driver_stop_refused_while_state_in_use() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver::default(), &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        let result = driver.with_state(h(CONTROLLER), |_| binding.stop(h(AGENT), h(CONTROLLER), &[])).unwrap();
        assert_eq!(result, Err(ProtocolError::AccessDenied));
        assert!(driver.is_managing(h(CONTROLLER)));
    }

    #[test]
    fn test_controller_driver_nested_stop_while_stopping_releases_only_that_handle() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver { open_child: true, ..Default::default() }, &closed);
        binding.0.driver.destroy_child_on_stop.set(Some(binding.0));
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Ok(()));
        assert_eq!(binding.0.driver.nested_stop_result.get(), Some(Ok(())));
        assert!(!driver.is_managing(h(CONTROLLER)));
        // The child is released by the nested Stop(), then the controller after stop() returns. Nothing twice.
        assert_eq!(*closed.lock().unwrap(), vec![(CHILD, Some(CONTROLLER)), (CONTROLLER, Some(CONTROLLER))]);
    }

    #[test]
    fn test_controller_driver_error_conversions() {
        assert_eq!(ProtocolError::from(ControllerError::NotManaged), ProtocolError::NotFound);
        assert_eq!(ProtocolError::from(ControllerError::Busy), ProtocolError::AccessDenied);
        assert_eq!(EfiError::from(ControllerError::NotManaged), EfiError::NotFound);
        assert_eq!(EfiError::from(ControllerError::Busy), EfiError::AccessDenied);
        assert!(!alloc::format!("{}", ControllerError::NotManaged).is_empty());
        assert!(!alloc::format!("{}", ControllerError::Busy).is_empty());
    }

    #[test]
    fn test_controller_driver_install_registers_agent_and_driver_binding() {
        let mut mock = MockProtocolServices::new();
        mock.expect_register_agent().times(1).returning(|| Ok(h(AGENT)));
        mock.expect_install_interface().times(1).returning(|handle, _, _| {
            assert_eq!(handle, Some(h(AGENT)));
            Ok(h(AGENT))
        });

        let driver = install_controller_driver(Service::mock(Box::new(mock)), TestDriver::default()).unwrap();
        assert_eq!(driver.agent(), h(AGENT));
        assert!(driver.controllers().is_empty());
    }

    const CHILD2: usize = 0x500;

    /// Like `mock_services`, but reports `AlreadyStarted` for a second `BY_DRIVER` open of the same controller,
    /// as the core does.
    fn mock_core_services(closed: &Closed) -> &'static dyn ProtocolServices {
        let opened: Arc<Mutex<Vec<usize>>> = Arc::default();
        let mut mock = MockProtocolServices::new();
        let opened_for_open = Arc::clone(&opened);
        mock.expect_open_interface().returning(move |handle, _, _, attributes| {
            if attributes == (OpenAttributes::ByDriver { controller: None }) {
                let mut opened = opened_for_open.lock().unwrap();
                if opened.contains(&(handle.as_raw() as usize)) {
                    return Err(ProtocolError::AlreadyStarted);
                }
                opened.push(handle.as_raw() as usize);
            }
            Ok(fake_ptr())
        });
        mock.expect_interface_on_handle().returning(|_, _| Ok(fake_ptr()));
        let closed = Arc::clone(closed);
        mock.expect_close_interface().returning(move |handle, _, _, controller| {
            let raw_handle = handle.as_raw() as usize;
            let raw_controller = controller.map(|c| c.as_raw() as usize);
            if raw_controller == Some(raw_handle) {
                opened.lock().unwrap().retain(|h| *h != raw_handle);
            }
            closed.lock().unwrap().push((raw_handle, raw_controller));
            Ok(())
        });
        Box::leak(Box::new(mock))
    }

    #[derive(Default)]
    struct BusDriver {
        fail_restart: bool,
        seen_managed: RefCell<Vec<bool>>,
        stopped_children: RefCell<Vec<Handle>>,
    }

    impl ControllerDriver for BusDriver {
        type Protocol = FakeProtocol;
        type State = u32;
        const RESTARTABLE: bool = true;

        fn supported(&self, context: &SupportedContext<'_, Self>) -> Result<(), ProtocolError> {
            self.seen_managed.borrow_mut().push(context.is_managed());
            Ok(())
        }

        fn start(&self, context: &StartContext<'_, Self>) -> Result<u32, ProtocolError> {
            context.open_for_child(h(CHILD))?;
            Ok(1)
        }

        fn restart(&self, context: &StartContext<'_, Self>, state: &mut u32) -> Result<(), ProtocolError> {
            context.open_for_child(h(CHILD2))?;
            if self.fail_restart {
                return Err(ProtocolError::DeviceError);
            }
            *state += 1;
            Ok(())
        }

        fn stop_children(
            &self,
            _controller: Handle,
            children: &[Handle],
            _state: &mut u32,
        ) -> Result<(), ProtocolError> {
            self.stopped_children.borrow_mut().extend_from_slice(children);
            Ok(())
        }
    }

    fn setup_with<D: ControllerDriver>(
        driver: D,
        services: &'static dyn ProtocolServices,
    ) -> (ManagedDriver<D>, ControllerDriverRef<D>) {
        let inner: &'static Inner<D> =
            Box::leak(Box::new(Inner { driver, services, agent: h(AGENT), controllers: RefCell::new(Vec::new()) }));
        (ManagedDriver(inner), ControllerDriverRef { inner })
    }

    #[test]
    fn test_controller_driver_supported_reports_already_started_for_managed_device_driver() {
        let closed = Closed::default();
        let (binding, _) = setup_with(TestDriver::default(), mock_core_services(&closed));
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.supported(h(AGENT), h(CONTROLLER), None), Err(ProtocolError::AlreadyStarted));
        assert_eq!(binding.start(h(AGENT), h(CONTROLLER), None), Err(ProtocolError::AlreadyStarted));
        // The existing BY_DRIVER usage is not closed.
        assert!(closed.lock().unwrap().is_empty());
    }

    #[test]
    fn test_controller_driver_restartable_bus_driver_restarts_and_stops_children() {
        let closed = Closed::default();
        let (binding, driver) = setup_with(BusDriver::default(), mock_core_services(&closed));

        assert_eq!(binding.supported(h(AGENT), h(CONTROLLER), None), Ok(()));
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();
        assert_eq!(binding.supported(h(AGENT), h(CONTROLLER), None), Ok(()));
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();
        assert_eq!(*binding.0.driver.seen_managed.borrow(), vec![false, true]);
        assert_eq!(driver.with_state(h(CONTROLLER), |state| *state), Ok(2));
        // Only the probe from the first supported() was closed.
        assert_eq!(*closed.lock().unwrap(), vec![(CONTROLLER, Some(CONTROLLER))]);
        closed.lock().unwrap().clear();

        // Stopping one child leaves the controller managed.
        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[h(CHILD)]), Ok(()));
        assert_eq!(*binding.0.driver.stopped_children.borrow(), vec![h(CHILD)]);
        assert!(driver.is_managing(h(CONTROLLER)));
        assert_eq!(*closed.lock().unwrap(), vec![(CONTROLLER, Some(CHILD))]);

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[]), Ok(()));
        assert!(!driver.is_managing(h(CONTROLLER)));
        assert_eq!(
            *closed.lock().unwrap(),
            vec![(CONTROLLER, Some(CHILD)), (CONTROLLER, Some(CHILD2)), (CONTROLLER, Some(CONTROLLER))]
        );
    }

    #[test]
    fn test_controller_driver_failed_restart_closes_only_what_it_opened() {
        let closed = Closed::default();
        let (binding, driver) =
            setup_with(BusDriver { fail_restart: true, ..Default::default() }, mock_core_services(&closed));
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.start(h(AGENT), h(CONTROLLER), None), Err(ProtocolError::DeviceError));
        assert!(driver.is_managing(h(CONTROLLER)));
        assert_eq!(driver.with_state(h(CONTROLLER), |state| *state), Ok(1));
        assert_eq!(*closed.lock().unwrap(), vec![(CONTROLLER, Some(CHILD2))]);
    }

    #[test]
    fn test_controller_driver_stop_children_unsupported_by_default_keeps_controller() {
        let closed = Closed::default();
        let (binding, driver) = setup(TestDriver::default(), &closed);
        binding.start(h(AGENT), h(CONTROLLER), None).unwrap();

        assert_eq!(binding.stop(h(AGENT), h(CONTROLLER), &[h(CHILD)]), Err(ProtocolError::Unsupported));
        assert!(driver.is_managing(h(CONTROLLER)));
        assert!(closed.lock().unwrap().is_empty());
    }
}
