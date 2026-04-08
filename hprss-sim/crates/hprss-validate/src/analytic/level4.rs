use std::collections::HashMap;

use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::FixedPriorityScheduler;
use hprss_types::{
    BusArbitration, CriticalityLevel, DeviceId, EventKind, JobId, TaskId,
    device::{DeviceConfig, InterconnectConfig, PreemptionModel, SharedBusConfig},
    task::{ArrivalModel, DagTask, DeviceType, ExecutionTimeModel, SubTask, Task},
};

/// Scope of Level 4 validation.
pub const LEVEL4_SCOPE: &str =
    "heterogeneous core checks: gpu boundary, dsp dma block, fpga switch, transfer gating";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeteroPreemptionObservation {
    pub low_job_completion: u64,
    pub high_job_completion: u64,
    pub high_job_release: u64,
    pub blocking_window_ns: u64,
    pub deadline_misses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FpgaSwitchObservation {
    pub context_switch_ns: u64,
    pub reconfig_time_ns: u64,
    pub low_job_completion: u64,
    pub high_job_completion: Option<u64>,
    pub completed_jobs: u64,
    pub deadline_misses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferGatingObservation {
    pub predecessor_completion: u64,
    pub successor_release: u64,
    pub successor_completion: u64,
    pub transfer_time: u64,
    pub deadline_misses: u64,
}

pub fn observe_gpu_limited_preemption_boundary() -> HeteroPreemptionObservation {
    run_non_fully_preemptive_case(gpu_device(0, 10), DeviceType::Gpu, 20, 3, 0, 5, 10)
}

pub fn observe_dsp_dma_blocking() -> HeteroPreemptionObservation {
    run_non_fully_preemptive_case(dsp_device(0, 7), DeviceType::Dsp, 16, 2, 0, 3, 7)
}

pub fn observe_fpga_non_preemptive_switch() -> FpgaSwitchObservation {
    run_fpga_switch_case(5)
}

pub fn observe_dag_transfer_gating() -> TransferGatingObservation {
    let transfer_time = 2_000 + 250; // latency + ceil(500 / 2.0)
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 50_000,
            seed: 53,
        },
        vec![cpu_device(0), gpu_device(1, 10_000)],
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
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        }],
    );

    let dag = DagTask {
        id: TaskId(90),
        name: "transfer-gated".to_string(),
        arrival: ArrivalModel::Aperiodic,
        deadline: 50_000,
        criticality: CriticalityLevel::Lo,
        nodes: vec![
            SubTask {
                index: 0,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 5_000 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_deps: vec![],
            },
            SubTask {
                index: 1,
                exec_times: vec![(
                    DeviceType::Gpu,
                    ExecutionTimeModel::Deterministic { wcet: 4_000 },
                )],
                affinity: vec![DeviceType::Gpu],
                data_deps: vec![(0, 500)],
            },
        ],
        edges: vec![(0, 1)],
    };
    engine.register_dags(vec![dag]);

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    let mut release_by_node = [0_u64; 2];
    for job_id in [JobId(0), JobId(1)] {
        let job = engine
            .get_job(job_id)
            .expect("both DAG node jobs should exist in tiny case");
        let node = job
            .dag_provenance
            .expect("DAG jobs must preserve provenance")
            .node
            .0 as usize;
        release_by_node[node] = job.release_time;
    }

    let mut completion_by_node = [0_u64; 2];
    for completion in &engine.metrics().completions {
        let job = engine
            .get_job(completion.job_id)
            .expect("completion must reference existing DAG jobs");
        let node = job
            .dag_provenance
            .expect("DAG jobs must preserve provenance")
            .node
            .0 as usize;
        completion_by_node[node] = completion.completion_time;
    }

    TransferGatingObservation {
        predecessor_completion: completion_by_node[0],
        successor_release: release_by_node[1],
        successor_completion: completion_by_node[1],
        transfer_time,
        deadline_misses: engine.metrics().deadline_misses,
    }
}

fn run_non_fully_preemptive_case(
    device: DeviceConfig,
    device_type: DeviceType,
    low_wcet: u64,
    high_wcet: u64,
    low_release: u64,
    high_release: u64,
    blocking_window_ns: u64,
) -> HeteroPreemptionObservation {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 100,
            seed: 41,
        },
        vec![device],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![
        aperiodic_task(0, 2, 100, low_wcet, device_type, "low"),
        aperiodic_task(1, 1, 100, high_wcet, device_type, "high"),
    ]);

    let low = schedule_aperiodic_job(&mut engine, TaskId(0), low_release, 100, 2);
    let high = schedule_aperiodic_job(&mut engine, TaskId(1), high_release, 100, 1);

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    let completion_by_job: HashMap<JobId, u64> = engine
        .metrics()
        .completions
        .iter()
        .map(|record| (record.job_id, record.completion_time))
        .collect();

    HeteroPreemptionObservation {
        low_job_completion: completion_by_job
            .get(&low)
            .copied()
            .expect("low-priority job must complete"),
        high_job_completion: completion_by_job
            .get(&high)
            .copied()
            .expect("high-priority job must complete"),
        high_job_release: high_release,
        blocking_window_ns,
        deadline_misses: engine.metrics().deadline_misses,
    }
}

