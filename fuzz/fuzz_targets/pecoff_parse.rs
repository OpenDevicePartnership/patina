//! Fuzz target for PE/COFF header parsing.
//!
//! The Patina DXE Core parses every image it loads with [`UefiPeInfo::parse`] before it allocates memory for the
//! image, so this is the first code that sees an attacker-controlled image. [`UefiPeInfo::parse_mapped`] is the same
//! parser for an image that is already loaded into memory. [`UefiPeInfo::validate_sections`] is run on a parsed image
//! next, as the DXE Core does before it allocates the image. None of them may panic for any input.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
#![no_main]

use libfuzzer_sys::fuzz_target;
use patina_dxe_core::pecoff::UefiPeInfo;

fuzz_target!(|data: &[u8]| {
    // The result is intentionally ignored: rejecting the input with an error is correct. Only a panic (or any other
    // abnormal termination) is a finding.
    if let Ok(pe_info) = UefiPeInfo::parse(data) {
        let _ = pe_info.validate_sections();
    }
    let _ = UefiPeInfo::parse_mapped(data);
});
