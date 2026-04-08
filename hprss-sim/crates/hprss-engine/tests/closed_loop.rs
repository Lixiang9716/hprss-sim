use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::FixedPriorityScheduler;
use hprss_types::{
    BusArbitration, CriticalityLevel, DeviceId, InterconnectConfig, JobId, SharedBusConfig,
    TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DeviceType, ExecutionTimeModel, Task},
};

fn cpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("cpu-{id}"),
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

fn gpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("gpu-{id}"),
        device_group: None,
        device_type: DeviceType::Gpu,
        cores: 1,
        preemption: PreemptionModel::LimitedPreemptive {
            granularity_ns: 20_000,
        },
        context_switch_ns: 5_000,
        speed_factor: 4.0,
        multicore_policy: None,
        power_watts: None,
    }
}

#[test]
fn cpu_task_releases_and_completes() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 1_000_000,
            seed: 7,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![Task {
        id: TaskId(0),
        name: "cpu-periodic".to_string(),
        priority: 1,
        arrival: ArrivalModel::Periodic { period: 500_000 },
        deadline: 500_000,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![(
            DeviceType::Cpu,
            ExecutionTimeModel::Deterministic { wcet: 100_000 },
        )],
        affinity: vec![DeviceType::Cpu],
        data_size: 0,
    }]);
    engine.schedule_initial_arrivals();

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    assert!(engine.metrics().total_jobs >= 1);
    assert!(engine.metrics().completed_jobs >= 1);
    assert_eq!(engine.metrics().deadline_misses, 0);
}

#[test]
fn accelerator_task_transfer_and_completion() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 2_000_000,
            seed: 13,
        },
        vec![cpu_device(0), gpu_device(1)],
        vec![InterconnectConfig {
            from: DeviceId(0),
            to: DeviceId(1),
            latency_ns: 2_000,
            bandwidth_bytes_per_ns: 2.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        }],
        vec![SharedBusConfig {
            id: hprss_types::BusId(0),
            name: "unused".to_string(),
            total_bandwidth_bytes_per_ns: 2.0,
            arbitration: BusArbitration::RoundRobin,
        }],
    );
    engine.register_tasks(vec![Task {
        id: TaskId(0),
        name: "gpu-periodic".to_string(),
        priority: 1,
        arrival: ArrivalModel::Periodic { period: 1_000_000 },
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![
            (
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 160_000 },
            ),
            (
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 40_000 },
            ),
        ],
        affinity: vec![DeviceType::Gpu],
        data_size: 4_096,
    }]);
    engine.schedule_initial_arrivals();

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    assert!(engine.metrics().completed_jobs >= 1);
    let job = engine
        .get_job(JobId(0))
        .expect("first released job should exist");
    assert_eq!(
        job.actual_exec_ns,
        Some(40_000),
        "actual exec time should match target GPU model"
    );
}

#[test]
fn four_cpu_cores_can_run_four_jobs_in_parallel() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 200_000,
            seed: 21,
        },
        vec![cpu_device(0), cpu_device(1), cpu_device(2), cpu_device(3)],
        vec![],
        vec![],
    );
    let tasks = (0..4)
        .map(|i| Task {
            id: TaskId(i),
            name: format!("cpu-par-{i}"),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 200_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 80_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        })
        .collect();
    engine.register_tasks(tasks);
    engine.schedule_initial_arrivals();

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    assert_eq!(engine.metrics().total_jobs, 4);
    assert_eq!(engine.metrics().completed_jobs, 4);
    assert_eq!(engine.metrics().deadline_misses, 0);
}