fn run_fpga_switch_case(context_switch_ns: u64) -> FpgaSwitchObservation {
    let reconfig_time_ns = 5;
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 100,
            seed: 47,
        },
        vec![fpga_device(0, context_switch_ns, reconfig_time_ns)],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![
        aperiodic_task(0, 2, 100, 10, DeviceType::Fpga, "fpga-low"),
        aperiodic_task(1, 1, 100, 10, DeviceType::Fpga, "fpga-high"),
    ]);

    let low = schedule_aperiodic_job(&mut engine, TaskId(0), 0, 100, 2);
    let high = schedule_aperiodic_job(&mut engine, TaskId(1), 1, 29, 1);

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    let completion_by_job: HashMap<JobId, u64> = engine
        .metrics()
        .completions
        .iter()
        .map(|record| (record.job_id, record.completion_time))
        .collect();

    FpgaSwitchObservation {
        context_switch_ns,
        reconfig_time_ns,
        low_job_completion: completion_by_job
            .get(&low)
            .copied()
            .expect("low-priority FPGA job should complete"),
        high_job_completion: completion_by_job.get(&high).copied(),
        completed_jobs: engine.metrics().completed_jobs,
        deadline_misses: engine.metrics().deadline_misses,
    }
}

fn gpu_device(id: u32, granularity_ns: u64) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("gpu-{id}"),
        device_group: None,
        device_type: DeviceType::Gpu,
        cores: 1,
        preemption: PreemptionModel::LimitedPreemptive { granularity_ns },
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn dsp_device(id: u32, dma_non_preemptive_ns: u64) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("dsp-{id}"),
        device_group: None,
        device_type: DeviceType::Dsp,
        cores: 1,
        preemption: PreemptionModel::InterruptLevel {
            isr_overhead_ns: 500,
            dma_non_preemptive_ns,
        },
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn fpga_device(id: u32, context_switch_ns: u64, reconfig_time_ns: u64) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("fpga-{id}"),
        device_group: None,
        device_type: DeviceType::Fpga,
        cores: 1,
        preemption: PreemptionModel::NonPreemptive { reconfig_time_ns },
        context_switch_ns,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn cpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("cpu-{id}"),
        device_group: None,
        device_type: DeviceType::Cpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn aperiodic_task(
    id: u32,
    priority: u32,
    deadline: u64,
    wcet: u64,
    device: DeviceType,
    name: &str,
) -> Task {
    Task {
        id: TaskId(id),
        name: name.to_string(),
        priority,
        arrival: ArrivalModel::Aperiodic,
        deadline,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![(device, ExecutionTimeModel::Deterministic { wcet })],
        affinity: vec![device],
        data_size: 0,
    }
}

fn schedule_aperiodic_job(
    engine: &mut SimEngine,
    task_id: TaskId,
    release_time: u64,
    absolute_deadline: u64,
    priority: u32,
) -> JobId {
    let job_id = engine.create_job(task_id, release_time, absolute_deadline, None, priority);
    engine.schedule_event(release_time, EventKind::TaskArrival { task_id, job_id });
    engine.schedule_event(
        absolute_deadline.saturating_add(1),
        EventKind::DeadlineCheck {
            job_id,
            expected_version: 0,
        },
    );
    job_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_marker_is_set() {
        assert_eq!(
            LEVEL4_SCOPE,
            "heterogeneous core checks: gpu boundary, dsp dma block, fpga switch, transfer gating"
        );
    }

    #[test]
    fn gpu_limited_preemption_happens_only_at_boundary() {
        let obs = observe_gpu_limited_preemption_boundary();
        assert_eq!(obs.high_job_release, 5);
        assert_eq!(obs.blocking_window_ns, 10);
        assert_eq!(obs.high_job_completion, 13);
        assert_eq!(obs.low_job_completion, 23);
        assert!(
            obs.high_job_completion > obs.high_job_release + 3,
            "high-priority job should wait for the next GPU boundary instead of preempting immediately"
        );
        assert_eq!(obs.deadline_misses, 0);
    }

    #[test]
    fn dsp_dma_non_preemptive_region_introduces_blocking() {
        let obs = observe_dsp_dma_blocking();
        assert_eq!(obs.high_job_release, 3);
        assert_eq!(obs.blocking_window_ns, 7);
        assert_eq!(obs.high_job_completion, 9);
        assert_eq!(obs.low_job_completion, 18);
        assert!(
            obs.high_job_completion > obs.high_job_release + 2,
            "high-priority DSP job should be blocked until DMA non-preemptive region ends"
        );
        assert_eq!(obs.deadline_misses, 0);
    }

    #[test]
    fn fpga_non_preemptive_reconfiguration_overhead_impacts_deadline_behavior() {
        let without_overhead = run_fpga_switch_case(0);
        let with_overhead = observe_fpga_non_preemptive_switch();

        assert_eq!(without_overhead.context_switch_ns, 0);
        assert_eq!(without_overhead.deadline_misses, 0);
        assert_eq!(without_overhead.completed_jobs, 2);
        assert_eq!(without_overhead.low_job_completion, 10);
        assert_eq!(without_overhead.high_job_completion, Some(20));

        assert_eq!(with_overhead.context_switch_ns, 5);
        assert_eq!(with_overhead.deadline_misses, 1);
        assert_eq!(with_overhead.completed_jobs, 1);
        assert_eq!(with_overhead.low_job_completion, 15);
        assert_eq!(with_overhead.high_job_completion, None);
    }

    #[test]
    fn dag_successor_is_gated_until_transfer_completes() {
        let obs = observe_dag_transfer_gating();
        assert_eq!(obs.predecessor_completion, 5_000);
        assert_eq!(
            obs.successor_release,
            obs.predecessor_completion + obs.transfer_time
        );
        assert_eq!(obs.successor_completion, 11_250);
        assert!(
            obs.successor_release > obs.predecessor_completion,
            "successor node should not release before edge transfer completion"
        );
        assert_eq!(obs.deadline_misses, 0);
    }
}
