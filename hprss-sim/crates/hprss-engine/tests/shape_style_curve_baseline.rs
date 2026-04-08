use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::FixedPriorityScheduler;
use hprss_types::{
    DeviceId,
    device::{DeviceConfig, PreemptionModel},
    task::DeviceType,
};
use hprss_workload::{WorkloadConfig, generate_taskset};

#[derive(Debug, Clone)]
struct CurvePoint {
    utilization: f64,
    schedulability_ratio: f64,
}

fn cpu_device() -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(0),
        name: "cpu-0".to_string(),
        device_group: None,
        device_type: DeviceType::Cpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 1_000,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn run_single_seed(utilization: f64, seed: u64) -> bool {
    let devices = vec![cpu_device()];
    let tasks = generate_taskset(
        &WorkloadConfig {
            num_tasks: 12,
            total_utilization: utilization,
            period_range_ms: (1, 4),
            seed,
        },
        &devices,
    );

    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 40_000_000,
            seed,
        },
        devices,
        vec![],
        vec![],
    );
    engine.register_tasks(tasks);
    engine.schedule_initial_arrivals();

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);
    let summary = engine.summary();
    summary.deadline_misses == 0 && summary.completed_jobs == summary.total_jobs
}

fn shape_style_fp_baseline_curve() -> Vec<CurvePoint> {
    // SHAPE's exact algorithm/workload model is not implemented in this repository yet.
    // This test provides a deterministic, in-repo baseline inspired by SHAPE-style
    // schedulability-rate curves: sweep utilization points and compute the
    // schedulable ratio over a fixed seed set. In this baseline, a sample is
    // considered schedulable only if it has zero deadline misses and no unfinished
    // jobs at the end of the simulation horizon.
    const UTILIZATION_POINTS: [f64; 5] = [0.4, 0.7, 1.0, 1.3, 1.6];
    const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

    UTILIZATION_POINTS
        .iter()
        .map(|&utilization| {
            let schedulable_runs = SEEDS
                .clone()
                .filter(|&seed| run_single_seed(utilization, seed))
                .count();

            CurvePoint {
                utilization,
                schedulability_ratio: schedulable_runs as f64 / SEEDS.clone().count() as f64,
            }
        })
        .collect()
}

#[test]
fn shape_style_schedulability_curve_baseline_is_deterministic() {
    let first = shape_style_fp_baseline_curve();
    let second = shape_style_fp_baseline_curve();

    assert_eq!(first.len(), second.len());
    for (lhs, rhs) in first.iter().zip(second.iter()) {
        assert!((lhs.utilization - rhs.utilization).abs() < f64::EPSILON);
        assert!((lhs.schedulability_ratio - rhs.schedulability_ratio).abs() < f64::EPSILON);
    }
}

#[test]
fn shape_style_schedulability_curve_baseline_degrades_with_higher_load() {
    let curve = shape_style_fp_baseline_curve();

    for point in &curve {
        assert!(
            (0.0..=1.0).contains(&point.schedulability_ratio),
            "ratio must stay within [0, 1], got {:?}",
            point
        );
    }

    let low_load = curve
        .first()
        .expect("curve should have at least one utilization point");
    let high_load = curve
        .last()
        .expect("curve should have at least one utilization point");

    assert!(
        low_load.schedulability_ratio > high_load.schedulability_ratio,
        "expected lower schedulability at higher utilization; curve={curve:?}"
    );

    let drops = curve
        .windows(2)
        .filter(|pair| pair[1].schedulability_ratio < pair[0].schedulability_ratio)
        .count();
    assert!(
        drops >= 1,
        "expected at least one downward step; curve={curve:?}"
    );
}
