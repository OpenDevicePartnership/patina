//! This module contains every perf token and perf id known by this library.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

use alloc::string::String;
use core::convert::TryFrom;

use r_efi::efi;

use crate::performance_measurement_protocol::PerfAttribute;

#[derive(Debug, Eq, PartialEq)]
pub enum KnownPerfToken {
    /// SEC Phase
    SEC,
    /// DXE Phase
    DXE,
    /// PEI Phase
    PEI,
    /// BDS Phase
    BDS,
    /// Diver binding start function call.
    DriverBindingStart,
    /// Diver binding support function call.
    DriverBindingSupport,
    /// Diver binding stop function call.
    DriverBindingStop,
    /// Load a dispatched module.
    LoadImage,
    /// Dispatch modules entry oint execution
    StartImage,
    /// PEIM modules entry point execution.
    PEIM,
}

impl KnownPerfToken {
    pub const fn as_str(&self) -> &'static str {
        match self {
            KnownPerfToken::SEC => "SEC",
            KnownPerfToken::DXE => "DXE",
            KnownPerfToken::PEI => "PEI",
            KnownPerfToken::BDS => "BDS",
            KnownPerfToken::DriverBindingStart => "DB:Start",
            KnownPerfToken::DriverBindingSupport => "DB:Support",
            KnownPerfToken::DriverBindingStop => "DB:Stop",
            KnownPerfToken::LoadImage => "LoadImage",
            KnownPerfToken::StartImage => "StartImage",
            KnownPerfToken::PEIM => "PEIM",
        }
    }
}

impl TryFrom<&str> for KnownPerfToken {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let this = match value {
            v if v == Self::SEC.as_str() => Self::SEC,
            v if v == Self::DXE.as_str() => Self::DXE,
            v if v == Self::PEI.as_str() => Self::PEI,
            v if v == Self::BDS.as_str() => Self::BDS,
            v if v == Self::DriverBindingStart.as_str() => Self::DriverBindingStart,
            v if v == Self::DriverBindingSupport.as_str() => Self::DriverBindingSupport,
            v if v == Self::DriverBindingStop.as_str() => Self::DriverBindingStop,
            v if v == Self::LoadImage.as_str() => Self::LoadImage,
            v if v == Self::StartImage.as_str() => Self::StartImage,
            v if v == Self::PEIM.as_str() => Self::PEIM,
            _ => return Err(()),
        };
        Ok(this)
    }
}

#[derive(Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum KnownPerfId {
    PerfEvent = 0x00,
    ModuleStart = 0x01,
    ModuleEnd = 0x02,
    ModuleLoadImageStart = 0x03,
    ModuleLoadImageEnd = 0x04,
    ModuleDbStart = 0x05,
    ModuleDbEnd = 0x06,
    ModuleDbSupportStart = 0x07,
    ModuleDbSupportEnd = 0x08,
    ModuleDbStopStart = 0x09,
    ModuleDbStopEnd = 0x0A,
    PerfEventSignalStart = 0x10,
    PerfEventSignalEnd = 0x11,
    PerfCallbackStart = 0x20,
    PerfCallbackEnd = 0x21,
    PerfFunctionStart = 0x30,
    PerfFunctionEnd = 0x31,
    PerfInModuleStart = 0x40,
    PerfInModuleEnd = 0x41,
    PerfCrossModuleStart = 0x50,
    PerfCrossModuleEnd = 0x51,
}

impl KnownPerfId {
    pub const fn as_u16(&self) -> u16 {
        match self {
            Self::PerfEvent => Self::PerfEvent as u16,
            Self::ModuleStart => Self::ModuleStart as u16,
            Self::ModuleEnd => Self::ModuleEnd as u16,
            Self::ModuleLoadImageStart => Self::ModuleLoadImageStart as u16,
            Self::ModuleLoadImageEnd => Self::ModuleLoadImageEnd as u16,
            Self::ModuleDbStart => Self::ModuleDbStart as u16,
            Self::ModuleDbEnd => Self::ModuleDbEnd as u16,
            Self::ModuleDbSupportStart => Self::ModuleDbSupportStart as u16,
            Self::ModuleDbSupportEnd => Self::ModuleDbSupportEnd as u16,
            Self::ModuleDbStopStart => Self::ModuleDbStopStart as u16,
            Self::ModuleDbStopEnd => Self::ModuleDbStopEnd as u16,
            Self::PerfEventSignalStart => Self::PerfEventSignalStart as u16,
            Self::PerfEventSignalEnd => Self::PerfEventSignalEnd as u16,
            Self::PerfCallbackStart => Self::PerfCallbackStart as u16,
            Self::PerfCallbackEnd => Self::PerfCallbackEnd as u16,
            Self::PerfFunctionStart => Self::PerfFunctionStart as u16,
            Self::PerfFunctionEnd => Self::PerfFunctionEnd as u16,
            Self::PerfInModuleStart => Self::PerfInModuleStart as u16,
            Self::PerfInModuleEnd => Self::PerfInModuleEnd as u16,
            Self::PerfCrossModuleStart => Self::PerfCrossModuleStart as u16,
            Self::PerfCrossModuleEnd => Self::PerfCrossModuleEnd as u16,
        }
    }

