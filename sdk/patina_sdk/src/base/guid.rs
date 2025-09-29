//! Patina GUID type
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use r_efi::efi;

/// A wrapper type for displaying UEFI GUIDs in a human-readable format.
pub struct Guid<'a>(pub &'a efi::Guid);

impl core::fmt::Display for Guid<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = self.0.as_fields();
        write!(f, "{time_low:08X}-{time_mid:04X}-{time_hi_and_version:04X}-{clk_seq_hi_res:02X}{clk_seq_low:02X}-")?;
        for byte in node.iter() {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for Guid<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", &self)
    }
}
