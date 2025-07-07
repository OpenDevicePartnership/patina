//! Benchmarks for the search operations in various data structures.
//!
//! This benchmark tests the performance performing random search operations on the supported data structures in this
//! crate, including Red-Black Trees (RBT), Binary Search Trees (BST), and Sorted Slices.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_search`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_search -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina_internal_collections --bench bench_search -- rbt
//! > cargo make bench -p patina_internal_collections --bench bench_search -- 32bit
//! > cargo make bench -p patina_internal_collections --bench bench_search
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use patina_internal_collections::{node_size, Bst, Rbt, SortedSlice};
use rand::Rng;
use std::{collections::HashSet, hash::Hash, mem::size_of};
use uint::construct_uint;

const MAX_SIZE: usize = 200;

// The size of MemorySpaceDescriptor
construct_uint! {
    pub struct U384(6);
}

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

fn benchmark_search_function(c: &mut Criterion) {
    let mut group = c.benchmark_group("search");
    let nums = random_numbers::<u32>(0, 100_000);

    // RBT 32bit
    let mut mem = [0; MAX_SIZE * node_size::<u32>()];
    let mut rbt: Rbt<u32> = Rbt::with_capacity(&mut mem);
    for i in &nums {
        rbt.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("rbt", "32bit"), &rbt, |b, rbt| {
        b.iter(|| {
            for i in &nums {
                rbt.get(i).unwrap();
            }
        })
    });

    // BST 32bit
    let mut mem = [0; MAX_SIZE * node_size::<u32>()];
    let mut bst: Bst<u32> = Bst::with_capacity(&mut mem);
    for i in &nums {
        bst.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("bst", "32bit"), &bst, |b, bst| {
        b.iter(|| {
            for i in &nums {
                bst.get(i).unwrap();
            }
        })
    });

    // SORTED SLICE 32bit
    let mut mem = [0; MAX_SIZE * size_of::<u32>()];
    let mut ss: SortedSlice<u32> = SortedSlice::new(&mut mem);
    for i in &nums {
        ss.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("sorted_slice", "32bit"), &ss, |b, ss| {
        b.iter(|| {
            for i in &nums {
                ss.search_with_key(i).unwrap();
            }
        })
    });

    // 128bit nums
    let nums = random_numbers::<i128>(0, 100_000);

    // RBT 128bit
    let mut mem = [0; MAX_SIZE * node_size::<i128>()];
    let mut rbt: Rbt<i128> = Rbt::with_capacity(&mut mem);
    for i in &nums {
        rbt.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("rbt", "128bit"), &rbt, |b, rbt| {
        b.iter(|| {
            for i in &nums {
                rbt.get(i).unwrap();
            }
        })
    });

    // BST 128bit
    let mut mem = [0; MAX_SIZE * node_size::<i128>()];
    let mut bst: Bst<i128> = Bst::with_capacity(&mut mem);
    for i in &nums {
        bst.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("bst", "128bit"), &bst, |b, bst| {
        b.iter(|| {
            for i in &nums {
                bst.get(i).unwrap();
            }
        })
    });

    // SORTED SLICE 128bit
    let mut mem = [0; MAX_SIZE * size_of::<i128>()];
    let mut ss: SortedSlice<i128> = SortedSlice::new(&mut mem);
    for i in &nums {
        ss.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("sorted_slice", "128bit"), &ss, |b, ss| {
        b.iter(|| {
            for i in &nums {
                ss.search_with_key(i).unwrap();
            }
        })
    });

    // u64 nums (converted into 384bit)
    let nums = random_numbers::<u32>(0, 100_000);
    let nums = nums.into_iter().map(|x| x.into()).collect::<Vec<U384>>();

    // RBT 384bit
    let mut mem = [0; MAX_SIZE * node_size::<U384>()];
    let mut rbt: Rbt<U384> = Rbt::with_capacity(&mut mem);

    for i in &nums {
        rbt.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("rbt", "384bit"), &rbt, |b, rbt| {
        b.iter(|| {
            for i in &nums {
                rbt.get(i).unwrap();
            }
        })
    });

    // BST 384bit
    let mut mem = [0; MAX_SIZE * node_size::<U384>()];
    let mut bst: Bst<U384> = Bst::with_capacity(&mut mem);
    for i in &nums {
        bst.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("bst", "384bit"), &bst, |b, bst| {
        b.iter(|| {
            for i in &nums {
                bst.get(i).unwrap();
            }
        })
    });

    // SORTED SLICE 384bit
    let mut mem = [0; MAX_SIZE * size_of::<U384>()];
    let mut ss: SortedSlice<U384> = SortedSlice::new(&mut mem);
    for i in &nums {
        ss.add(*i).unwrap();
    }
    group.bench_with_input(BenchmarkId::new("sorted_slice", "384bit"), &ss, |b, ss| {
        b.iter(|| {
            for i in &nums {
                ss.search_with_key(i).unwrap();
            }
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_search_function);
criterion_main!(benches);
