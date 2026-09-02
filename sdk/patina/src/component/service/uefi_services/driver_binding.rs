//! Driver binding protocol production for Patina components.
//!
//! A component can implement [`DriverBinding`] and install it with
//! [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding). This
//! produces a `EFI_DRIVER_BINDING_PROTOCOL` that `ConnectController()`/`DisconnectController()` can
//! call into.
//!
//! This allows a Patina component to participate in the UEFI Driver Model. `OpenAttributes::ByDriver`
//! or `OpenAttributes::ByDriverExclusive` usage recorded under a
//! [`register_agent`](super::protocol::ProtocolServices::register_agent) handle, can provide a `Stop()`
//! function to call during controller disconnect or protocol uninstall.
//!
//! Returning `Err` from [`DriverBinding::stop`] blocks the release. Accepting the request requires that
//! `stop` release its resources. It receives its own agent handle and must call `close_interface`/`close_protocol`.
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
use crate::uefi::device_path::walker::DevicePathWalker;

pub use super::handle::Handle;
use super::protocol::ProtocolError;

/// A component's UEFI driver model implementation.
///
/// Implement this trait and pass it to
/// [`install_driver_binding`](super::protocol::ProtocolServicesExt::install_driver_binding) to
/// produce a driver binding protocol. `supported` and `start` default to a no-op implementation
/// for components that only care about providing `Stop()`. For example a component that never
/// expects `ConnectController()` to be called against it but wants a `Stop()` implementation so
/// a `ByDriver` usage it holds can be released.
pub trait DriverBinding {
    /// The driver binding protocol's version, used by `ConnectController()` to order candidates.
    const VERSION: u32 = 0;

    /// Returns `Ok(())` if this driver can manage `controller`.
    ///
    /// `agent` is this driver's own handle, the same one [`install_driver_binding`] returned. It is
    /// the caller's own identity to use for any `open_interface`/`open_protocol` calls this method
    /// makes, similar to the way a C driver reads `This->DriverBindingHandle`.
    ///
    /// `controller` is the handle of the controller to check.
    ///
    /// `remaining_device_path` is the unconsumed portion of the device path passed to
    /// `ConnectController()`, if any, as an iterator over its nodes.
    ///
    /// [`install_driver_binding`]: super::protocol::ProtocolServicesExt::install_driver_binding
    fn supported(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<DevicePathWalker>,
    ) -> Result<(), ProtocolError> {
        let _ = (agent, controller, remaining_device_path);
        Err(ProtocolError::NotFound)
    }

    /// Starts managing `controller`.
    ///
    /// `controller` is the handle of the controller to start managing.
    ///
    /// A driver that opens the controller's protocol itself (rather than relying only on the
    /// `ByDriver` open made before connecting) should do so here, using `agent` as its own agent
    /// handle. See [`Self::supported`] for `agent` and `remaining_device_path`.
    fn start(
        &self,
        agent: Handle,
        controller: Handle,
        remaining_device_path: Option<DevicePathWalker>,
    ) -> Result<(), ProtocolError> {
        let _ = (agent, controller, remaining_device_path);
        Ok(())
    }

    /// Stops managing `controller`, releasing `children` first if any are given.
    ///
    /// The controller handle is the handle of the controller to stop managing.
    ///
    /// `children` is a slice of child handles, that, if provided, are requested to be freed.
    ///
    /// Returning `Err` refuses the request. This is what a caller of `DisconnectController()`, or
    /// an uninstall that needs this driver to let go of `controller`, sees as failure.
    ///
    /// Releasing `controller` here means calling `close_interface`/`close_protocol` for whatever
    /// this driver opened while managing it, using `agent` as its own agent handle, similar to the
    /// way a C driver's `Stop()` calls `CloseProtocol()` with `This->DriverBindingHandle`. Note that
    /// returning `Ok(())` alone does not release a protocol usage. The protocol database only clears a
    /// `ByDriver`/`ByDriverExclusive` usage when it is explicitly closed.
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
    const PROTOCOL_GUID: BinaryGuid = BinaryGuid(efi::protocols::driver_binding::PROTOCOL_GUID);
}

/// Converts a raw device path pointer from a driver binding "trampoline" into a safe device path walker.
///
/// # Safety
///
/// `ptr` must be null or point to a valid, properly terminated device path that remains valid for
/// the duration of the call, per the UEFI `ConnectController()` contract for `RemainingDevicePath`.
unsafe fn device_path_from_raw(ptr: *mut efi::protocols::device_path::Protocol) -> Option<DevicePathWalker> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: forwarded from the caller's contract on this function.
    Some(unsafe { DevicePathWalker::new(ptr) })
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
    // the duration of this call, per `ConnectController()`'s contract.
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
        // valid handles when number_of_children is non-zero, per the `Stop()` contract.
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
            _remaining: Option<DevicePathWalker>,
        ) -> Result<(), ProtocolError> {
            Ok(())
        }

        fn start(
            &self,
            _agent: Handle,
            _controller: Handle,
            _remaining: Option<DevicePathWalker>,
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

    #[test]
    fn test_driver_binding_default_supported_and_start() {
        struct MinimalBinding;
        impl DriverBinding for MinimalBinding {
            fn stop(&self, _agent: Handle, _controller: Handle, _children: &[Handle]) -> Result<(), ProtocolError> {
                Ok(())
            }
        }

        let binding = MinimalBinding;
        assert_eq!(binding.supported(fake_handle(), fake_handle(), None), Err(ProtocolError::NotFound));
        assert_eq!(binding.start(fake_handle(), fake_handle(), None), Ok(()));
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
        let this = (&raw const holder.protocol).cast_mut();

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
        let this = (&raw const holder.protocol).cast_mut();
        let controller = fake_handle().as_raw();

        assert_eq!(
            stop_trampoline::<RecordingBinding>(this, controller, 0, core::ptr::null_mut()),
            efi::Status::SUCCESS
        );
    }

    #[test]
    fn test_driver_binding_stop_trampoline_propagates_err() {
        let holder = holder_for(RecordingBinding { stop_result: Err(ProtocolError::AccessDenied) });
        let this = (&raw const holder.protocol).cast_mut();
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
        let this = (&raw const holder.protocol).cast_mut();
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
                _remaining: Option<DevicePathWalker>,
            ) -> Result<(), ProtocolError> {
                self.seen_in_supported.set(Some(agent));
                Ok(())
            }

            fn start(
                &self,
                agent: Handle,
                _controller: Handle,
                _remaining: Option<DevicePathWalker>,
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
        let this = (&raw const holder.protocol).cast_mut();
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
