//! Re-exports of internal modules for the fuzz targets in the repository's `fuzz` directory.
//!
//! This module only exists when cargo-fuzz builds the crate (it sets `--cfg fuzzing`), so normal builds are unchanged
//! and the internal modules stay private. To fuzz another internal module, re-export it from here. See
//! `docs/src/dev/testing/fuzzing.md`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

/// PE/COFF image parsing and loading.
pub mod pecoff {
    pub use crate::pecoff::*;
}
