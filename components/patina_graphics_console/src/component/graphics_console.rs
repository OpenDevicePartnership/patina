//! The graphics console component.
//!
//! [`GraphicsConsoleProvider`] installs an EFI Driver Binding Protocol (`GraphicsConsoleDriverBinding`)
//! that binds a `Simple Text Output` console to every Graphics Output Protocol (GOP) controller in the
//! system. Each controller it starts gets its own `SimpleTextOutputHolder`, rendering text with the
//! EFI HII Font Protocol over the GOP.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

extern crate alloc;

use alloc::boxed::Box;
use core::ptr::NonNull;

use patina::{
    char16,
    component::{
        component,
        service::{
            Service,
            pcd::PcdServices,
            uefi_services::{
                driver_model::{
                    component_name::UefiDriverModelComponentName,
                    driver_binding::DriverBinding,
                    language::{LanguageEntry, LanguageTable},
                },
                handle::Handle,
                protocol::{OpenAttributes, ProtocolError, ProtocolServices, ProtocolServicesExt},
                tpl::TplServices,
            },
        },
    },
    error::Result,
    protocol::ProtocolInterface,
    standard::efi::protocols::{device_path, graphics_output, hii_database, hii_font},
    uefi::device_path::walker::DevicePathWalker,
};

use crate::console::{
    font::HiiFontHandle, font_package, gop::GopHandle, output::SimpleTextOutputHolder, pcd::ConsolePreferences,
};

/// The name this driver publishes through the EFI Component Name protocols.
static DRIVER_NAME: LanguageTable =
    LanguageTable(&[LanguageEntry { iso639: b"eng", rfc4646: "en", name: char16!("Graphics Console") }]);

/// Installs the graphics console's driver binding.
///
/// `Service<dyn PcdServices>` is an optional dependency. A platform with no PCD driver gets a console
/// that defaults to the display's highest resolution and largest text mode.
#[derive(Default)]
pub struct GraphicsConsoleProvider;

#[component]
impl GraphicsConsoleProvider {
    /// Creates a new instance of the component.
    pub fn new() -> Self {
        Self
    }

    fn entry_point(
        self,
        protocols: Service<dyn ProtocolServices>,
        tpl: Service<dyn TplServices>,
        pcd: Option<Service<dyn PcdServices>>,
    ) -> Result<()> {
        let binding = GraphicsConsoleDriverBinding { protocols, tpl, pcd };
        protocols.install_driver_binding(binding)?;
        Ok(())
    }
}

/// The EFI Driver Binding implementation. Binds to every Graphics Output Protocol
/// controller and installs a [`SimpleTextOutputHolder`] over each one it starts.
struct GraphicsConsoleDriverBinding {
    protocols: Service<dyn ProtocolServices>,
    tpl: Service<dyn TplServices>,
    pcd: Option<Service<dyn PcdServices>>,
}

impl UefiDriverModelComponentName for GraphicsConsoleDriverBinding {
    fn driver_name(&self) -> &'static LanguageTable {
        &DRIVER_NAME
    }
}

impl DriverBinding for GraphicsConsoleDriverBinding {
    fn supported(
        &self,
        agent: Handle,
        controller: Handle,
        _remaining_device_path: Option<DevicePathWalker>,
    ) -> core::result::Result<(), ProtocolError> {
        // Requiring a real device path keeps this driver from binding on top of a virtual,
        // aggregate GOP handle (for example one con splitter produces).  Both opens are
        // dropped (closed) at the end of this function. `Start()` re-opens whatever it actually needs.
        let _device_path = self.protocols.open_protocol::<device_path::Protocol>(
            controller,
            agent,
            OpenAttributes::ByDriver { controller },
        )?;
        let _gop = self.protocols.open_protocol::<graphics_output::Protocol>(
            controller,
            agent,
            OpenAttributes::ByDriver { controller },
        )?;

        // A console with no font source to rasterize with is not useful, so require one to be
        // present before binding.
        self.protocols.locate_protocol::<hii_font::Protocol>().map_err(|_| ProtocolError::NotFound)?;
        Ok(())
    }

