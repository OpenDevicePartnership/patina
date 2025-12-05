//! Compile-fail tests for component parameter validation.
//!
//! These tests verify that the `#[component]` macro correctly detects
//! parameter conflicts at compile time.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

#[test]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/duplicate_config_mut.rs");
    t.compile_fail("tests/ui/config_and_config_mut_conflict.rs");
    t.compile_fail("tests/ui/storage_mut_with_config.rs");
    t.compile_fail("tests/ui/storage_mut_with_config_mut.rs");
    t.compile_fail("tests/ui/storage_with_config_mut.rs");
    t.compile_fail("tests/ui/duplicate_commands.rs");
    t.compile_fail("tests/ui/duplicate_boot_services.rs");
    t.compile_fail("tests/ui/duplicate_runtime_services.rs");
    t.compile_fail("tests/ui/duplicate_storage_mut.rs");
    t.compile_fail("tests/ui/storage_and_storage_mut_conflict.rs");
    t.compile_fail("tests/ui/storage_mut_and_storage_conflict.rs");
    t.pass("tests/ui/multiple_storage_allowed.rs");
}
