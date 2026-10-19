//! Driver binding protocol production for Patina components.
//!
//! A component can implement [`DriverBinding`] and install it with
//! [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding). This
//! produces an `EFI_DRIVER_BINDING_PROTOCOL` that `connect_controller`/`disconnect_controller` can
//! call into.
//!
//! This allows a Patina component to participate in the UEFI Driver Model.
//!
//! By opening a protocol with `OpenAttributes::ByDriver` or `OpenAttributes::ByDriverExclusive` usage
//! recorded under a [`register_agent`](super::protocol::ProtocolServices::register_agent) handle, the
//! implementation can provide a `stop` function to call during controller disconnect or protocol uninstall.
//!
//! Returning `Err` from [`DriverBinding::stop`] blocks the binding release. Accepting the request requires that
//! `stop` release its resources. It receives its agent handle and must call `close_interface`/`close_protocol`.
//! Returning `Ok(())` without closing the usage, does not release it.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::vec::Vec;

use crate::base::error::EfiError;
use crate::base::guid::BinaryGuid;
use crate::base::protocol::ProtocolInterface;
use crate::standard::efi;
use crate::uefi::device_path::walker::BorrowedDevicePathWalker;

pub use super::handle::Handle;
use super::protocol::ProtocolError;

/// A component UEFI driver model implementation.
///
/// Implement this trait and pass it to
/// [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding) to
/// produce a driver binding protocol. `stop` and `start` are required. `supported` defaults to
/// reporting [`ProtocolError::Unsupported`] for every controller, for components that never expect
/// `connect_controller` to be called against them but want a `stop` implementation so a `ByDriver`
/// usage they hold can be released.
pub trait DriverBinding {
    /// The version number of the UEFI driver/component that implemented this trait. This field is used by the
    /// EFI boot service `connect_controller` to determine the order that the driver's `supported` service will be
    /// used when a controller needs to be started. `DriverBinding` implementations (instances) with higher
    /// `VERSION` values will be used before ones with lower `VERSION` values.
    ///
    /// The values of `0x0`-`0x0f` and `0xfffffff0`-`0xffffffff` are reserved for platform/OEM specific drivers.
    /// The values of `0x10`-`0xffffffef` are reserved for IHV-developed drivers.
    const VERSION: u32 = 0;

    /// Tests to see if this driver can manage `controller`.
    ///
    /// Implementations typically use the device path attached to the `controller` handle passed and/or the
    /// services from the bus I/O abstraction attached the `controller` handle to determine if this
    /// driver binding implementation supports `controller`.
    ///
    /// This function may be called many times during platform initialization. In order to reduce boot times,
    /// the tests performed by this function must be very small, and take as little time as possible to execute.
    ///
    /// This function must not change the state of any hardware devices, and this function must be aware that
    /// the device specified by `controller` may already be managed by the same component/driver or a different
    /// component/driver.
    ///
    /// A function implementation must match any manual memory allocations with deallocations to prevent memory
    /// leaks. In addition, any protocols opened must be closed. The implementation must also guarantee that the
    /// state of `controller` remains consistent and valid.
    ///
    /// ## Parameters
    ///
    /// - `agent` is this driver's own handle, the same one
    ///   [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding) returned. It is
    ///   the caller's identity to use for any `open_interface`/`open_protocol` calls this method
    ///   makes, similar to the way a C driver reads `This->DriverBindingHandle`.
    /// - `controller` is the handle of the controller to check.
    /// - `remaining_device_path` is the unconsumed portion of the device path passed to `connect_controller`,
    ///   if any, as an iterator over its nodes. It is only valid for the duration of the call.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if this driver can manage `controller`.
    /// - `Err(ProtocolError::AlreadyStarted)` if this driver is already managing any devices on `controller`,
    ///   unless this is a bus managing component/driver that may create additional child handles across multiple
    ///   invocations.
    /// - `Err(ProtocolError::AccessDenied)` if any interfaces on the device specified by `controller` are
    ///   already open for exclusive access by a different component/driver/application.
    /// - `Err(ProtocolError::Unsupported)` if this component/driver does not support `controller`.
    fn supported(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<BorrowedDevicePathWalker<'_>>,
    ) -> Result<(), ProtocolError> {
        let _ = (agent, controller, remaining_device_path);
        Err(ProtocolError::Unsupported)
    }

