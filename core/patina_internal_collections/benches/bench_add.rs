//! Benchmarks for the add operations in various data structures.
//!
//! This benchmark tests the performance performing random add operations on the supported data structures in this
//! crate, including Red-Black Trees (RBT), Binary Search Trees (BST), and Sorted Slices.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_add`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_add -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina_internal_collections --bench bench_add -- rbt
//! > cargo make bench -p patina_internal_collections --bench bench_add -- 32bit
//! > cargo make bench -p patina_internal_collections --bench bench_add
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use patina_internal_collections::{Bst, Rbt, SortedSlice, node_size};
use rand::Rng;
use ruint::Uint;
use std::{collections::HashSet, hash::Hash, mem::size_of};

const MAX_SIZE: usize = 4096;

type U384 = Uint<384, 6>;

fn random_numbers<D>(min: D, max: D) -> Vec<D>
where
    D: Copy + Eq + std::cmp::PartialOrd + Hash + rand::distributions::uniform::SampleUniform,
{
    let mut rng = rand::thread_rng();
    let mut nums: HashSet<D> = HashSet::new();
    while nums.len() < MAX_SIZE {
        let num: D = rng.gen_range(min..=max);
        nums.insert(num);
    }
    nums.into_iter().collect()
}

pub fn benchmark_add_function(c: &mut Criterion) {
    let mut group = c.benchmark_group("add");
    let nums = random_numbers::<u32>(0, 100_000);
    group.bench_with_input(BenchmarkId::new("rbt", "32bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<u32>()];
            let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem);

            for i in nums {
                rbt.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "32bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<u32>()];
            let mut bst: Bst<u32> = Bst::with_capacity(&mut mem);

            for i in nums {
                bst.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("sorted_slice", "32bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * size_of::<u32>()];
            let mut ss: SortedSlice<u32> = SortedSlice::new(&mut mem);

            for i in nums {
                ss.add(*i).unwrap();
            }
        })
    });

    let nums = random_numbers::<i128>(0, 100_000);

    group.bench_with_input(BenchmarkId::new("rbt", "128bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<i128>()];
            let mut rbt: Rbt<i128> = Rbt::with_capacity(&mut mem);

            for i in nums {
                rbt.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "128bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<i128>()];
            let mut bst: Bst<i128> = Bst::with_capacity(&mut mem);

            for i in nums {
                bst.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("sorted_slice", "128bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * size_of::<i128>()];
            let mut ss: SortedSlice<i128> = SortedSlice::new(&mut mem);

            for i in nums {
                ss.add(*i).unwrap();
            }
        })
    });

    let nums = random_numbers::<u32>(0, 100_000);

    group.bench_with_input(BenchmarkId::new("rbt", "384bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<U384>()];
            let mut rbt: Rbt<U384> = Rbt::with_capacity(&mut mem);

            for i in nums {
                rbt.add(Uint::from(*i)).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "384bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<U384>()];
            let mut bst: Bst<U384> = Bst::with_capacity(&mut mem);

            for i in nums {
                bst.add(Uint::from(*i)).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("sorted_slice", "384bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * size_of::<U384>()];
            let mut ss: SortedSlice<U384> = SortedSlice::new(&mut mem);

            for i in nums {
                ss.add(Uint::from(*i)).unwrap();
            }
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_add_function);
criterion_main!(benches);
