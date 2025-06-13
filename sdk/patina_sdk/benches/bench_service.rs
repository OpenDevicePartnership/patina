//! Benchmarks for the component framework.
//!
//! This benchmark tests the performance of executing services, which can come with some overhead due to either vtable
//! usage (for dynamic services (Service<dyn Trait>)) or due to the need to downcast from `dyn Any` to a concrete type
//! or a trait object.
//!
//! ## Benchmark execution
//!
//! Running this exact benchmark can be done with the following command:
//!
//! `> cargo make bench -p patina_sdk --bench bench_service`
//!
//! If you wish to run a subset of benchmarks in this file, you can filter them by name:
//!
//! `> cargo make bench -p patina_sdk --bench bench_service -- <filter>`
//!
//! ## Examples
//!
//! ```bash
//! > cargo make bench -p patina_sdk --bench bench_service -- dyn
//! > cargo make bench -p patina_sdk --bench bench_service -- concrete
//! > cargo make bench -p patina_sdk --bench bench_service
//! ```
//!
//! ## License
//!
//! Copyright (C) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!
use criterion::{criterion_group, criterion_main, Bencher, Criterion};
use patina_sdk::component::service::Service;
use patina_sdk_macro::IntoService;

trait TestService {
    fn do_something(&self) -> u32;
}

#[derive(IntoService)]
#[service(dyn TestService)]
struct MockService(pub u32);

impl TestService for MockService {
    fn do_something(&self) -> u32 {
        self.0
    }
}

/// Benchmark the cost of indirection that comes with using a dyn service (v-table).
fn execute_dyn_service(b: &mut Bencher<'_>) {
    const VAL: u32 = 42;
    let service: Service<dyn TestService> = Service::mock(Box::new(MockService(VAL)));
    b.iter_batched(|| service.clone(), |s| assert_eq!(s.do_something(), VAL), criterion::BatchSize::SmallInput);
}

/// Benchmark the cost of indirection that comes with using a concrete service (no v-table).
fn execute_concrete_service(b: &mut Bencher<'_>) {
    const VAL: u32 = 42;
    let service: Service<MockService> = Service::mock(Box::new(MockService(VAL)));
    b.iter_batched(|| service.clone(), |s| assert_eq!(s.do_something(), VAL), criterion::BatchSize::SmallInput);
}

pub fn benchmark_service_indirection(c: &mut Criterion) {
    let mut group = c.benchmark_group("service");

    group.bench_function("execute_dyn_service", execute_dyn_service);
    group.bench_function("execute_concrete_service", execute_concrete_service);

    group.finish();
}

criterion_group!(benches, benchmark_service_indirection);
criterion_main!(benches);
