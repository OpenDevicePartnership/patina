//! Configuration for ESRT component
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
//! **Phase 1 - Issue #2: Define Configuration Structure**
//!
//! TODO: Developer 1 - Implement the following:
//!
//! 1. Define `EsrtConfig` struct with fields:
//!    - enable_component: bool (to support conditional initialization)
//!    - max_fmp_entries: u32
//!    - max_non_fmp_entries: u32
//!
//! 2. Implement `Default` trait with sensible defaults:
//!    - enable_component: true
//!    - max_fmp_entries: 32
//!    - max_non_fmp_entries: 32
//!
//! 3. Add `validate()` method:
//!    - Ensure 0 < max_fmp_entries <= 256
//!    - Ensure 0 < max_non_fmp_entries <= 256
//!    - Return appropriate errors
//!
//! 4. Add unit tests:
//!    - Test default configuration
//!    - Test validation with valid values
//!    - Test validation with invalid values (0, >256)

// Minimal stub to allow compilation - Developer 1 should replace this

/// TODO: Developer 1 - Replace this stub with full implementation
pub struct EsrtConfig {
    /// Maximum number of FMP (Firmware Management Protocol) entries allowed in the ESRT table
    pub max_fmp_entries: u32,
}

impl Default for EsrtConfig {
    #[coverage(off)]
    fn default() -> Self {
        Self { max_fmp_entries: 32 }
    }
}

#[cfg(test)]
mod tests {
    // TODO: Developer 1 - Add unit tests here
}
