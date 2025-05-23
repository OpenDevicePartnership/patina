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
