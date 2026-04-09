use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::{CpEdfScheduler, FederatedScheduler, FixedPriorityScheduler};
use hprss_types::{
    BusArbitration, CriticalityLevel, DeviceId, JobId, TaskId,
    device::{DeviceConfig, InterconnectConfig, PreemptionModel, SharedBusConfig},
    task::{ArrivalModel, DagTask, DeviceType, ExecutionTimeModel, SubTask},
};

fn cpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("cpu-{id}"),
        device_group: Some("FT2000".to_string()),
        device_type: DeviceType::Cpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 0,
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
            granularity_ns: 10_000,
        },
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

#[test]
fn fan_in_successor_released_after_both_edges_complete() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 1_000_000,
            seed: 1,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    let dag = DagTask {
        id: TaskId(100),
        name: "fan-in".to_string(),
        arrival: ArrivalModel::Aperiodic,
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        nodes: vec![
            SubTask {
                index: 0,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 10_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 1,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 20_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 2,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 5_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![(0, 64), (1, 64)],
            },
        ],
        edges: vec![(0, 2), (1, 2)],
    };
    engine.register_dags(vec![dag]);

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    assert_eq!(engine.metrics().total_jobs, 3);
    assert_eq!(engine.metrics().completed_jobs, 3);

    let successor = engine
        .get_job(JobId(2))
        .expect("successor node job should be released");
    assert!(
        successor.release_time >= 30_000,
        "fan-in successor should not be released before both predecessors complete"
    );
    let prov = successor
        .dag_provenance
        .expect("successor job should keep DAG provenance");
    assert_eq!(prov.node.0, 2);
}

#[test]
fn edge_transfer_across_devices_releases_successor() {
    let cpu = cpu_device(0);
    let gpu = gpu_device(1);
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 1_000_000,
            seed: 2,
        },
        vec![cpu, gpu],
        vec![InterconnectConfig {
            from: DeviceId(0),
            to: DeviceId(1),
            latency_ns: 2_000,
            bandwidth_bytes_per_ns: 1.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        }],
        vec![SharedBusConfig {
            id: hprss_types::BusId(0),
            name: "unused".to_string(),
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        }],
    );

    let dag = DagTask {
        id: TaskId(200),
        name: "cpu-to-gpu".to_string(),
        arrival: ArrivalModel::Aperiodic,
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        nodes: vec![
            SubTask {
                index: 0,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 8_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 1,
                exec_times: vec![(
                    DeviceType::Gpu,
                    ExecutionTimeModel::Deterministic { wcet: 6_000 },
                )],
                affinity: vec![DeviceType::Gpu],
                data_deps: vec![(0, 128)],
            },
        ],
        edges: vec![(0, 1)],
    };
    engine.register_dags(vec![dag]);

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    assert_eq!(engine.metrics().total_jobs, 2);
    assert_eq!(engine.metrics().completed_jobs, 2);
    assert_eq!(engine.metrics().deadline_misses, 0);
}

#[test]
fn cpedf_executes_dag_chain_without_deadline_miss() {
    let cpu = cpu_device(0);
    let gpu = gpu_device(1);
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 1_000_000,
            seed: 3,
        },
        vec![cpu, gpu],
        vec![InterconnectConfig {
            from: DeviceId(0),
            to: DeviceId(1),
            latency_ns: 2_000,
            bandwidth_bytes_per_ns: 1.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        }],
        vec![SharedBusConfig {
            id: hprss_types::BusId(0),
            name: "unused".to_string(),
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        }],
    );

    let dag = DagTask {
        id: TaskId(220),
        name: "cpedf-chain".to_string(),
        arrival: ArrivalModel::Aperiodic,
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        nodes: vec![
            SubTask {
                index: 0,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 10_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 1,
                exec_times: vec![(
                    DeviceType::Gpu,
                    ExecutionTimeModel::Deterministic { wcet: 6_000 },
                )],
                affinity: vec![DeviceType::Gpu],
                data_deps: vec![(0, 128)],
            },
        ],
        edges: vec![(0, 1)],
    };
    engine.register_dags(vec![dag]);

    let mut scheduler = CpEdfScheduler::default();
    engine.run(&mut scheduler);

    assert_eq!(engine.metrics().total_jobs, 2);
    assert_eq!(engine.metrics().completed_jobs, 2);
    assert_eq!(engine.metrics().deadline_misses, 0);
}

#[test]
fn federated_pins_same_dag_instance_cpu_nodes_to_same_core() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 500_000,
            seed: 4,
        },
        vec![cpu_device(0), cpu_device(1)],
        vec![],
        vec![],
    );

    let dag = DagTask {
        id: TaskId(230),
        name: "federated-fanout".to_string(),
        arrival: ArrivalModel::Aperiodic,
        deadline: 500_000,
        criticality: CriticalityLevel::Lo,
        nodes: vec![
            SubTask {
                index: 0,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 12_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 1,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 12_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
        ],
        edges: vec![],
    };
    engine.register_dags(vec![dag]);

    let mut scheduler = FederatedScheduler::default();
    engine.run(&mut scheduler);

    assert_eq!(engine.metrics().total_jobs, 2);
    assert_eq!(engine.metrics().completed_jobs, 2);
    let job0 = engine.get_job(JobId(0)).expect("job 0 should exist");
    let job1 = engine.get_job(JobId(1)).expect("job 1 should exist");
    assert_eq!(job0.assigned_device, job1.assigned_device);
}
