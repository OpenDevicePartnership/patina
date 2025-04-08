use core::{
    convert::TryFrom,
    result::Result::{Err, Ok},
};

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
