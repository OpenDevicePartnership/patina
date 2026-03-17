//! DXE Core Patina Test Audit Tests
//!
//! These tests are intended to audit various states of the Patina DXE Core at the end of boot.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!

use crate::GCD;
use alloc::vec::Vec;
use patina::{test::patina_test, u_assert};
// used in the macro, but not directly referenced; causes a warning if patina tests not enabled.
#[allow(unused)]
use patina::BinaryGuid;
#[allow(unused)]
use r_efi::efi;

// Verify that all adjacent free memory descriptors in the GCD are merged together
#[patina_test]
#[on(event = BinaryGuid(efi::EVENT_GROUP_READY_TO_BOOT))]
#[on(event = BinaryGuid(efi::EVENT_GROUP_EXIT_BOOT_SERVICES))]
fn gcd_free_memory_merged_test() -> patina::test::Result {
    let mut last_desc: Option<patina::pi::dxe_services::MemorySpaceDescriptor> = None;
    let mut descs = Vec::with_capacity(GCD.memory_descriptor_count() * 2);
    GCD.get_memory_descriptors(&mut descs, crate::gcd::DescriptorFilter::Free).map_err(|_| "Can't get descriptors")?;
    for desc in descs {
        // check if the last descriptor and the current descriptor are both free memory descriptors and not part of
        // a memory bin (image_handle != null)
        if let Some(last) = last_desc
            && last.image_handle.is_null()
            && desc.image_handle.is_null()
        {
            u_assert!(last.base_address + last.length != desc.base_address, "Found adjacent free memory descriptors");
        }
        last_desc = Some(desc);
    }

    Ok(())
}