    pub fn try_from_perf_info(
        handle: efi::Handle,
        string: Option<&String>,
        attribute: PerfAttribute,
    ) -> Result<Self, efi::Status> {
        if let Some(string) = string.as_ref() {
            if let Ok(token) = KnownPerfToken::try_from(string.as_str()) {
                Ok(match token {
                    KnownPerfToken::StartImage if attribute == PerfAttribute::PerfStartEntry => Self::ModuleStart,
                    KnownPerfToken::StartImage => Self::ModuleEnd,

                    KnownPerfToken::LoadImage if attribute == PerfAttribute::PerfStartEntry => {
                        Self::ModuleLoadImageStart
                    }
                    KnownPerfToken::LoadImage => Self::ModuleLoadImageEnd,

                    KnownPerfToken::DriverBindingStart if attribute == PerfAttribute::PerfStartEntry => {
                        Self::ModuleDbStart
                    }
                    KnownPerfToken::DriverBindingStart => Self::ModuleDbEnd,
                    KnownPerfToken::DriverBindingSupport if attribute == PerfAttribute::PerfStartEntry => {
                        Self::ModuleDbSupportStart
                    }
                    KnownPerfToken::DriverBindingSupport => Self::ModuleDbSupportEnd,
                    KnownPerfToken::DriverBindingStop if attribute == PerfAttribute::PerfStartEntry => {
                        Self::ModuleDbStopStart
                    }
                    KnownPerfToken::DriverBindingStop => Self::ModuleDbStopEnd,

                    KnownPerfToken::PEI | KnownPerfToken::DXE | KnownPerfToken::BDS
                        if attribute == PerfAttribute::PerfStartEntry =>
                    {
                        Self::PerfCrossModuleStart
                    }
                    KnownPerfToken::PEI | KnownPerfToken::DXE | KnownPerfToken::BDS => Self::PerfCrossModuleEnd,

                    KnownPerfToken::SEC | KnownPerfToken::PEIM if attribute == PerfAttribute::PerfStartEntry => {
                        Self::PerfInModuleStart
                    }
                    KnownPerfToken::SEC | KnownPerfToken::PEIM => Self::PerfInModuleEnd,
                })
            } else {
                Ok(match attribute {
                    PerfAttribute::PerfStartEntry => Self::PerfInModuleStart,
                    _ => Self::PerfInModuleEnd,
                })
            }
        } else if !handle.is_null() {
            if attribute == PerfAttribute::PerfStartEntry {
                Ok(KnownPerfId::PerfInModuleStart)
            } else {
                Ok(KnownPerfId::PerfInModuleEnd)
            }
        } else {
            Err(efi::Status::INVALID_PARAMETER)
        }
    }
}

impl TryFrom<u16> for KnownPerfId {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let this = match value {
            v if v == Self::PerfEvent as u16 => Self::PerfEvent,
            v if v == Self::ModuleStart as u16 => Self::ModuleStart,
            v if v == Self::ModuleEnd as u16 => Self::ModuleEnd,
            v if v == Self::ModuleLoadImageStart as u16 => Self::ModuleLoadImageStart,
            v if v == Self::ModuleLoadImageEnd as u16 => Self::ModuleLoadImageEnd,
            v if v == Self::ModuleDbStart as u16 => Self::ModuleDbStart,
            v if v == Self::ModuleDbEnd as u16 => Self::ModuleDbEnd,
            v if v == Self::ModuleDbSupportStart as u16 => Self::ModuleDbSupportStart,
            v if v == Self::ModuleDbSupportEnd as u16 => Self::ModuleDbSupportEnd,
            v if v == Self::ModuleDbStopStart as u16 => Self::ModuleDbStopStart,
            v if v == Self::ModuleDbStopEnd as u16 => Self::ModuleDbStopEnd,
            v if v == Self::PerfEventSignalStart as u16 => Self::PerfEventSignalStart,
            v if v == Self::PerfEventSignalEnd as u16 => Self::PerfEventSignalEnd,
            v if v == Self::PerfCallbackStart as u16 => Self::PerfCallbackStart,
            v if v == Self::PerfCallbackEnd as u16 => Self::PerfCallbackEnd,
            v if v == Self::PerfFunctionStart as u16 => Self::PerfFunctionStart,
            v if v == Self::PerfFunctionEnd as u16 => Self::PerfFunctionEnd,
            v if v == Self::PerfInModuleStart as u16 => Self::PerfInModuleStart,
            v if v == Self::PerfInModuleEnd as u16 => Self::PerfInModuleEnd,
            v if v == Self::PerfCrossModuleStart as u16 => Self::PerfCrossModuleStart,
            v if v == Self::PerfCrossModuleEnd as u16 => Self::PerfCrossModuleEnd,
            _ => return Err(()),
        };
        Ok(this)
    }
}