    /// Starts managing a device or bus `controller`.
    ///
    /// This implementation must start the device or bus represented by `controller`.
    ///
    /// Any resources manually allocated in `start` must be released in `stop`.
    ///
    /// This function is expected to only be called from `connect_controller`.
    ///
    /// ## Parameters
    ///
    /// - `agent` is this driver's own handle, the same one
    ///   [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding) returned. It is
    ///   the caller's identity to use for any `open_interface`/`open_protocol` calls this method
    ///   makes, similar to the way a C driver reads `This->DriverBindingHandle`.
    /// - `controller` is the handle of the controller to start managing.
    /// - `remaining_device_path` is the unconsumed portion of the device path passed to `connect_controller`,
    ///   if any, as an iterator over its nodes. It is only valid for the duration of the call.
    ///   - If this is a device (not bus) implementation, this parameter is ignored.
    ///   - If this is a bus implementation and `remaining_device_path` is `None`, then the bus implementation
    ///     should attempt to manage all child devices.
    ///   - If this is a bus implementation and `remaining_device_path` is `Some`, but begins with an `End` node,
    ///     then the bus implementation must not enumerate any of the children of `controller` nor create any
    ///     new child device handles.
    ///   - Otherwise, if this is a bus implementation and `remaining_device_path` is `Some`, then the bus
    ///     implementation should attempt to manage only the child device specified by the remaining device path.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if the driver successfully started managing the controller.
    /// - `Ok(())` if the device could not be started because the device needs to be configured by the user or requires
    ///   a repair operation and repair services are available.
    /// - `Err(ProtocolError::DeviceError)` if the component/driver encountered an error while starting the controller
    ///   or if the `controller` is not supported (due to a more extensive check than `supported`).
    /// - `Err(ProtocolError::OutOfResources)` if the driver cannot allocate the necessary resources to start the
    ///   controller.
    fn start(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<BorrowedDevicePathWalker<'_>>,
    ) -> Result<(), ProtocolError>;

    /// Stops a device or bus controller.
    ///
    /// The implementation will perform different operations depending on the `children` parameter.
    /// If non-zero children are provided, the implementation represents a bus driver and it should
    /// stop managing only the specified child devices.
    ///
    /// If no children are provided, the implementation represents a device driver or a bus driver
    /// that should stop managing the controller.
    ///
    /// In all cases, this function is required to undo what was performed in the corresponding `start`
    /// call.
    ///
    /// Note: Simply returning `Ok(())` does not release any protocol usage. The caller is responsible
    /// for closing any protocols opened by the driver.
    ///
    /// This function is expected to only be called from `disconnect_controller`.
    ///
    /// ## Parameters
    ///
    /// - `agent` is the handle of the driver binding protocol instance that is requesting the stop.
    /// - `controller` is the handle of the controller to stop managing.
    /// - `children` is a slice of child handles, that, if provided, are requested to be freed.
    ///
    /// ## Returns
    ///
    /// - `Ok(())` if the driver successfully stopped managing the controller.
    /// - `Err(ProtocolError::DeviceError)` if the component/driver encountered an error while stopping the controller.
    fn stop(&self, agent: Handle, controller: Handle, children: &[Handle]) -> Result<(), ProtocolError>;
}

/// Wraps a component's [`DriverBinding`] alongside the raw protocol struct installed for it.
///
/// `protocol` is the first field so a pointer to this struct can be safely reinterpreted as a
/// pointer to `efi::protocols::driver_binding::Protocol`.
#[repr(C)]
pub(super) struct DriverBindingHolder<B: DriverBinding> {
    pub(super) protocol: efi::protocols::driver_binding::Protocol,
    pub(super) binding: B,
}

// SAFETY: `efi::protocols::driver_binding::Protocol` already has a `ProtocolInterface` impl
// (`crate::base::protocol`'s `impl_r_efi_protocol!(driver_binding);`), confirming its layout
// matches the UEFI driver binding protocol GUID. `DriverBindingHolder` is `#[repr(C)]` with
// `protocol` as its first field, so a pointer to a `DriverBindingHolder<B>` is also a valid pointer
// to an `efi::protocols::driver_binding::Protocol`.
unsafe impl<B: DriverBinding> ProtocolInterface for DriverBindingHolder<B> {
    const PROTOCOL_GUID: BinaryGuid = <efi::protocols::driver_binding::Protocol as ProtocolInterface>::PROTOCOL_GUID;
}

