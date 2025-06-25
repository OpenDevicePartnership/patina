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
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use patina_internal_collections::{node_size, Bst, Rbt, SliceKey, SortedSlice};
use rand::{prelude::SliceRandom, Rng};
use std::{collections::HashSet, hash::Hash};
use uint::construct_uint;

const MAX_SIZE: usize = 4096;

static mut MEM_U32: [u8; 131072] = [0; MAX_SIZE * node_size::<u32>()];
static mut MEM_U128: [u8; 196608] = [0; MAX_SIZE * node_size::<u128>()];
static mut MEM_U384: [u8; 327680] = [0; MAX_SIZE * node_size::<U384>()];

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
                unsafe {
                    MEM_U32.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut rbt: Rbt<u32> = Rbt::with_capacity(unsafe { &mut MEM_U32 });
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
                unsafe {
                    MEM_U32.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut bst: Bst<u32> = Bst::with_capacity(unsafe { &mut MEM_U32 });
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
                            std::println!("{:?}", nums);
                            std::println!("{}", nums_shuffled.len());
                            std::println!("{:?}", nums_shuffled);
                            panic!("lol")
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
                unsafe {
                    MEM_U32.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut ss: SortedSlice<u32> = SortedSlice::new(unsafe { &mut MEM_U32 });
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
                unsafe {
                    MEM_U128.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut rbt: Rbt<u128> = Rbt::with_capacity(unsafe { &mut MEM_U128 });
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
                unsafe {
                    MEM_U128.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut bst: Bst<u128> = Bst::with_capacity(unsafe { &mut MEM_U128 });
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
                unsafe {
                    MEM_U128.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut ss: SortedSlice<u128> = SortedSlice::new(unsafe { &mut MEM_U128 });
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
    let nums = nums.into_iter().map(|x| x.into()).collect::<Vec<U384>>();
    let mut nums_shuffled = nums.clone();
    nums_shuffled.shuffle(&mut rand::thread_rng());

    // RBT 384bit
    group.bench_function(BenchmarkId::new("rbt", "384bit"), |b| {
        b.iter_batched_ref(
            || {
                unsafe {
                    MEM_U384.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut rbt: Rbt<U384> = Rbt::with_capacity(unsafe { &mut MEM_U384 });
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
                unsafe {
                    MEM_U384.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut bst: Bst<U384> = Bst::with_capacity(unsafe { &mut MEM_U384 });
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
                unsafe {
                    MEM_U384.fill(0);
                }
                #[allow(static_mut_refs)]
                let mut ss: SortedSlice<U384> = SortedSlice::new(unsafe { &mut MEM_U384 });
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
