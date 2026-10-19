//! A module for the [Protocol] param type.
//!
//! [Protocol] lets a component depend on a specific UEFI protocol interface being installed,
//! without handling [`ProtocolServices`] directly. The component is not dispatched until both the
//! [`ProtocolServices`] service and an interface for `P` are available, and dereferencing the
//! parameter gives direct access to the interface.
//!
//! ## Example
//!
//! ```rust
//! use patina::{
//!     error::Result,
//!     component::{component, protocol::Protocol},
//!     protocol::ProtocolInterface,
//!     BinaryGuid,
//! };
//!
//! #[repr(C)]
//! struct MyProtocol {
//!     value: u32,
//! }
//!
//! unsafe impl ProtocolInterface for MyProtocol {
//!     const PROTOCOL_GUID: BinaryGuid = BinaryGuid::ZERO;
//! }
//!
//! /// A component that will only run once `MyProtocol` is installed.
//! struct MyComponent;
//!
//! #[component]
//! impl MyComponent {
//!     fn entry_point(self, protocol: Protocol<MyProtocol>) -> Result<()> {
//!         let _value = protocol.value;
//!         Ok(())
//!     }
//! }
//!
//! /// A component that will always run, but the protocol will be `None` if not installed.
//! struct MyOtherComponent;
//!
//! #[component]
//! impl MyOtherComponent {
//!     fn entry_point(self, protocol: Option<Protocol<MyProtocol>>) -> Result<()> {
//!         if let Some(protocol) = protocol {
//!             let _value = protocol.value;
//!         }
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use core::{fmt::Debug, marker::PhantomData, ops::Deref};

use alloc::borrow::Cow;

use crate::base::protocol::ProtocolInterface;

use super::{
    metadata::MetaData,
    params::Param,
    service::{
        Service,
        uefi_services::protocol::{ProtocolServices, ProtocolServicesExt},
    },
    storage::{Storage, UnsafeStorageCell},
};

/// A UEFI protocol interface provided to a component through the protocol database.
///
/// A component that requests `Protocol<P>` is not dispatched until both the [`ProtocolServices`]
/// service and an interface for `P` are available. The protocol service is retained by this
/// parameter and used to resolve the interface each time it is dereferenced.
///
/// Wrap this parameter in [`Option`] when the component should run even if the protocol is not
/// installed.
pub struct Protocol<P: ProtocolInterface + 'static> {
    protocols: Service<dyn ProtocolServices>,
    _marker: PhantomData<fn() -> P>,
}

impl<P: ProtocolInterface + 'static> Protocol<P> {
    /// Creates a `Protocol<P>` for testing, backed by a mock [`ProtocolServices`] that resolves
    /// `P::PROTOCOL_GUID` to `interface`.
    ///
    /// This function is intended for testing purposes only.
    ///
    /// ## Example
    /// ``` rust
    /// use patina::{component::protocol::Protocol, protocol::ProtocolInterface, BinaryGuid};
    ///
    /// #[repr(C)]
    /// struct MyProtocol {
    ///     value: u32,
    /// }
    ///
    /// unsafe impl ProtocolInterface for MyProtocol {
    ///     const PROTOCOL_GUID: BinaryGuid = BinaryGuid::ZERO;
    /// }
    ///
    /// static MY_PROTOCOL: MyProtocol = MyProtocol { value: 42 };
    ///
    /// fn my_component_to_test(protocol: Protocol<MyProtocol>) {
    ///     assert_eq!(protocol.value, 42);
    /// }
    ///
    /// #[test]
    /// fn test_component() {
    ///     let protocol = Protocol::mock(&MY_PROTOCOL);
    ///     my_component_to_test(protocol);
    /// }
    /// ```
    #[cfg(any(test, feature = "mockall"))]
    #[allow(clippy::test_attr_in_doctest)]
    pub fn mock(interface: &'static P) -> Self {
        use super::service::uefi_services::protocol::{MockProtocolServices, ProtocolError, ProtocolPtr};

        let mut mock = MockProtocolServices::new();
        mock.expect_locate_interface().returning_st(move |guid| {
            if guid == P::PROTOCOL_GUID {
                Ok(ProtocolPtr::from_raw(core::ptr::from_ref(interface).cast_mut().cast())
                    .expect("the &'static reference is not null"))
            } else {
                Err(ProtocolError::NotFound)
            }
        });
        Self { protocols: Service::mock(alloc::boxed::Box::new(mock)), _marker: PhantomData }
    }
}

