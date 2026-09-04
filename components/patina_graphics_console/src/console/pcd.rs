//! Dynamic-PCD-based resolution and text-mode preferences.
//!
//! C-based BDS code (for example `MdeModulePkg/Application/BootManagerMenuApp/BootManagerMenu.c`)
//! controls console resolution by writing `PcdVideoHorizontalResolution`, `PcdVideoVerticalResolution`,
//! `PcdConOutColumn`, and `PcdConOutRow`, then disconnecting and reconnecting the console handles so
//! the driver picks the new values up. This module reads those same four PCDs so this component
//! stays interoperable with that existing behavior. To do so, each `DriverBinding::start()` call
//! re-reads them, and a `DisconnectController`/`ConnectController` cycle naturally re-invokes `start()`.
//!
//! ## PCD token numbers
//!
//! These four are declared `DynamicEx` in `MdeModulePkg.dec` (`[PcdsPatchableInModule, PcdsDynamic,
//! PcdsDynamicEx]`), which fixes their token numbers in the DEC file itself, so they are consistent
//! across every platform, provided that platform classifies all four PCDs as `PcdsDynamicEx`
//! (not plain `PcdsDynamic`).
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use patina::{
    BinaryGuid,
    component::service::{
        Service,
        pcd::{PcdServices, PcdToken},
    },
};

/// `gEfiMdeModulePkgTokenSpaceGuid`, as declared in `MdeModulePkg.dec`.
const MDE_MODULE_PKG_TOKEN_SPACE_GUID: BinaryGuid = BinaryGuid::from_string("A1AFF049-FDEB-442A-B320-13AB4CB72BBC");

const PCD_CON_OUT_ROW: PcdToken = PcdToken::DynamicEx(MDE_MODULE_PKG_TOKEN_SPACE_GUID, 0x4000_0006);
const PCD_CON_OUT_COLUMN: PcdToken = PcdToken::DynamicEx(MDE_MODULE_PKG_TOKEN_SPACE_GUID, 0x4000_0007);
const PCD_VIDEO_HORIZONTAL_RESOLUTION: PcdToken = PcdToken::DynamicEx(MDE_MODULE_PKG_TOKEN_SPACE_GUID, 0x4000_0009);
const PCD_VIDEO_VERTICAL_RESOLUTION: PcdToken = PcdToken::DynamicEx(MDE_MODULE_PKG_TOKEN_SPACE_GUID, 0x4000_000a);

/// Desired video resolution and text-mode dimensions, resolved from the platform's dynamic PCDs
/// when available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ConsolePreferences {
    /// Desired horizontal/vertical resolution in pixels. `None` means "use the highest resolution
    /// the display supports", matching `PcdVideoHorizontalResolution`/`PcdVideoVerticalResolution`
    /// both being `0`.
    pub(crate) resolution: Option<(u32, u32)>,
    /// Desired text-mode columns/rows. `None` means "use the largest available text mode",
    /// matching `PcdConOutColumn`/`PcdConOutRow` both being `0`.
    pub(crate) text_mode: Option<(u32, u32)>,
}

impl ConsolePreferences {
    /// Reads the resolution/text-mode PCDs through `pcd`.
    ///
    /// Returns the "auto" default (highest resolution, largest text mode) when `pcd` is `None` or
    /// any individual read fails.
    pub(crate) fn read(pcd: Option<Service<dyn PcdServices>>) -> Self {
        let Some(pcd) = pcd else { return Self::default() };

        // SAFETY: this is a well-known DynamicEx token (see the module documentation above),
        // read with the `u32` accessor matching its `UINT32` declaration in `MdeModulePkg.dec`.
        let horizontal = unsafe { pcd.get_u32(PCD_VIDEO_HORIZONTAL_RESOLUTION) }.unwrap_or(0);
        // SAFETY: as above.
        let vertical = unsafe { pcd.get_u32(PCD_VIDEO_VERTICAL_RESOLUTION) }.unwrap_or(0);
        // SAFETY: as above.
        let columns = unsafe { pcd.get_u32(PCD_CON_OUT_COLUMN) }.unwrap_or(0);
        // SAFETY: as above.
        let rows = unsafe { pcd.get_u32(PCD_CON_OUT_ROW) }.unwrap_or(0);

        Self {
            resolution: (horizontal != 0 && vertical != 0).then_some((horizontal, vertical)),
            text_mode: (columns != 0 && rows != 0).then_some((columns, rows)),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_pcd_console_preferences_none_service_is_auto() {
        assert_eq!(ConsolePreferences::read(None), ConsolePreferences { resolution: None, text_mode: None });
    }

    #[test]
    fn test_pcd_console_preferences_reads_nonzero_values() {
        let mut mock = patina::component::service::pcd::MockPcdServices::new();
        mock.expect_get_u32().returning(|token| match token {
            PCD_VIDEO_HORIZONTAL_RESOLUTION => Ok(1920),
            PCD_VIDEO_VERTICAL_RESOLUTION => Ok(1080),
            PCD_CON_OUT_COLUMN | PCD_CON_OUT_ROW => Ok(0),
            _ => panic!("unexpected token {token:?}"),
        });

        let preferences = ConsolePreferences::read(Some(Service::mock(alloc::boxed::Box::new(mock))));
        assert_eq!(preferences, ConsolePreferences { resolution: Some((1920, 1080)), text_mode: None });
    }

    #[test]
    fn test_pcd_console_preferences_reads_text_mode_nonzero() {
        let mut mock = patina::component::service::pcd::MockPcdServices::new();
        mock.expect_get_u32().returning(|token| match token {
            PCD_VIDEO_HORIZONTAL_RESOLUTION | PCD_VIDEO_VERTICAL_RESOLUTION => Ok(0),
            PCD_CON_OUT_COLUMN => Ok(100),
            PCD_CON_OUT_ROW => Ok(31),
            _ => panic!("unexpected token {token:?}"),
        });

        let preferences = ConsolePreferences::read(Some(Service::mock(alloc::boxed::Box::new(mock))));
        assert_eq!(preferences, ConsolePreferences { resolution: None, text_mode: Some((100, 31)) });
    }

    #[test]
    fn test_pcd_console_preferences_read_failure_defaults_to_auto() {
        let mut mock = patina::component::service::pcd::MockPcdServices::new();
        mock.expect_get_u32().returning(|_| Err(patina::component::service::pcd::PcdError::NotFound));

        let preferences = ConsolePreferences::read(Some(Service::mock(alloc::boxed::Box::new(mock))));
        assert_eq!(preferences, ConsolePreferences { resolution: None, text_mode: None });
    }
}
