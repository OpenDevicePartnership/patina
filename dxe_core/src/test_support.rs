//! DXE Core Test Support
//!
//! Code to help support testing.
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use crate::{protocols::PROTOCOL_DB, GCD};
use mu_pi::dxe_services::GcdMemoryType;
use r_efi::efi;

#[macro_export]
macro_rules! test_collateral {
    ($fname:expr) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/resources/test/", $fname)
    };
}

/// A global mutex that can be used for tests to synchronize on access to global state.
/// Usage model is for tests that affect or assert things against global state to acquire this mutex to ensure that
/// other tests run in parallel do not modify or interact with global state non-deterministically.
/// The test should acquire the mutex when it starts to care about or modify global state, and release it when it no
/// longer cares about global state or modifies it (typically this would be the start and end of a test case,
/// respectively).
static GLOBAL_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// default GCD allocation.
const TEST_GCD_MEM_SIZE: usize = 0x1000000;

/// Reset the GCD with a default chunk of memory from the system allocator. This will ensure that the GCD is able
/// to support interactions with other core subsystem (e.g. allocators).
/// Note: for simplicity, this implementation intentionally leaks the memory allocated for the GCD. Expectation is
/// that this should be called few enough times in testing so that this leak does not cause problems.
pub(crate) unsafe fn init_test_gcd(size: Option<usize>) {
    let addr =
        alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(size.unwrap_or(TEST_GCD_MEM_SIZE), 0x1000).unwrap());
    GCD.reset();
    GCD.init(48, 16);
    GCD.add_memory_space(
        GcdMemoryType::SystemMemory,
        addr as usize,
        TEST_GCD_MEM_SIZE,
        efi::MEMORY_UC
            | efi::MEMORY_WC
            | efi::MEMORY_WT
            | efi::MEMORY_WB
            | efi::MEMORY_WP
            | efi::MEMORY_RP
            | efi::MEMORY_XP
            | efi::MEMORY_RO,
    )
    .unwrap();
}

/// Reset and re-initialize the protocol database to default empty state.
pub(crate) unsafe fn init_test_protocol_db() {
    PROTOCOL_DB.reset();
    PROTOCOL_DB.init_protocol_db();
}

/// All tests should run from inside this.
pub(crate) fn with_global_lock<F: Fn()>(f: F) {
    let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap();
    f();
}