#[cfg(test)]
mod test {
    use core::{assert_eq, convert::From, ptr};

    use super::*;

    #[test]
    fn test_known_token() {
        assert!(KnownPerfToken::try_from("").is_err());
        assert_eq!(Ok(KnownPerfToken::SEC), KnownPerfToken::try_from("SEC"));
        assert_eq!(Ok(KnownPerfToken::DXE), KnownPerfToken::try_from("DXE"));
        assert_eq!(Ok(KnownPerfToken::PEI), KnownPerfToken::try_from("PEI"));
        assert_eq!(Ok(KnownPerfToken::BDS), KnownPerfToken::try_from("BDS"));
        assert_eq!(Ok(KnownPerfToken::DriverBindingStart), KnownPerfToken::try_from("DB:Start"));
        assert_eq!(Ok(KnownPerfToken::DriverBindingSupport), KnownPerfToken::try_from("DB:Support"));
        assert_eq!(Ok(KnownPerfToken::DriverBindingStop), KnownPerfToken::try_from("DB:Stop"));
        assert_eq!(Ok(KnownPerfToken::LoadImage), KnownPerfToken::try_from("LoadImage"));
        assert_eq!(Ok(KnownPerfToken::StartImage), KnownPerfToken::try_from("StartImage"));
        assert_eq!(Ok(KnownPerfToken::PEIM), KnownPerfToken::try_from("PEIM"));
    }

    #[test]
    fn test_known_perf_id() {
        assert_eq!(
            Ok(KnownPerfId::ModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("StartImage")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::ModuleEnd),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("StartImage")),
                PerfAttribute::PerfEndEntry
            )
        );

        assert_eq!(
            Ok(KnownPerfId::ModuleLoadImageStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("LoadImage")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::ModuleLoadImageEnd),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("LoadImage")),
                PerfAttribute::PerfEndEntry
            )
        );

        assert_eq!(
            Ok(KnownPerfId::ModuleDbStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Start")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::ModuleDbEnd),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Start")),
                PerfAttribute::PerfEndEntry
            )
        );

        assert_eq!(
            Ok(KnownPerfId::ModuleDbSupportStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Support")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::ModuleDbSupportEnd),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Support")),
                PerfAttribute::PerfEndEntry
            )
        );

        assert_eq!(
            Ok(KnownPerfId::ModuleDbStopStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Stop")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::ModuleDbStopEnd),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DB:Stop")),
                PerfAttribute::PerfEndEntry
            )
        );

        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("PEI")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, Some(&String::from("PEI")), PerfAttribute::PerfEndEntry)
        );
        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("DXE")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, Some(&String::from("DXE")), PerfAttribute::PerfEndEntry)
        );
        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("BDS")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::PerfCrossModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, Some(&String::from("BDS")), PerfAttribute::PerfEndEntry)
        );

        assert_eq!(
            Ok(KnownPerfId::PerfInModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("PEIM")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::PerfInModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, Some(&String::from("PEIM")), PerfAttribute::PerfEndEntry)
        );
        assert_eq!(
            Ok(KnownPerfId::PerfInModuleStart),
            KnownPerfId::try_from_perf_info(
                1 as efi::Handle,
                Some(&String::from("SEC")),
                PerfAttribute::PerfStartEntry
            )
        );
        assert_eq!(
            Ok(KnownPerfId::PerfInModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, Some(&String::from("SEC")), PerfAttribute::PerfEndEntry)
        );

        assert_eq!(
            Ok(KnownPerfId::PerfInModuleStart),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, None, PerfAttribute::PerfStartEntry)
        );
        assert_eq!(
            Ok(KnownPerfId::PerfInModuleEnd),
            KnownPerfId::try_from_perf_info(1 as efi::Handle, None, PerfAttribute::PerfEndEntry)
        );

        assert_eq!(
            Err(efi::Status::INVALID_PARAMETER),
            KnownPerfId::try_from_perf_info(ptr::null_mut(), None, PerfAttribute::PerfStartEntry)
        );
    }
}
