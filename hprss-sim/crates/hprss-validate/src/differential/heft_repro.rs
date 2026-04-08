use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::{FixedPriorityScheduler, HeftScheduler};
use hprss_types::{
    BusArbitration, BusId, CriticalityLevel, DagInstanceId, DeviceId, InterconnectConfig,
    SharedBusConfig, TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DagTask, DeviceType, ExecutionTimeModel, SubTask},
};

/// Scope of HEFT makespan reproduction against a simple baseline scheduler.
pub const HEFT_REPRO_SCOPE: &str = "HEFT Topcuoglu-style makespan trend repro";

#[derive(Debug, Clone)]
pub struct HeftReproWorkload {
    pub name: &'static str,
    pub horizon: u64,
    pub dag: DagTask,
    pub devices: Vec<DeviceConfig>,
    pub interconnects: Vec<InterconnectConfig>,
    pub buses: Vec<SharedBusConfig>,
    pub expected_fp_makespan: u64,
    pub expected_heft_makespan: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeftFpMakespanReproReport {
    pub scope: &'static str,
    pub workload: &'static str,
    pub fp_makespan: u64,
    pub heft_makespan: u64,
    pub heft_speedup: f64,
}

pub fn selected_heft_repro_workloads() -> Vec<HeftReproWorkload> {
    vec![gpu_friendly_chain_workload(), mixed_device_chain_workload()]
}

pub fn run_heft_fp_makespan_repro(workload: &HeftReproWorkload) -> HeftFpMakespanReproReport {
    let fp_makespan = run_with_fp(workload);
    let heft_makespan = run_with_heft(workload);
    let heft_speedup = if heft_makespan == 0 {
        f64::INFINITY
    } else {
        fp_makespan as f64 / heft_makespan as f64
    };

    HeftFpMakespanReproReport {
        scope: HEFT_REPRO_SCOPE,
        workload: workload.name,
        fp_makespan,
        heft_makespan,
        heft_speedup,
    }
}

fn run_with_fp(workload: &HeftReproWorkload) -> u64 {
    let mut engine = new_engine(workload);
    engine.register_dags(vec![workload.dag.clone()]);
    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);
    engine.summary().makespan
}

fn run_with_heft(workload: &HeftReproWorkload) -> u64 {
    let mut engine = new_engine(workload);
    engine.register_dags(vec![workload.dag.clone()]);

    let mut scheduler = HeftScheduler::default();
    let plan = scheduler
        .planner
        .build_plan(&workload.dag, &workload.devices)
        .clone();
    scheduler.set_instance_plan(DagInstanceId(0), plan);
    engine.run(&mut scheduler);
    engine.summary().makespan
}

fn new_engine(workload: &HeftReproWorkload) -> SimEngine {
    SimEngine::new(
        SimConfig {
            duration_ns: workload.horizon,
            seed: 7,
        },
        workload.devices.clone(),
        workload.interconnects.clone(),
        workload.buses.clone(),
    )
}

fn gpu_friendly_chain_workload() -> HeftReproWorkload {
    let devices = standard_cpu_gpu_devices();
    HeftReproWorkload {
        name: "gpu-friendly-chain",
        horizon: 1_000_000,
        dag: DagTask {
            id: TaskId(700),
            name: "gpu-friendly-chain".to_string(),
            arrival: ArrivalModel::Aperiodic,
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            nodes: vec![
                dual_affinity_node(0, 90_000, 20_000, vec![]),
                dual_affinity_node(1, 80_000, 20_000, vec![(0, 10)]),
                dual_affinity_node(2, 70_000, 20_000, vec![(1, 10)]),
            ],
            edges: vec![(0, 1), (1, 2)],
        },
        interconnects: bidirectional_links(),
        buses: vec![SharedBusConfig {
            id: BusId(0),
            name: "unused".to_string(),
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        }],
        expected_fp_makespan: 240_000,
        expected_heft_makespan: 15_020,
        devices,
    }
}

fn mixed_device_chain_workload() -> HeftReproWorkload {
    let devices = standard_cpu_gpu_devices();
    HeftReproWorkload {
        name: "mixed-device-chain",
        horizon: 1_000_000,
        dag: DagTask {
            id: TaskId(701),
            name: "mixed-device-chain".to_string(),
            arrival: ArrivalModel::Aperiodic,
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            nodes: vec![
                dual_affinity_node(0, 120_000, 30_000, vec![]),
                dual_affinity_node(1, 15_000, 100_000, vec![(0, 10)]),
                dual_affinity_node(2, 110_000, 25_000, vec![(1, 10)]),
            ],
            edges: vec![(0, 1), (1, 2)],
        },
        interconnects: bidirectional_links(),
        buses: vec![SharedBusConfig {
            id: BusId(0),
            name: "unused".to_string(),
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        }],
        expected_fp_makespan: 245_000,
        expected_heft_makespan: 28_760,
        devices,
    }
}

fn standard_cpu_gpu_devices() -> Vec<DeviceConfig> {
    vec![
        DeviceConfig {
            id: DeviceId(0),
            name: "cpu-0".to_string(),
            device_group: Some("FT2000".to_string()),
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption: PreemptionModel::FullyPreemptive,
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        },
        DeviceConfig {
            id: DeviceId(1),
            name: "gpu-0".to_string(),
            device_group: Some("GP201".to_string()),
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive {
                granularity_ns: 10_000,
            },
            context_switch_ns: 0,
            speed_factor: 4.0,
            multicore_policy: None,
            power_watts: None,
        },
    ]
}

fn bidirectional_links() -> Vec<InterconnectConfig> {
    vec![
        InterconnectConfig {
            from: DeviceId(0),
            to: DeviceId(1),
            latency_ns: 0,
            bandwidth_bytes_per_ns: 1.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        },
        InterconnectConfig {
            from: DeviceId(1),
            to: DeviceId(0),
            latency_ns: 0,
            bandwidth_bytes_per_ns: 1.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        },
    ]
}

fn dual_affinity_node(
    index: usize,
    cpu_wcet: u64,
    gpu_wcet: u64,
    data_deps: Vec<(usize, u64)>,
) -> SubTask {
    SubTask {
        index,
        exec_times: vec![
            (
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: cpu_wcet },
            ),
            (
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: gpu_wcet },
            ),
        ],
        affinity: vec![DeviceType::Cpu, DeviceType::Gpu],
        data_deps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_workloads_show_expected_heft_advantage() {
        let workloads = selected_heft_repro_workloads();
        assert_eq!(workloads.len(), 2);

        for workload in &workloads {
            let report = run_heft_fp_makespan_repro(workload);
            assert_eq!(report.fp_makespan, workload.expected_fp_makespan);
            assert_eq!(report.heft_makespan, workload.expected_heft_makespan);
            assert!(report.heft_makespan < report.fp_makespan);
        }
    }
}
