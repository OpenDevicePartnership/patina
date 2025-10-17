//! SMBIOS Configuration
//!
//! Configuration types for the SMBIOS component
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// Configuration for SMBIOS service
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbiosConfiguration {
    /// SMBIOS major version (e.g., 3 for SMBIOS 3.x)
    pub major_version: u8,
    /// SMBIOS minor version (e.g., 0 for SMBIOS 3.0)
    pub minor_version: u8,
}

impl Default for SmbiosConfiguration {
    fn default() -> Self {
        Self { major_version: 3, minor_version: 9 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::format;

    #[test]
    fn test_default_configuration() {
        let config = SmbiosConfiguration::default();
        assert_eq!(config.major_version, 3);
        assert_eq!(config.minor_version, 9);
    }

    #[test]
    fn test_custom_configuration() {
        let config = SmbiosConfiguration { major_version: 2, minor_version: 8 };
        assert_eq!(config.major_version, 2);
        assert_eq!(config.minor_version, 8);
    }

    #[test]
    fn test_configuration_clone() {
        let config1 = SmbiosConfiguration { major_version: 3, minor_version: 5 };
        let config2 = config1.clone();
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_configuration_debug() {
        let config = SmbiosConfiguration { major_version: 3, minor_version: 9 };
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("3"));
        assert!(debug_str.contains("9"));
    }
}
