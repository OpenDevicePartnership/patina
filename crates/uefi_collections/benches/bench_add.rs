use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::Rng;
use std::{collections::HashSet, hash::Hash, mem::size_of};
use uefi_collections::{node_size, Bst, Rbt, SortedSlice};
use uint::construct_uint;

const MAX_SIZE: usize = 4096;

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

pub fn benchmark_add_function(c: &mut Criterion) {
    let mut group = c.benchmark_group("add");
    let nums = random_numbers::<u32>(0, 100_000);
    group.bench_with_input(BenchmarkId::new("rbt", "32bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<u32>()];
            let mut rbt: Rbt<u32> = Rbt::new(&mut mem);

            for i in nums {
                rbt.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "32bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<u32>()];
            let mut bst: Bst<u32> = Bst::new(&mut mem);

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
            let mut rbt: Rbt<i128> = Rbt::new(&mut mem);

            for i in nums {
                rbt.add(*i).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "128bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<i128>()];
            let mut bst: Bst<i128> = Bst::new(&mut mem);

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
            let mut rbt: Rbt<U384> = Rbt::new(&mut mem);

            for i in nums {
                rbt.add((*i).into()).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("bst", "384bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * node_size::<U384>()];
            let mut bst: Bst<U384> = Bst::new(&mut mem);

            for i in nums {
                bst.add((*i).into()).unwrap();
            }
        })
    });

    group.bench_with_input(BenchmarkId::new("sorted_slice", "384bit"), &nums, |b, nums| {
        b.iter(|| {
            let mut mem = [0; MAX_SIZE * size_of::<U384>()];
            let mut ss: SortedSlice<U384> = SortedSlice::new(&mut mem);

            for i in nums {
                ss.add((*i).into()).unwrap();
            }
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_add_function);
criterion_main!(benches);