    fn start(
        &self,
        agent: Handle,
        controller: Handle,
        _remaining_device_path: Option<DevicePathWalker>,
    ) -> core::result::Result<(), ProtocolError> {
        let gop_ptr = self.protocols.open_interface(
            controller,
            <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID,
            agent,
            OpenAttributes::ByDriver { controller },
        )?;
        // Note: This is a closure so every early-return path below still releases the GOP usage on failure.
        let result = (|| {
            let gop = NonNull::new(gop_ptr.as_raw().cast::<graphics_output::Protocol>())
                .ok_or(ProtocolError::InvalidParameter)?;
            // SAFETY: `gop_ptr` was just returned by `open_interface` for `graphics_output::Protocol`,
            // whose `ProtocolInterface` impl guarantees the interface has that layout. It stays
            // open (and this pointer valid) until `stop()` closes it.
            let gop = unsafe { GopHandle::new(gop) };

            let hii_font_ptr =
                self.protocols.locate_interface(<hii_font::Protocol as ProtocolInterface>::PROTOCOL_GUID)?;
            let hii_font =
                NonNull::new(hii_font_ptr.as_raw().cast::<hii_font::Protocol>()).ok_or(ProtocolError::NotFound)?;
            // SAFETY: as above, for `hii_font::Protocol`. HII Font is a system-wide service (not
            // opened against a controller), so it is only located, never closed in `stop()`.
            let hii_font = unsafe { HiiFontHandle::new(hii_font) };

            // Note: The console still works without a font package, just without any glyphs to render until
            // something else supplies a font package.
            if let Ok(hii_database) = self.protocols.locate_protocol::<hii_database::Protocol>()
                && let Err(err) = font_package::register(hii_database)
            {
                log::warn!("Failed to register default HII font package: {err:?}");
            }

            let preferences = ConsolePreferences::read(self.pcd);
            let holder =
                SimpleTextOutputHolder::new(gop, hii_font, self.tpl, preferences.resolution, preferences.text_mode)
                    .map_err(|_| ProtocolError::InvalidParameter)?;

            self.protocols.install_protocol::<SimpleTextOutputHolder>(Some(controller), holder)?;
            Ok(())
        })();

        if result.is_err() {
            let _ = self.protocols.close_interface(
                controller,
                <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID,
                agent,
                Some(controller),
            );
        }
        result
    }

