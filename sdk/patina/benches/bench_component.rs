//! Benchmarks for the component framework.
//!
//! This benchmark tests the performance of adding and running components in the Patina SDK.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina --bench bench_component`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina --bench bench_component -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina --bench bench_component -- with_component
//! > cargo make bench -p patina --bench bench_component -- run_component
//! > cargo make bench -p patina --bench bench_component
//! ```
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0
//!
use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use patina::{
    boot_services::StandardBootServices,
    component::{Component, IntoComponent, Storage, params::*},
    error::Result,
};

#[derive(IntoComponent)]
struct TestComponent;

impl TestComponent {
    fn entry_point(self, _bs: StandardBootServices, _config: Config<i32>) -> Result<()> {
        Ok(())
    }
}

struct Scheduler {
    components: Vec<Box<dyn Component>>,
    storage: Storage,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self { components: Vec::new(), storage: Storage::new() }
    }

    /// Registers a component with the core, that will be dispatched during the driver execution phase.
    pub fn with_component<I>(mut self, component: impl IntoComponent<I>) -> Self {
        let mut component = component.into_component();
        component.initialize(&mut self.storage);
        self.components.push(component);
        self
    }

    pub fn run(&mut self) {
        loop {
            let len = self.components.len();
            self.components.retain_mut(|component| !match component.run(&mut self.storage) {
                Ok(false) => panic!("Did not run in this test"),
                Err(_) => panic!("Failed"),
                Ok(true) => true,
            });
            if self.components.len() == len {
                break;
            }
        }
    }
}

fn add_component_abstracted(b: &mut Bencher<'_>, count: &usize) {
    b.iter_batched(
        Scheduler::new,
        |mut core| {
            for _ in 0..*count {
                core = core.with_component(TestComponent);
            }
        },
        criterion::BatchSize::SmallInput,
    )
}

fn run_component_abstracted(b: &mut Bencher<'_>, count: &usize) {
    let mock_bs = core::mem::MaybeUninit::<r_efi::efi::BootServices>::zeroed();

    let init = |count: usize| -> Scheduler {
        let mut core = Scheduler::new();
        core.storage.set_boot_services(StandardBootServices::new(unsafe { &*mock_bs.as_ptr() }));
        for _ in 0..count {
            core = core.with_component(TestComponent);
        }
        core
    };

    b.iter_batched(
        || init(*count),
        |mut core| {
            core.run();
        },
        criterion::BatchSize::SmallInput,
    )
}

fn add_and_run_component_abstracted(b: &mut Bencher<'_>, count: &usize) {
    let mock_bs = core::mem::MaybeUninit::<r_efi::efi::BootServices>::zeroed();

    let init = || -> Scheduler {
        let mut core = Scheduler::new();
        core.storage.set_boot_services(StandardBootServices::new(unsafe { &*mock_bs.as_ptr() }));
        core
    };

    b.iter_batched(
        init,
        |mut core| {
            for _ in 0..*count {
                core = core.with_component(TestComponent);
            }
            core.run();
        },
        criterion::BatchSize::SmallInput,
    )
}

pub fn benchmark_add_component(c: &mut Criterion) {
    let mut group = c.benchmark_group("with_component");

    group.bench_with_input("add_component_0001", &1_usize, add_component_abstracted);
    group.bench_with_input("add_component_0010", &10_usize, add_component_abstracted);
    group.bench_with_input("add_component_0100", &100_usize, add_component_abstracted);
    group.bench_with_input("add_component_0200", &200_usize, add_component_abstracted);
    group.bench_with_input("add_component_0500", &500_usize, add_component_abstracted);
    group.bench_with_input("add_component_1000", &1000_usize, add_component_abstracted);

    group.finish()
}

pub fn benchmark_run_component(c: &mut Criterion) {
    let mut group = c.benchmark_group("run_component");

    group.bench_with_input("run_0001", &1_usize, run_component_abstracted);
    group.bench_with_input("run_0010", &10_usize, run_component_abstracted);
    group.bench_with_input("run_0100", &100_usize, run_component_abstracted);
    group.bench_with_input("run_0200", &200_usize, run_component_abstracted);
    group.bench_with_input("run_0500", &500_usize, run_component_abstracted);
    group.bench_with_input("run_1000", &1000_usize, run_component_abstracted);
}

pub fn benchmark_add_and_run_component(c: &mut Criterion) {
    let mut group = c.benchmark_group("add_and_run_component");

    group.bench_with_input("add_and_run_0001", &1_usize, add_and_run_component_abstracted);
    group.bench_with_input("add_and_run_0010", &10_usize, add_and_run_component_abstracted);
    group.bench_with_input("add_and_run_0100", &100_usize, add_and_run_component_abstracted);
    group.bench_with_input("add_and_run_0200", &200_usize, add_and_run_component_abstracted);
    group.bench_with_input("add_and_run_0500", &500_usize, add_and_run_component_abstracted);
    group.bench_with_input("add_and_run_1000", &1000_usize, add_and_run_component_abstracted);
}

criterion_group!(benches, benchmark_add_component, benchmark_run_component, benchmark_add_and_run_component);
criterion_main!(benches);
