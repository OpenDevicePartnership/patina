//! This module contains every Performance Token and Performance ID for well-known performance events within
//! the Patina SDK.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use alloc::string::String;
use core::convert::TryFrom;

use crate::{performance::measurement::PerfAttribute, standard::efi};

/// Performance tokens for well-known performance events.
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
    /// Dispatch modules entry point execution
    StartImage,
    /// PEIM modules entry point execution.
    PEIM,
}

impl KnownPerfToken {
    /// Returns the string representation of the `KnownPerfToken`.
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

/// Performance IDs for well-known performance events.
#[derive(Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum KnownPerfId {
    /// The general perf event ID, used for general performance events.
    PerfEvent = 0x00,
    /// Core Measurement: The performance ID for when the dispatcher dispatches a module.
    ModuleStart = 0x01,
    /// Core Measurement: The performance ID for when the dispatched module finishes execution.
    ModuleEnd = 0x02,
    /// Core Measurement: The performance ID for when the dispatcher begins loading an image.
    ModuleLoadImageStart = 0x03,
    /// Core Measurement: The performance ID for when the dispatcher finishes loading an image.
    ModuleLoadImageEnd = 0x04,
    /// Core Measurement: The performance ID for when the driver binding starts.
    ModuleDbStart = 0x05,
    /// Core Measurement: The performance ID for when the driver binding ends.
    ModuleDbEnd = 0x06,
    /// Core Measurement: The performance ID for when the driver binding support starts.
    ModuleDbSupportStart = 0x07,
    /// Core Measurement: The performance ID for when the driver binding support ends.
    ModuleDbSupportEnd = 0x08,
    /// The performance ID for when the driver binding stop starts.
    ModuleDbStopStart = 0x09,
    /// The performance ID for when the driver binding stop ends.
    ModuleDbStopEnd = 0x0A,
    /// The performance ID for the start of event signal behavior.
    PerfEventSignalStart = 0x10,
    /// The performance ID for the end of event signal behavior.
    PerfEventSignalEnd = 0x11,
    /// The performance ID for the start of callback behavior.
    PerfCallbackStart = 0x20,
    /// The performance ID for the end of callback behavior.
    PerfCallbackEnd = 0x21,
    /// The performance ID for the start of a callback function.
    PerfFunctionStart = 0x30,
    /// The performance ID for the end of a callback function.
    PerfFunctionEnd = 0x31,
    /// The performance ID for behavior within a module.
    PerfInModuleStart = 0x40,
    /// The performance ID for the end of behavior within a module.
    PerfInModuleEnd = 0x41,
    /// The performance ID for the start of behavior spanning multiple modules.
    PerfCrossModuleStart = 0x50,
    /// The performance ID for the end of behavior spanning multiple modules.
    PerfCrossModuleEnd = 0x51,
}

impl KnownPerfId {
    /// Returns the `u16` representation of the `KnownPerfId`.
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

    /// Attempts to convert the provided metadata to a `KnownPerfId`.
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

    /// Normalizes a raw performance id for a start/end measurement entry.
    ///
    /// Mirrors the EDK II `CreatePerformanceMeasurement` behavior for token-based measurements:
    /// - Entries with [`PerfAttribute::PerfEntry`] carry an explicit known id and are returned unchanged.
    /// - A `perf_id` of `0` is derived from the caller metadata via [`Self::try_from_perf_info`].
    /// - An id that is both a known id and a known token is rejected.
    /// - An unknown id is snapped to the low-4-bit start/end convention (start ids clear the low nibble; end ids set
    ///   it).
    ///
    /// ## Errors
    ///
    /// Returns [`efi::Status::INVALID_PARAMETER`] when the metadata is inconsistent or an id cannot be derived.
    pub fn normalize_perf_id(
        perf_id: u16,
        handle: efi::Handle,
        token: Option<&String>,
        attribute: PerfAttribute,
    ) -> Result<u16, efi::Status> {
        if attribute == PerfAttribute::PerfEntry {
            return Ok(perf_id);
        }

        if perf_id == 0 {
            return Ok(Self::try_from_perf_info(handle, token, attribute)?.as_u16());
        }

        let is_known_id = KnownPerfId::try_from(perf_id).is_ok();
        let is_known_token = token.is_some_and(|s| KnownPerfToken::try_from(s.as_str()).is_ok());

        if is_known_id && is_known_token {
            return Err(efi::Status::INVALID_PARAMETER);
        }

        if !is_known_id && !is_known_token {
            // By convention, a start measurement has its lower 4 bits as 0, and an end measurement does not.
            if attribute == PerfAttribute::PerfStartEntry && (perf_id & 0x000F) != 0 {
                return Ok(perf_id & 0xFFF0);
            } else if attribute == PerfAttribute::PerfEndEntry && (perf_id & 0x000F) == 0 {
                return Ok(perf_id + 1);
            }
        }

        Ok(perf_id)
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
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
    fn test_normalize_perf_id_perf_entry_is_unchanged() {
        // PerfEntry carries an explicit id and must pass through untouched, even a "start-shaped" value.
        assert_eq!(Ok(0x1234), KnownPerfId::normalize_perf_id(0x1234, ptr::null_mut(), None, PerfAttribute::PerfEntry));
    }

    #[test]
    fn test_normalize_perf_id_zero_is_derived_from_token() {
        assert_eq!(
            Ok(KnownPerfId::ModuleStart.as_u16()),
            KnownPerfId::normalize_perf_id(
                0,
                1 as efi::Handle,
                Some(&String::from("StartImage")),
                PerfAttribute::PerfStartEntry
            )
        );
    }

    #[test]
    fn test_normalize_perf_id_zero_without_metadata_errors() {
        assert_eq!(
            Err(efi::Status::INVALID_PARAMETER),
            KnownPerfId::normalize_perf_id(0, ptr::null_mut(), None, PerfAttribute::PerfStartEntry)
        );
    }

    #[test]
    fn test_normalize_perf_id_known_id_and_token_is_rejected() {
        assert_eq!(
            Err(efi::Status::INVALID_PARAMETER),
            KnownPerfId::normalize_perf_id(
                KnownPerfId::ModuleStart.as_u16(),
                1 as efi::Handle,
                Some(&String::from("StartImage")),
                PerfAttribute::PerfStartEntry
            )
        );
    }

    #[test]
    fn test_normalize_perf_id_unknown_applies_low_bit_convention() {
        // Start entries clear the low nibble.
        assert_eq!(
            Ok(0x1230),
            KnownPerfId::normalize_perf_id(0x1234, 1 as efi::Handle, None, PerfAttribute::PerfStartEntry)
        );
        // End entries set the low nibble by incrementing when it is zero.
        assert_eq!(
            Ok(0x1231),
            KnownPerfId::normalize_perf_id(0x1230, 1 as efi::Handle, None, PerfAttribute::PerfEndEntry)
        );
    }

    #[test]
    fn test_normalize_perf_id_known_id_unknown_token_is_unchanged() {
        let id = KnownPerfId::ModuleStart.as_u16();
        assert_eq!(Ok(id), KnownPerfId::normalize_perf_id(id, 1 as efi::Handle, None, PerfAttribute::PerfStartEntry));
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