impl<P: ProtocolInterface + 'static> Deref for Protocol<P> {
    type Target = P;

    fn deref(&self) -> &Self::Target {
        self.protocols.locate_protocol::<P>().expect("the protocol is available")
    }
}

impl<P: ProtocolInterface + 'static> Copy for Protocol<P> {}

impl<P: ProtocolInterface + 'static> Clone for Protocol<P> {
    #[cfg_attr(coverage, coverage(off))]
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ProtocolInterface + 'static> Debug for Protocol<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Note: The wrapped interface is resolved lazily, so only the type name can be shown, not its contents.
        f.debug_struct("Protocol").field("type", &super::type_name::normalized::<P>()).finish()
    }
}

// SAFETY: Protocol<P> delegates its storage access to Service<dyn ProtocolServices>. The service
// is accessed immutably, and availability of both the service and P is checked by validate().
unsafe impl<P: ProtocolInterface + 'static> Param for Protocol<P> {
    type State = <Service<dyn ProtocolServices> as Param>::State;
    type Item<'storage, 'state> = Self;

    unsafe fn get_param<'storage, 'state>(
        state: &'state Self::State,
        storage: UnsafeStorageCell<'storage>,
    ) -> Self::Item<'storage, 'state> {
        // SAFETY: validate() first delegates to the Service<dyn ProtocolServices> Param
        // implementation, so the service is present in storage under this state.
        let protocols = unsafe { <Service<dyn ProtocolServices> as Param>::get_param(state, storage) };
        Self { protocols, _marker: PhantomData }
    }

    fn validate(state: &Self::State, storage: UnsafeStorageCell) -> bool {
        if !<Service<dyn ProtocolServices> as Param>::validate(state, storage) {
            return false;
        }

        // SAFETY: The service Param validated successfully immediately above, so retrieving its
        // immutable, static service reference is valid.
        let protocols = unsafe { <Service<dyn ProtocolServices> as Param>::get_param(state, storage) };
        protocols.locate_interface(P::PROTOCOL_GUID).is_ok()
    }

    fn init_state(storage: &mut Storage, meta: &mut MetaData) -> Result<Self::State, Cow<'static, str>> {
        <Service<dyn ProtocolServices> as Param>::init_state(storage, meta)
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate as patina;
    use crate::BinaryGuid;
    use crate::component::service::{
        IntoService,
        uefi_services::protocol::{
            Handle, MockProtocolServices, NotifyCallback, NotifyRegistration, ProtocolError, ProtocolPtr, Tpl,
        },
    };
    use alloc::{boxed::Box, vec::Vec};

    #[repr(C)]
    struct FakeProtocol {
        value: u32,
    }

    // SAFETY: This test-only type declares a fixed GUID used consistently for both the mocked
    // locate path and the value read back through Deref.
    unsafe impl ProtocolInterface for FakeProtocol {
        const PROTOCOL_GUID: BinaryGuid = BinaryGuid::from_string("2d33f3f4-5b1e-4d3e-8b8a-3f6f8b2c9a10");
    }

    static FAKE_INSTANCE: FakeProtocol = FakeProtocol { value: 42 };

    #[test]
    fn test_protocol_deref_locates_interface() {
        let mut mock = MockProtocolServices::new();
        mock.expect_locate_interface().times(1).returning(|guid| {
            assert_eq!(guid, FakeProtocol::PROTOCOL_GUID);
            Ok(ProtocolPtr::from_raw(&raw const FAKE_INSTANCE as *mut core::ffi::c_void).unwrap())
        });

        let protocol = Protocol::<FakeProtocol> { protocols: Service::mock(Box::new(mock)), _marker: PhantomData };
        assert_eq!(
            alloc::format!("{protocol:?}"),
            alloc::format!("Protocol {{ type: \"{}::FakeProtocol\" }}", module_path!())
        );
        assert_eq!(protocol.value, 42);
    }

    #[test]
    fn test_protocol_mock_helper() {
        let protocol = Protocol::mock(&FAKE_INSTANCE);
        assert_eq!(protocol.value, 42);
    }

    /// A minimal, [`ProtocolServices`] used to exercise the [Param] state/`validate`/`get_param`
    /// calls through [Storage], which requires a type implementing
    /// [`IntoService`](crate::component::service::IntoService) instead of a `Service::mock`.
    #[derive(IntoService)]
    #[service(dyn ProtocolServices)]
    struct FakeProtocolServices {
        guid: BinaryGuid,
        installed: bool,
    }

    impl ProtocolServices for FakeProtocolServices {
        fn install_interface(
            &self,
            _handle: Option<Handle>,
            _protocol: BinaryGuid,
            _interface: ProtocolPtr,
        ) -> Result<Handle, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn uninstall_interface(
            &self,
            _handle: Handle,
            _protocol: BinaryGuid,
            _interface: ProtocolPtr,
        ) -> Result<(), ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn install_marker(&self, _handle: Option<Handle>, _protocol: BinaryGuid) -> Result<Handle, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn uninstall_marker(&self, _handle: Handle, _protocol: BinaryGuid) -> Result<(), ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn locate_interface(&self, protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError> {
            if self.installed && protocol == self.guid {
                Ok(ProtocolPtr::from_raw(&raw const FAKE_INSTANCE as *mut core::ffi::c_void).unwrap())
            } else {
                Err(ProtocolError::NotFound)
            }
        }

        fn locate_handles(&self, _protocol: BinaryGuid) -> Result<Vec<Handle>, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        #[cfg(feature = "unstable-device-path")]
        fn locate_device_path(
            &self,
            _protocol: BinaryGuid,
            _path: &crate::uefi::device_path::paths::DevicePath,
        ) -> Result<(Handle, crate::uefi::device_path::paths::DevicePathBuf), ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn interface_on_handle(&self, _handle: Handle, _protocol: BinaryGuid) -> Result<ProtocolPtr, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn open_interface(
            &self,
            _handle: Handle,
            _protocol: BinaryGuid,
            _agent: Handle,
            _attributes: crate::component::service::uefi_services::protocol::OpenAttributes,
        ) -> Result<ProtocolPtr, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn close_interface(
            &self,
            _handle: Handle,
            _protocol: BinaryGuid,
            _agent: Handle,
            _controller: Option<Handle>,
        ) -> Result<(), ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn register_agent(&self) -> Result<Handle, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn register_install_notify(
            &self,
            _protocol: BinaryGuid,
            _notify_tpl: Tpl,
            _callback: NotifyCallback,
        ) -> Result<NotifyRegistration, ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }

        fn cancel_install_notify(&self, _registration: NotifyRegistration) -> Result<(), ProtocolError> {
            unimplemented!("not used by Protocol<P>")
        }
    }

    #[test]
    fn test_protocol_validate_false_when_service_missing() {
        let mut storage = Storage::default();
        let mut meta = MetaData::new::<i32>();

        let id = <Protocol<FakeProtocol> as Param>::init_state(&mut storage, &mut meta).unwrap();
        assert!(!<Protocol<FakeProtocol> as Param>::validate(&id, (&storage).into()));
    }

    #[test]
    fn test_protocol_validate_false_when_protocol_not_installed() {
        let mut storage = Storage::default();
        let mut meta = MetaData::new::<i32>();

        let id = <Protocol<FakeProtocol> as Param>::init_state(&mut storage, &mut meta).unwrap();
        storage.add_service(FakeProtocolServices { guid: FakeProtocol::PROTOCOL_GUID, installed: false });

        assert!(!<Protocol<FakeProtocol> as Param>::validate(&id, (&storage).into()));
    }

    #[test]
    fn test_protocol_validate_true_and_get_param_when_installed() {
        let mut storage = Storage::default();
        let mut meta = MetaData::new::<i32>();

        let id = <Protocol<FakeProtocol> as Param>::init_state(&mut storage, &mut meta).unwrap();
        storage.add_service(FakeProtocolServices { guid: FakeProtocol::PROTOCOL_GUID, installed: true });

        assert!(<Protocol<FakeProtocol> as Param>::validate(&id, (&storage).into()));
        // SAFETY: Test code - Protocol<FakeProtocol> was just validated above.
        let protocol = unsafe { <Protocol<FakeProtocol> as Param>::get_param(&id, (&storage).into()) };
        assert_eq!(protocol.value, 42);
    }

    #[test]
    fn test_option_protocol_returns_none_when_not_installed() {
        let mut storage = Storage::default();
        let mut meta = MetaData::new::<i32>();

        let id = <Option<Protocol<FakeProtocol>> as Param>::init_state(&mut storage, &mut meta).unwrap();
        // SAFETY: Option<P> validate() is always true. get_param() re-checks P internally.
        let protocol = unsafe { <Option<Protocol<FakeProtocol>> as Param>::get_param(&id, (&storage).into()) };
        assert!(protocol.is_none());
    }
}
