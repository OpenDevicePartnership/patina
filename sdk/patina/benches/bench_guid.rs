//! Benchmarks for GUID operations.
//!
//! This benchmark compares the performance of patina::Guid wrapper operations
//! against r_efi::efi::Guid operations to measure performance delta.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina --bench bench_guid`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina --bench bench_guid -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina --bench bench_guid -- guid_creation
//! > cargo make bench -p patina --bench bench_guid -- guid_display
//! > cargo make bench -p patina --bench bench_guid -- guid_comparison
//! > cargo make bench -p patina --bench bench_guid -- guid_complex_operations
//! > cargo make bench -p patina --bench bench_guid
//! ```
//!
//! ## Benchmark Categories
//!
//! - **guid_creation**: Tests creation performance from r_efi references and string parsing
//! - **guid_display**: Tests string formatting performance between wrapper and manual formatting
//! - **guid_comparison**: Tests equality comparison performance for GUIDs that are equal and not equal
//! - **guid_complex_operations**: Tests performance of combined create-and-format operations
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use patina::base::guid::{Guid, OwnedGuid};
use r_efi::efi;

const TEST_GUID_STRING: &str = "12345678-9abc-def0-1122-334455667788";

fn create_r_efi_guid() -> efi::Guid {
    efi::Guid::from_fields(0x12345678, 0x9abc, 0xdef0, 0x11, 0x22, &[0x33, 0x44, 0x55, 0x66, 0x77, 0x88])
}

// Creation benchmarks
fn bench_patina_from_ref(b: &mut Bencher<'_>, _input: &usize) {
    let r_efi_guid = create_r_efi_guid();
    b.iter(|| Guid::from(&r_efi_guid))
}

fn bench_patina_try_from_string(b: &mut Bencher<'_>, _input: &usize) {
    b.iter(|| OwnedGuid::try_from_string(TEST_GUID_STRING).expect("Valid GUID"))
}

fn bench_r_efi_direct(b: &mut Bencher<'_>, _input: &usize) {
    b.iter(create_r_efi_guid)
}

// Display benchmarks
fn bench_patina_format(b: &mut Bencher<'_>, _input: &usize) {
    let patina_guid = OwnedGuid::try_from_string(TEST_GUID_STRING).expect("Valid GUID");
    b.iter(|| format!("{}", patina_guid))
}

fn bench_r_efi_manual_format(b: &mut Bencher<'_>, _input: &usize) {
    let r_efi_guid = create_r_efi_guid();
    b.iter(|| {
        let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = r_efi_guid.as_fields();
        format!(
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            time_low,
            time_mid,
            time_hi_and_version,
            clk_seq_hi_res,
            clk_seq_low,
            node[0],
            node[1],
            node[2],
            node[3],
            node[4],
            node[5]
        )
    })
}

// Comparison benchmarks
fn bench_patina_eq_same(b: &mut Bencher<'_>, _input: &usize) {
    let patina_guid1 = OwnedGuid::try_from_string(TEST_GUID_STRING).expect("Valid GUID");
    let patina_guid2 = OwnedGuid::try_from_string(TEST_GUID_STRING).expect("Valid GUID");
    b.iter(|| patina_guid1 == patina_guid2)
}

fn bench_patina_eq_different(b: &mut Bencher<'_>, _input: &usize) {
    let patina_guid1 = OwnedGuid::try_from_string(TEST_GUID_STRING).expect("Valid GUID");
    let patina_guid_different = OwnedGuid::try_from_string("00000000-0000-0000-0000-000000000000").expect("Valid GUID");
    b.iter(|| patina_guid1 == patina_guid_different)
}

fn bench_r_efi_eq_same(b: &mut Bencher<'_>, _input: &usize) {
    let r_efi_guid1 = create_r_efi_guid();
    let r_efi_guid2 = create_r_efi_guid();
    b.iter(|| r_efi_guid1 == r_efi_guid2)
}

fn bench_r_efi_eq_different(b: &mut Bencher<'_>, _input: &usize) {
    let r_efi_guid1 = create_r_efi_guid();
    let r_efi_guid_different = efi::Guid::from_fields(0, 0, 0, 0, 0, &[0; 6]);
    b.iter(|| r_efi_guid1 == r_efi_guid_different)
}

// Complex operation benchmarks
fn bench_patina_complex(b: &mut Bencher<'_>, input: &usize) {
    let count = *input;
    b.iter(|| {
        for i in 0..count {
            let guid_str = format!("{:08x}-0000-0000-0000-000000000000", i);
            let guid = OwnedGuid::try_from_string(&guid_str).expect("Valid GUID");
            let _formatted = format!("{}", guid);
        }
    })
}

fn bench_r_efi_complex(b: &mut Bencher<'_>, input: &usize) {
    let count = *input;
    b.iter(|| {
        for i in 0..count {
            let guid = efi::Guid::from_fields(i as u32, 0, 0, 0, 0, &[0; 6]);
            let (time_low, time_mid, time_hi_and_version, clk_seq_hi_res, clk_seq_low, node) = guid.as_fields();
            let _formatted = format!(
                "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                time_low,
                time_mid,
                time_hi_and_version,
                clk_seq_hi_res,
                clk_seq_low,
                node[0],
                node[1],
                node[2],
                node[3],
                node[4],
                node[5]
            );
        }
    })
}

pub fn benchmark_guid_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("guid_creation");

    group.bench_with_input("patina_from_ref", &1_usize, bench_patina_from_ref);
    group.bench_with_input("patina_try_from_string", &1_usize, bench_patina_try_from_string);
    group.bench_with_input("r_efi_direct", &1_usize, bench_r_efi_direct);
}

pub fn benchmark_guid_display(c: &mut Criterion) {
    let mut group = c.benchmark_group("guid_display");

    group.bench_with_input("patina_format", &1_usize, bench_patina_format);
    group.bench_with_input("r_efi_manual_format", &1_usize, bench_r_efi_manual_format);
}

pub fn benchmark_guid_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("guid_comparison");

    group.bench_with_input("patina_eq_same", &1_usize, bench_patina_eq_same);
    group.bench_with_input("patina_eq_different", &1_usize, bench_patina_eq_different);
    group.bench_with_input("r_efi_eq_same", &1_usize, bench_r_efi_eq_same);
    group.bench_with_input("r_efi_eq_different", &1_usize, bench_r_efi_eq_different);
}

pub fn benchmark_guid_complex_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("guid_complex_operations");

    group.bench_with_input("patina_complex_10", &10_usize, bench_patina_complex);
    group.bench_with_input("patina_complex_100", &100_usize, bench_patina_complex);
    group.bench_with_input("patina_complex_1000", &1000_usize, bench_patina_complex);

    group.bench_with_input("r_efi_complex_10", &10_usize, bench_r_efi_complex);
    group.bench_with_input("r_efi_complex_100", &100_usize, bench_r_efi_complex);
    group.bench_with_input("r_efi_complex_1000", &1000_usize, bench_r_efi_complex);
}

criterion_group!(
    benches,
    benchmark_guid_creation,
    benchmark_guid_display,
    benchmark_guid_comparison,
    benchmark_guid_complex_operations
);
criterion_main!(benches);
