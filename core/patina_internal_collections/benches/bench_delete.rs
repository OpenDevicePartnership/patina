//! Benchmarks for the delete operations in various data structures.
//!
//! This benchmark tests the performance performing random delete operations on the supported data structures in this
//! crate, including Red-Black Trees (RBT), Binary Search Trees (BST), and Sorted Slices.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_delete`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina_internal_collections --bench bench_delete -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina_internal_collections --bench bench_delete -- rbt
//! > cargo make bench -p patina_internal_collections --bench bench_delete -- 32bit
//! > cargo make bench -p patina_internal_collections --bench bench_delete
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use patina_internal_collections::{Bst, Rbt, SliceKey, SortedSlice, node_size};
use rand::{Rng, prelude::SliceRandom};
use ruint::Uint;
use std::{collections::HashSet, hash::Hash};

const MAX_SIZE: usize = 4096;
const U32_MAX_SIZE: usize = MAX_SIZE * node_size::<u32>();
const U128_MAX_SIZE: usize = MAX_SIZE * node_size::<u128>();
const U384_MAX_SIZE: usize = MAX_SIZE * node_size::<U384>();

fn mem_u32() -> &'static mut [u8; U32_MAX_SIZE] {
    Box::leak(Box::new([0u8; U32_MAX_SIZE]))
}

fn mem_u128() -> &'static mut [u8; U128_MAX_SIZE] {
    Box::leak(Box::new([0u8; U128_MAX_SIZE]))
}

fn mem_u384() -> &'static mut [u8; U384_MAX_SIZE] {
    Box::leak(Box::new([0u8; U384_MAX_SIZE]))
}

// The size of MemorySpaceDescriptor
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

#[allow(static_mut_refs)]
fn benchmark_delete_function(c: &mut Criterion) {
    let mut group = c.benchmark_group("delete");
    let nums = random_numbers::<u32>(0, 100_000);
    let mut nums_shuffled = nums.clone();
    nums_shuffled.shuffle(&mut rand::thread_rng());
    // RBT 32bit
    group.bench_function(BenchmarkId::new("rbt", "32bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut rbt: Rbt<u32> = Rbt::with_capacity(mem_u32());
                for i in &nums {
                    rbt.add(*i).unwrap();
                }
                rbt
            },
            |rbt| {
                for i in &nums {
                    rbt.delete(i.key()).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // BST 32bit
    group.bench_function(BenchmarkId::new("bst", "32bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut bst: Bst<u32> = Bst::with_capacity(mem_u32());
                for i in &nums {
                    bst.add(*i).unwrap();
                }
                bst
            },
            |bst| {
                for i in &nums_shuffled {
                    match bst.delete(i.key()) {
                        Ok(_) => {}
                        Err(_) => {
                            std::println!("{}", nums.len());
                            std::println!("{nums:?}");
                            std::println!("{}", nums_shuffled.len());
                            std::println!("{nums_shuffled:?}");
                            panic!("Failed to delete {i}");
                        }
                    }
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // SORTED SLICE 32bit
    group.bench_function(BenchmarkId::new("sorted_slice", "32bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut ss: SortedSlice<u32> = SortedSlice::new(mem_u32());
                for i in &nums {
                    ss.add(*i).unwrap();
                }
                ss
            },
            |ss| {
                for i in &nums_shuffled {
                    let idx = ss.search_idx_with_key(i).unwrap();
                    ss.remove_at_idx(idx).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    let nums = random_numbers::<u128>(0, 100_000);
    let mut nums_shuffled = nums.clone();
    nums_shuffled.shuffle(&mut rand::thread_rng());
    // RBT 128bit
    group.bench_function(BenchmarkId::new("rbt", "128bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut rbt: Rbt<u128> = Rbt::with_capacity(mem_u128());
                for i in &nums {
                    rbt.add(*i).unwrap();
                }
                rbt
            },
            |rbt| {
                for i in &nums {
                    rbt.delete(i.key()).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // BST u128bit
    group.bench_function(BenchmarkId::new("bst", "128bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut bst: Bst<u128> = Bst::with_capacity(mem_u128());
                for i in &nums {
                    bst.add(*i).unwrap();
                }
                bst
            },
            |bst| {
                for i in &nums_shuffled {
                    bst.delete(i.key()).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // SORTED SLICE 128bit
    group.bench_function(BenchmarkId::new("sorted_slice", "128bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut ss: SortedSlice<u128> = SortedSlice::new(mem_u128());
                for i in &nums {
                    ss.add(*i).unwrap();
                }
                ss
            },
            |ss| {
                for i in &nums_shuffled {
                    let idx = ss.search_idx_with_key(i).unwrap();
                    ss.remove_at_idx(idx).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    let nums = random_numbers::<u32>(0, 100_000);
    let nums = nums.into_iter().map(|x| Uint::from(x)).collect::<Vec<U384>>();
    let mut nums_shuffled = nums.clone();
    nums_shuffled.shuffle(&mut rand::thread_rng());

    // RBT 384bit
    group.bench_function(BenchmarkId::new("rbt", "384bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut rbt: Rbt<U384> = Rbt::with_capacity(mem_u384());
                for i in &nums {
                    rbt.add(*i).unwrap();
                }
                rbt
            },
            |rbt| {
                for i in &nums {
                    rbt.delete(i.key()).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // // BST 384bit
    group.bench_function(BenchmarkId::new("bst", "384bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut bst: Bst<U384> = Bst::with_capacity(mem_u384());
                for i in &nums {
                    bst.add(*i).unwrap();
                }
                bst
            },
            |bst| {
                for i in &nums_shuffled {
                    bst.delete(i.key()).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    // SORTED SLICE 384bit
    group.bench_function(BenchmarkId::new("sorted_slice", "384bit"), |b| {
        b.iter_batched_ref(
            || {
                let mut ss: SortedSlice<U384> = SortedSlice::new(mem_u384());
                for i in &nums {
                    ss.add(*i).unwrap();
                }
                ss
            },
            |ss| {
                for i in &nums_shuffled {
                    let idx = ss.search_idx_with_key(i).unwrap();
                    ss.remove_at_idx(idx).unwrap();
                }
            },
            criterion::BatchSize::PerIteration,
        );
    });

    group.finish()
}

criterion_group!(benches, benchmark_delete_function);
criterion_main!(benches);