    fn stop(&self, agent: Handle, controller: Handle, _children: &[Handle]) -> core::result::Result<(), ProtocolError> {
        let interface = self
            .protocols
            .interface_on_handle(controller, <SimpleTextOutputHolder as ProtocolInterface>::PROTOCOL_GUID)?;
        self.protocols.uninstall_interface(
            controller,
            <SimpleTextOutputHolder as ProtocolInterface>::PROTOCOL_GUID,
            interface,
        )?;

        // SAFETY: `interface` was produced by `install_protocol::<SimpleTextOutputHolder>` in
        // `start()`, which leaked a `Box<SimpleTextOutputHolder>` at this address. The uninstall
        // above just succeeded, so no other code can reach this pointer through the protocol
        // database anymore, making it safe to reclaim and drop.
        drop(unsafe { Box::from_raw(interface.as_raw().cast::<SimpleTextOutputHolder>()) });

        self.protocols.close_interface(
            controller,
            <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID,
            agent,
            Some(controller),
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use core::ffi::c_void;

    use patina::{
        component::service::uefi_services::protocol::{MockProtocolServices, ProtocolPtr},
        error::EfiError,
    };

    use crate::test_support::{FakeGop, FakeHiiFont, permissive_tpl};

    static FAKE_DEVICE_PATH: device_path::Protocol = device_path::Protocol { r#type: 0, sub_type: 0, length: [4, 0] };

    fn fake_agent() -> Handle {
        Handle::from_raw(0x1000_usize as *mut c_void).unwrap()
    }

    fn fake_controller() -> Handle {
        Handle::from_raw(0x2000_usize as *mut c_void).unwrap()
    }

    fn binding(protocols: Service<dyn ProtocolServices>) -> GraphicsConsoleDriverBinding {
        GraphicsConsoleDriverBinding { protocols, tpl: permissive_tpl(), pcd: None }
    }

    #[test]
    fn test_driver_name_reports_english() {
        let mock = MockProtocolServices::new();
        let name = binding(Service::mock(Box::new(mock))).driver_name();
        assert_eq!(name.0.len(), 1);
        assert_eq!(name.0[0].iso639, b"eng");
    }

    // ---- entry_point ----

    #[test]
    fn test_entry_point_installs_driver_binding_and_succeeds() {
        let mut mock = MockProtocolServices::new();
        mock.expect_register_agent().times(1).returning(|| Ok(fake_agent()));
        mock.expect_install_interface().times(1).returning(|handle, _, _| {
            assert_eq!(handle, Some(fake_agent()));
            Ok(fake_agent())
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = GraphicsConsoleProvider::new().entry_point(protocols, permissive_tpl(), None);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_entry_point_propagates_install_failure() {
        let mut mock = MockProtocolServices::new();
        mock.expect_register_agent().times(1).returning(|| Ok(fake_agent()));
        mock.expect_install_interface().times(1).returning(|_, _, _| Err(ProtocolError::OutOfResources));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = GraphicsConsoleProvider::new().entry_point(protocols, permissive_tpl(), None);

        assert_eq!(result, Err(EfiError::OutOfResources));
    }

    // ---- supported ----

    #[test]
    fn test_supported_succeeds_when_device_path_gop_and_hii_font_present() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let gop_addr = gop.handle().as_ptr() as usize;
        let hii_font_addr = hii_font.handle().as_ptr() as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(2).returning(move |_, guid, _, attrs| {
            assert_eq!(attrs, OpenAttributes::ByDriver { controller: fake_controller() });
            if guid == <device_path::Protocol as ProtocolInterface>::PROTOCOL_GUID {
                ProtocolPtr::from_raw(core::ptr::addr_of!(FAKE_DEVICE_PATH).cast_mut().cast())
                    .ok_or(ProtocolError::Internal)
            } else {
                ProtocolPtr::from_raw(gop_addr as *mut c_void).ok_or(ProtocolError::Internal)
            }
        });
        mock.expect_close_interface().times(2).returning(|_, _, _, _| Ok(()));
        mock.expect_locate_interface().times(1).returning(move |guid| {
            assert_eq!(guid, <hii_font::Protocol as ProtocolInterface>::PROTOCOL_GUID);
            ProtocolPtr::from_raw(hii_font_addr as *mut c_void).ok_or(ProtocolError::Internal)
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).supported(fake_agent(), fake_controller(), None);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_supported_fails_when_device_path_not_present() {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(1).returning(|_, guid, _, _| {
            assert_eq!(guid, <device_path::Protocol as ProtocolInterface>::PROTOCOL_GUID);
            Err(ProtocolError::NotFound)
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).supported(fake_agent(), fake_controller(), None);

        assert_eq!(result, Err(ProtocolError::NotFound));
    }

    #[test]
    fn test_supported_fails_when_gop_not_present() {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(2).returning(|_, guid, _, _| {
            if guid == <device_path::Protocol as ProtocolInterface>::PROTOCOL_GUID {
                ProtocolPtr::from_raw(core::ptr::addr_of!(FAKE_DEVICE_PATH).cast_mut().cast())
                    .ok_or(ProtocolError::Internal)
            } else {
                Err(ProtocolError::NotFound)
            }
        });
        mock.expect_close_interface().times(1).returning(|_, _, _, _| Ok(()));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).supported(fake_agent(), fake_controller(), None);

        assert_eq!(result, Err(ProtocolError::NotFound));
    }

    #[test]
    fn test_supported_maps_any_locate_failure_to_not_found() {
        let gop = FakeGop::new(&[(800, 600)]);
        let gop_addr = gop.handle().as_ptr() as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(2).returning(move |_, guid, _, _| {
            if guid == <device_path::Protocol as ProtocolInterface>::PROTOCOL_GUID {
                ProtocolPtr::from_raw(core::ptr::addr_of!(FAKE_DEVICE_PATH).cast_mut().cast())
                    .ok_or(ProtocolError::Internal)
            } else {
                ProtocolPtr::from_raw(gop_addr as *mut c_void).ok_or(ProtocolError::Internal)
            }
        });
        mock.expect_close_interface().times(2).returning(|_, _, _, _| Ok(()));
        mock.expect_locate_interface().times(1).returning(|_| Err(ProtocolError::AccessDenied));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).supported(fake_agent(), fake_controller(), None);

        // `supported` remaps any locate failure to `NotFound`.
        assert_eq!(result, Err(ProtocolError::NotFound));
    }

    // ---- start ----

    #[test]
    fn test_start_installs_console_when_hii_database_is_absent() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let gop_addr = gop.handle().as_ptr() as usize;
        let hii_font_addr = hii_font.handle().as_ptr() as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(1).returning(move |_, guid, _, attrs| {
            assert_eq!(guid, <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID);
            assert_eq!(attrs, OpenAttributes::ByDriver { controller: fake_controller() });
            ProtocolPtr::from_raw(gop_addr as *mut c_void).ok_or(ProtocolError::Internal)
        });
        mock.expect_locate_interface().times(2).returning(move |guid| {
            if guid == <hii_font::Protocol as ProtocolInterface>::PROTOCOL_GUID {
                ProtocolPtr::from_raw(hii_font_addr as *mut c_void).ok_or(ProtocolError::Internal)
            } else {
                // hii_database::Protocol is absent, so font package registration is skipped.
                Err(ProtocolError::NotFound)
            }
        });
        mock.expect_install_interface().times(1).returning(|handle, guid, _| {
            assert_eq!(handle, Some(fake_controller()));
            assert_eq!(guid, <SimpleTextOutputHolder as ProtocolInterface>::PROTOCOL_GUID);
            Ok(fake_controller())
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).start(fake_agent(), fake_controller(), None);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_start_fails_when_gop_open_fails_without_attempting_cleanup() {
        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface().times(1).returning(|_, _, _, _| Err(ProtocolError::NotFound));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).start(fake_agent(), fake_controller(), None);

        assert_eq!(result, Err(ProtocolError::NotFound));
    }

    #[test]
    fn test_start_closes_gop_when_hii_font_is_not_located() {
        let gop = FakeGop::new(&[(800, 600)]);
        let gop_addr = gop.handle().as_ptr() as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface()
            .times(1)
            .returning(move |_, _, _, _| ProtocolPtr::from_raw(gop_addr as *mut c_void).ok_or(ProtocolError::Internal));
        mock.expect_locate_interface().times(1).returning(|_| Err(ProtocolError::AccessDenied));
        mock.expect_close_interface().times(1).returning(|_, guid, agent, controller| {
            assert_eq!(guid, <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID);
            assert_eq!(agent, fake_agent());
            assert_eq!(controller, Some(fake_controller()));
            Ok(())
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).start(fake_agent(), fake_controller(), None);

        // `start` propagates the underlying error as-is.
        assert_eq!(result, Err(ProtocolError::AccessDenied));
    }

    #[test]
    fn test_start_closes_gop_when_install_protocol_fails() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let gop_addr = gop.handle().as_ptr() as usize;
        let hii_font_addr = hii_font.handle().as_ptr() as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_open_interface()
            .times(1)
            .returning(move |_, _, _, _| ProtocolPtr::from_raw(gop_addr as *mut c_void).ok_or(ProtocolError::Internal));
        mock.expect_locate_interface().times(2).returning(move |guid| {
            if guid == <hii_font::Protocol as ProtocolInterface>::PROTOCOL_GUID {
                ProtocolPtr::from_raw(hii_font_addr as *mut c_void).ok_or(ProtocolError::Internal)
            } else {
                Err(ProtocolError::NotFound)
            }
        });
        mock.expect_install_interface().times(1).returning(|_, _, _| Err(ProtocolError::OutOfResources));
        mock.expect_close_interface().times(1).returning(|_, _, _, _| Ok(()));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).start(fake_agent(), fake_controller(), None);

        assert_eq!(result, Err(ProtocolError::OutOfResources));
    }

    // ---- stop ----

    #[test]
    fn test_stop_uninstalls_holder_and_releases_gop() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let holder = SimpleTextOutputHolder::new(
            gop.gop_handle(),
            hii_font.hii_font_handle(),
            permissive_tpl(),
            Some((800, 600)),
            Some((80, 25)),
        )
        .unwrap();
        let holder_addr = Box::into_raw(holder) as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_interface_on_handle().times(1).returning(move |handle, guid| {
            assert_eq!(handle, fake_controller());
            assert_eq!(guid, <SimpleTextOutputHolder as ProtocolInterface>::PROTOCOL_GUID);
            ProtocolPtr::from_raw(holder_addr as *mut c_void).ok_or(ProtocolError::Internal)
        });
        mock.expect_uninstall_interface().times(1).returning(|_, _, _| Ok(()));
        mock.expect_close_interface().times(1).returning(|_, guid, agent, controller| {
            assert_eq!(guid, <graphics_output::Protocol as ProtocolInterface>::PROTOCOL_GUID);
            assert_eq!(agent, fake_agent());
            assert_eq!(controller, Some(fake_controller()));
            Ok(())
        });
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).stop(fake_agent(), fake_controller(), &[]);

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn test_stop_propagates_close_interface_failure() {
        let gop = FakeGop::new(&[(800, 600)]);
        let hii_font = FakeHiiFont::new();
        let holder = SimpleTextOutputHolder::new(
            gop.gop_handle(),
            hii_font.hii_font_handle(),
            permissive_tpl(),
            Some((800, 600)),
            Some((80, 25)),
        )
        .unwrap();
        let holder_addr = Box::into_raw(holder) as usize;

        let mut mock = MockProtocolServices::new();
        mock.expect_interface_on_handle()
            .times(1)
            .returning(move |_, _| ProtocolPtr::from_raw(holder_addr as *mut c_void).ok_or(ProtocolError::Internal));
        mock.expect_uninstall_interface().times(1).returning(|_, _, _| Ok(()));
        mock.expect_close_interface().times(1).returning(|_, _, _, _| Err(ProtocolError::AccessDenied));
        let protocols: Service<dyn ProtocolServices> = Service::mock(Box::new(mock));

        let result = binding(protocols).stop(fake_agent(), fake_controller(), &[]);

        assert_eq!(result, Err(ProtocolError::AccessDenied));
    }
}
