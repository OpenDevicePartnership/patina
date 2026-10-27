//! UEFI and Tiano decompression benchmarks.
//!
//! Run with `cargo make bench -p patina --bench bench_decompress`.
//!
//! Copyright (c) Microsoft Corporation.
//! SPDX-License-Identifier: Apache-2.0

use core::hint::black_box;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use patina::uefi::decompress::{DecompressionAlgorithm, decompress_into_with_algo};

fn bench_decompress(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompress");
    for (name, compressed, original, tiano) in [
        (
            "uefi",
            include_bytes!("../test_resources/uefi_compressed.bin").as_slice(),
            include_bytes!("../test_resources/uefi_uncompressed.bin").as_slice(),
            false,
        ),
        (
            "tiano",
            include_bytes!("../test_resources/tiano_compressed.bin").as_slice(),
            include_bytes!("../test_resources/tiano_uncompressed.bin").as_slice(),
            true,
        ),
    ] {
        let mut output = vec![0u8; original.len()];
        group.throughput(Throughput::Bytes(original.len() as u64));
        group.bench_function(name, |b| {
            b.iter(|| {
                let algorithm = if tiano {
                    DecompressionAlgorithm::TianoDecompress
                } else {
                    DecompressionAlgorithm::UefiDecompress
                };
                decompress_into_with_algo(black_box(compressed), black_box(&mut output), algorithm)
                    .expect("valid compressed fixture");
                black_box(&output);
            });
        });
        assert_eq!(output, original);
    }
    group.finish();
}

criterion_group!(benches, bench_decompress);
criterion_main!(benches);
