//! Criterion benchmarks for the DES engine at scale.
//!
//! Measures wall-clock time for a full simulation with various task counts.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_platform::PlatformConfig;
use hprss_scheduler::FixedPriorityScheduler;
use hprss_workload::{WorkloadConfig, generate_taskset};

const PLATFORM_TOML: &str = include_str!("../../../configs/platform_ft2000_full.toml");

fn build_engine(
    num_tasks: usize,
    utilization: f64,
    seed: u64,
) -> (SimEngine, FixedPriorityScheduler) {
    let platform = PlatformConfig::from_toml(PLATFORM_TOML).expect("parse platform");
    let devices = platform.build_devices().expect("build devices");
    let interconnects = platform
        .build_interconnects(&devices)
        .expect("build interconnects");
    let buses = platform.build_buses().expect("build buses");

    let workload_cfg = WorkloadConfig {
        num_tasks,
        total_utilization: utilization,
        seed,
        ..WorkloadConfig::default()
    };
    let tasks = generate_taskset(&workload_cfg, &devices);

    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: platform.duration_ns(),
            seed,
        },
        devices,
        interconnects,
        buses,
    );
    engine.register_tasks(tasks);
    engine.schedule_initial_arrivals();

    (engine, FixedPriorityScheduler)
}

fn bench_engine_run(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine_run");
    group.sample_size(10);

    for &num_tasks in &[30, 100, 250, 500] {
        group.bench_with_input(BenchmarkId::new("tasks", num_tasks), &num_tasks, |b, &n| {
            b.iter_with_setup(
                || build_engine(n, 0.85, 42),
                |(mut engine, mut sched)| {
                    engine.run(&mut sched);
                    engine.events_processed()
                },
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench_engine_run);
criterion_main!(benches);