/// Converts a raw device path pointer from a driver binding "trampoline" into a safe device path walker.
///
/// # Safety
///
/// `ptr` must be null or point to a valid, properly terminated device path that remains valid for
/// the duration of the call, per the UEFI `connect_controller` contract for `remaining_device_path`.
unsafe fn device_path_from_raw<'a>(
    ptr: *mut efi::protocols::device_path::Protocol,
) -> Option<BorrowedDevicePathWalker<'a>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: forwarded from the caller's contract on this function.
    Some(unsafe { BorrowedDevicePathWalker::new(ptr) })
}

pub(super) extern "efiapi" fn supported_trampoline<B: DriverBinding>(
    this: *mut efi::protocols::driver_binding::Protocol,
    controller_handle: efi::Handle,
    remaining_device_path: *mut efi::protocols::device_path::Protocol,
) -> efi::Status {
    // SAFETY: `this` always points to the `protocol` field of a `DriverBindingHolder<B>` built by
    // `install_driver_binding`, which is that struct's first field under `#[repr(C)]`.
    let Some(holder) = (unsafe { (this as *const DriverBindingHolder<B>).as_ref() }) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(agent) = Handle::from_raw(holder.protocol.driver_binding_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(controller) = Handle::from_raw(controller_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    // SAFETY: the UEFI caller guarantees remaining_device_path is null or a valid device path for
    // the duration of this call, per `connect_controller`'s contract.
    let remaining_device_path = unsafe { device_path_from_raw(remaining_device_path) };
    match holder.binding.supported(agent, controller, remaining_device_path) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => EfiError::from(err).into(),
    }
}

pub(super) extern "efiapi" fn start_trampoline<B: DriverBinding>(
    this: *mut efi::protocols::driver_binding::Protocol,
    controller_handle: efi::Handle,
    remaining_device_path: *mut efi::protocols::device_path::Protocol,
) -> efi::Status {
    // SAFETY: as in `supported_trampoline`.
    let Some(holder) = (unsafe { (this as *const DriverBindingHolder<B>).as_ref() }) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(agent) = Handle::from_raw(holder.protocol.driver_binding_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(controller) = Handle::from_raw(controller_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    // SAFETY: as in `supported_trampoline`.
    let remaining_device_path = unsafe { device_path_from_raw(remaining_device_path) };
    match holder.binding.start(agent, controller, remaining_device_path) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => EfiError::from(err).into(),
    }
}

pub(super) extern "efiapi" fn stop_trampoline<B: DriverBinding>(
    this: *mut efi::protocols::driver_binding::Protocol,
    controller_handle: efi::Handle,
    number_of_children: usize,
    child_handle_buffer: *mut efi::Handle,
) -> efi::Status {
    // SAFETY: as in `supported_trampoline`.
    let Some(holder) = (unsafe { (this as *const DriverBindingHolder<B>).as_ref() }) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(agent) = Handle::from_raw(holder.protocol.driver_binding_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let Some(controller) = Handle::from_raw(controller_handle) else {
        return efi::Status::INVALID_PARAMETER;
    };
    let children_raw: &[efi::Handle] = if number_of_children == 0 || child_handle_buffer.is_null() {
        &[]
    } else {
        // SAFETY: the UEFI caller guarantees child_handle_buffer points to number_of_children
        // valid handles when number_of_children is non-zero, per the `stop` contract.
        unsafe { core::slice::from_raw_parts(child_handle_buffer, number_of_children) }
    };
    let children: Vec<Handle> = children_raw.iter().copied().filter_map(Handle::from_raw).collect();
    match holder.binding.stop(agent, controller, &children) {
        Ok(()) => efi::Status::SUCCESS,
        Err(err) => EfiError::from(err).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use core::ptr::NonNull;

    fn fake_handle() -> Handle {
        Handle::from_raw(NonNull::<c_void>::dangling().as_ptr()).unwrap()
    }

    struct RecordingBinding {
        stop_result: Result<(), ProtocolError>,
    }

    impl DriverBinding for RecordingBinding {
        fn supported(
            &self,
            _agent: Handle,
            _controller: Handle,
            _remaining: Option<BorrowedDevicePathWalker>,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }

        fn start(
            &self,
            _agent: Handle,
            _controller: Handle,
            _remaining: Option<BorrowedDevicePathWalker>,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }

        fn stop(&self, _agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
            self.stop_result
        }
    }

    fn holder_for(binding: RecordingBinding) -> DriverBindingHolder<RecordingBinding> {
        let handle = fake_handle().as_raw();
        DriverBindingHolder {
            protocol: efi::protocols::driver_binding::Protocol {
                version: RecordingBinding::VERSION,
                supported: supported_trampoline::<RecordingBinding>,
                start: start_trampoline::<RecordingBinding>,
                stop: stop_trampoline::<RecordingBinding>,
                driver_binding_handle: handle,
                image_handle: handle,
            },
            binding,
        }
    }

    fn this_ptr<B: DriverBinding>(holder: &DriverBindingHolder<B>) -> *mut efi::protocols::driver_binding::Protocol {
        core::ptr::from_ref(holder).cast::<efi::protocols::driver_binding::Protocol>().cast_mut()
    }

    #[test]
    fn test_driver_binding_default_supported_is_unsupported() {
        struct MinimalBinding;
        impl DriverBinding for MinimalBinding {
            fn start(
                &self,
                _agent: Handle,
                _controller: Handle,
                _remaining: Option<BorrowedDevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                Ok(())
            }

            fn stop(&self, _agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
                Ok(())
            }
        }

        let binding = MinimalBinding;
        assert_eq!(binding.supported(fake_handle(), fake_handle(), None), Err(ProtocolError::Unsupported));
    }

    #[test]
    fn test_driver_binding_default_supported_trampoline_returns_unsupported_status() {
        struct MinimalBinding;
        impl DriverBinding for MinimalBinding {
            fn start(
                &self,
                _agent: Handle,
                _controller: Handle,
                _remaining: Option<BorrowedDevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                Ok(())
            }

            fn stop(&self, _agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
                Ok(())
            }
        }

        let handle = fake_handle().as_raw();
        let holder = DriverBindingHolder {
            protocol: efi::protocols::driver_binding::Protocol {
                version: MinimalBinding::VERSION,
                supported: supported_trampoline::<MinimalBinding>,
                start: start_trampoline::<MinimalBinding>,
                stop: stop_trampoline::<MinimalBinding>,
                driver_binding_handle: handle,
                image_handle: handle,
            },
            binding: MinimalBinding,
        };

        assert_eq!(
            supported_trampoline::<MinimalBinding>(this_ptr(&holder), handle, core::ptr::null_mut()),
            efi::Status::UNSUPPORTED
        );
    }

    #[test]
    fn test_driver_binding_holder_layout_matches_protocol_first_field() {
        let holder = holder_for(RecordingBinding { stop_result: Ok(()) });
        let holder_ptr = &raw const holder;
        let protocol_ptr = &raw const holder.protocol;
        assert_eq!(holder_ptr as usize, protocol_ptr as usize, "protocol must be the first field");
    }

    #[test]
    fn test_driver_binding_trampolines_null_controller_is_invalid_parameter() {
        let holder = holder_for(RecordingBinding { stop_result: Ok(()) });
        let this = this_ptr(&holder);

        assert_eq!(
            supported_trampoline::<RecordingBinding>(this, core::ptr::null_mut(), core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            start_trampoline::<RecordingBinding>(this, core::ptr::null_mut(), core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
        assert_eq!(
            stop_trampoline::<RecordingBinding>(this, core::ptr::null_mut(), 0, core::ptr::null_mut()),
            efi::Status::INVALID_PARAMETER
        );
    }

    #[test]
    fn test_driver_binding_stop_trampoline_propagates_ok() {
        let holder = holder_for(RecordingBinding { stop_result: Ok(()) });
        let this = this_ptr(&holder);
        let controller = fake_handle().as_raw();

        assert_eq!(
            stop_trampoline::<RecordingBinding>(this, controller, 0, core::ptr::null_mut()),
            efi::Status::SUCCESS
        );
    }

    #[test]
    fn test_driver_binding_stop_trampoline_propagates_err() {
        let holder = holder_for(RecordingBinding { stop_result: Err(ProtocolError::AccessDenied) });
        let this = this_ptr(&holder);
        let controller = fake_handle().as_raw();

        assert_eq!(
            stop_trampoline::<RecordingBinding>(this, controller, 0, core::ptr::null_mut()),
            efi::Status::ACCESS_DENIED
        );
    }

    #[test]
    fn test_driver_binding_stop_trampoline_filters_children() {
        struct ChildCountingBinding {
            children_seen: core::cell::Cell<usize>,
        }
        impl DriverBinding for ChildCountingBinding {
            fn start(
                &self,
                _agent: Handle,
                _controller: Handle,
                _remaining: Option<BorrowedDevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                Ok(())
            }

            fn stop(&self, _agent: Handle, _controller: Handle, children: &[Handle]) -> Result<(), ProtocolError> {
                self.children_seen.set(children.len());
                Ok(())
            }
        }

        let handle = fake_handle().as_raw();
        let holder = DriverBindingHolder {
            protocol: efi::protocols::driver_binding::Protocol {
                version: 0,
                supported: supported_trampoline::<ChildCountingBinding>,
                start: start_trampoline::<ChildCountingBinding>,
                stop: stop_trampoline::<ChildCountingBinding>,
                driver_binding_handle: handle,
                image_handle: handle,
            },
            binding: ChildCountingBinding { children_seen: core::cell::Cell::new(0) },
        };
        let this = this_ptr(&holder);
        let controller = fake_handle().as_raw();
        let mut children = [fake_handle().as_raw(), fake_handle().as_raw()];

        assert_eq!(
            stop_trampoline::<ChildCountingBinding>(this, controller, children.len(), children.as_mut_ptr()),
            efi::Status::SUCCESS
        );
        assert_eq!(holder.binding.children_seen.get(), 2);
    }

    #[test]
    fn test_driver_binding_trampolines_pass_driver_binding_handle_as_agent() {
        struct AgentRecordingBinding {
            seen_in_supported: core::cell::Cell<Option<Handle>>,
            seen_in_start: core::cell::Cell<Option<Handle>>,
            seen_in_stop: core::cell::Cell<Option<Handle>>,
        }
        impl DriverBinding for AgentRecordingBinding {
            fn supported(
                &self,
                agent: Handle,
                _controller: Handle,
                _remaining: Option<BorrowedDevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                self.seen_in_supported.set(Some(agent));
                Ok(())
            }

            fn start(
                &self,
                agent: Handle,
                _controller: Handle,
                _remaining: Option<BorrowedDevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                self.seen_in_start.set(Some(agent));
                Ok(())
            }

            fn stop(&self, agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
                self.seen_in_stop.set(Some(agent));
                Ok(())
            }
        }

        let agent_handle = fake_handle().as_raw();
        let holder = DriverBindingHolder {
            protocol: efi::protocols::driver_binding::Protocol {
                version: 0,
                supported: supported_trampoline::<AgentRecordingBinding>,
                start: start_trampoline::<AgentRecordingBinding>,
                stop: stop_trampoline::<AgentRecordingBinding>,
                driver_binding_handle: agent_handle,
                image_handle: agent_handle,
            },
            binding: AgentRecordingBinding {
                seen_in_supported: core::cell::Cell::new(None),
                seen_in_start: core::cell::Cell::new(None),
                seen_in_stop: core::cell::Cell::new(None),
            },
        };
        let this = this_ptr(&holder);
        let controller = fake_handle().as_raw();

        supported_trampoline::<AgentRecordingBinding>(this, controller, core::ptr::null_mut());
        start_trampoline::<AgentRecordingBinding>(this, controller, core::ptr::null_mut());
        stop_trampoline::<AgentRecordingBinding>(this, controller, 0, core::ptr::null_mut());

        let expected = Handle::from_raw(agent_handle);
        assert_eq!(holder.binding.seen_in_supported.get(), expected);
        assert_eq!(holder.binding.seen_in_start.get(), expected);
        assert_eq!(holder.binding.seen_in_stop.get(), expected);
    }
}
