use core::fmt;

/// # Boot Mode
///
/// The system boot mode indicates the "mode" in which the system is booting. The boot mode concept allows the firmware
/// to accommodate system initialization specific to a given set of circumstances represented by the boot mode. It is
/// a single value set in the HOB producer phase (e.g. PEI) and passed to the DXE phase via the Phase Handoff
/// Information Table (PHIT) HOB. During the HOB producer phase, various modules may modify the boot mode until it
/// settles upon a final value before being passed to the DXE phase.
///
/// ## References
///
/// - See [PI Spec 1.8A - Defined Boot Modes](https://uefi.org/specs/PI/1.8A/V1_Boot_Paths.html#defined-boot-modes)
/// - See [PI SPec 1.8A - PHIT HOB](https://uefi.org/specs/PI/1.8A/V3_HOB_Code_Definitions.html#efi-hob-handoff-info-table-phit-hob>)
///
/// ## Example
/// ```
/// use patina::pi::BootMode;
///
/// let boot_mode = BootMode::BootWithFullConfiguration;
/// println!("Boot Mode: {}", boot_mode);
/// ```
///
/// ## Note
///
/// All targets currently assume that that the boot mode is represented as a u32
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The basic S0 boot path. Informs all PEIMs to do a full configuration. The basic S0 boot path must be supported.
    BootWithFullConfiguration,
    /// A variation on the basic S0 boot path. Indicates that the minimal amount of hardware should be initialized
    /// to boot the system.
    BootWithMinimalConfiguration,
    /// A variation on the basic S0 boot path. Indicates that the configuration data from the last boot should be used
    /// without any changes.
    BootAssumingNoConfigurationChanges,
    /// A variation on the basic S0 boot path. Indicates that the any diagnostic code should be run.
    BootWithFullConfigurationPlusDiagnostic,
    /// A variation on the basic S0 boot path. Indicates that a known set of safe values for programming hardware
    /// should be used.
    BootWithDefaultSettings,
    /// The current boot is a S4 (Save to Disk) hibernate resume.
    BootOnS4Resume,
    /// The current boot is a S5 (Soft Off) power on. Some platforms may use this mode to differentiate between a normal
    /// for example, if buttons other than the power button can wake the system.
    BootOnS5Resume,
    /// The current boot is a manufacturing mode boot. Firmware drivers may parameterize actions based on this mode
    /// that should only occur in a factory or a manufacturing environment.
    BootWithMfgModeSettings,
    /// The boot is a S2 (Sleep) resume boot.
    BootOnS2Resume = 0x10,
    /// The boot is a S3 (Save to RAM) resume boot. Platforms that support S3 resume must take special care to
    /// preserve/restore memory and critical hardware
    BootOnS3Resume,
    /// This boot mode can be either an INIT, S3, or other means by which to restart the machine. If it is an
    /// S3, for example, the flash update cause will supersede the S3 restart. It is incumbent upon platform
    /// code, such as the Memory Initialization PEIM, to determine the exact cause and perform correct behavior
    /// (i.e., S3 state restoration versus INIT behavior).
    BootOnFlashUpdate,
    /// The boot is in recovery mode. This mode is used to recover from a previous boot failure.
    BootInRecoveryMode = 0x20,
}

// Implement Display for Mode to output a string for each enum variant
impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} ({:#x?})",
            match self {
                Mode::BootWithFullConfiguration => "Boot With Full Configuration",
                Mode::BootWithMinimalConfiguration => "Boot With Minimal Configuration",
                Mode::BootAssumingNoConfigurationChanges => "Boot Assuming No Configuration Changes",
                Mode::BootWithFullConfigurationPlusDiagnostic => "Boot With Full Configuration Plus Diagnostic",
                Mode::BootWithDefaultSettings => "Boot With Default Settings",
                Mode::BootOnS4Resume => "Boot On S4 Resume",
                Mode::BootOnS5Resume => "Boot On S5 Resume",
                Mode::BootWithMfgModeSettings => "Boot With Mfg Mode Settings",
                Mode::BootOnS2Resume => "Boot On S2 Resume",
                Mode::BootOnS3Resume => "Boot On S3 Resume",
                Mode::BootOnFlashUpdate => "Boot On Flash Update",
                Mode::BootInRecoveryMode => "Boot In Recovery Mode",
            },
            *self as u32
        )
    }
}

impl core::convert::TryFrom<u32> for Mode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Mode::BootWithFullConfiguration),
            1 => Ok(Mode::BootWithMinimalConfiguration),
            2 => Ok(Mode::BootAssumingNoConfigurationChanges),
            3 => Ok(Mode::BootWithFullConfigurationPlusDiagnostic),
            4 => Ok(Mode::BootWithDefaultSettings),
            5 => Ok(Mode::BootOnS4Resume),
            6 => Ok(Mode::BootOnS5Resume),
            7 => Ok(Mode::BootWithMfgModeSettings),
            0x10 => Ok(Mode::BootOnS2Resume),
            0x11 => Ok(Mode::BootOnS3Resume),
            0x12 => Ok(Mode::BootOnFlashUpdate),
            0x20 => Ok(Mode::BootInRecoveryMode),
            _ => Err(()),
        }
    }
}

// Add unit tests for Mode
#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::TryFrom;

    #[test]
    fn test_try_from() {
        assert_eq!(Mode::try_from(0x0).unwrap(), Mode::BootWithFullConfiguration);
        assert_eq!(Mode::try_from(0x1).unwrap(), Mode::BootWithMinimalConfiguration);
        assert_eq!(Mode::try_from(0x2).unwrap(), Mode::BootAssumingNoConfigurationChanges);
        assert_eq!(Mode::try_from(0x3).unwrap(), Mode::BootWithFullConfigurationPlusDiagnostic);
        assert_eq!(Mode::try_from(0x4).unwrap(), Mode::BootWithDefaultSettings);
        assert_eq!(Mode::try_from(0x5).unwrap(), Mode::BootOnS4Resume);
        assert_eq!(Mode::try_from(0x6).unwrap(), Mode::BootOnS5Resume);
        assert_eq!(Mode::try_from(0x7).unwrap(), Mode::BootWithMfgModeSettings);
        assert_eq!(Mode::try_from(0x10).unwrap(), Mode::BootOnS2Resume);
        assert_eq!(Mode::try_from(0x11).unwrap(), Mode::BootOnS3Resume);
        assert_eq!(Mode::try_from(0x12).unwrap(), Mode::BootOnFlashUpdate);
        assert_eq!(Mode::try_from(0x20).unwrap(), Mode::BootInRecoveryMode);
        assert!(Mode::try_from(999).is_err());
    }

    #[test]
    fn test_invalid_values() {
        let invalid_values = [
            0x8, 0x9, 0xA, 0xB, 0xC, 0xD, 0xE, 0xF, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
            0x1E, 0x1F, 0x21,
        ];
        for &value in &invalid_values {
            assert!(Mode::try_from(value).is_err());
        }
    }
}
