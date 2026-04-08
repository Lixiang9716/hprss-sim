use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::{EdfScheduler, FixedPriorityScheduler};
use hprss_types::{
    CriticalityLevel, DeviceId, TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DeviceType, ExecutionTimeModel, Task},
};

/// Scope of Level 3 differential validation.
///
/// This is the in-repo CPU-only differential phase used as an internal baseline
/// before wiring external adapters (e.g., SimSo).
pub const LEVEL3_SCOPE: &str = "cpu-only differential baseline (internal phase)";
const MISS_RATIO_EPS: f64 = 1e-12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuOnlySchedulerConfig {
    FixedPriority,
    Edf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuOnlyTask {
    pub period: u64,
    pub deadline: u64,
    pub wcet: u64,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuOnlyWorkload {
    pub name: &'static str,
    pub horizon: u64,
    pub tasks: Vec<CpuOnlyTask>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CpuOnlyRunSummary {
    pub scheduler: CpuOnlySchedulerConfig,
    pub deadline_misses: u64,
    pub completion_count: u64,
    pub miss_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CpuOnlyDifferentialReport {
    pub scope: &'static str,
    pub workload: String,
    pub baseline: CpuOnlyRunSummary,
    pub candidate: CpuOnlyRunSummary,
    pub outputs_match: bool,
}

impl CpuOnlyDifferentialReport {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

pub fn selected_cpu_only_workloads() -> Vec<CpuOnlyWorkload> {
    vec![
        CpuOnlyWorkload {
            name: "single-task-control",
            horizon: 100,
            tasks: vec![CpuOnlyTask {
                period: 10,
                deadline: 10,
                wcet: 3,
                priority: 1,
            }],
        },
        CpuOnlyWorkload {
            name: "two-task-harmonic",
            horizon: 64,
            tasks: vec![
                CpuOnlyTask {
                    period: 4,
                    deadline: 4,
                    wcet: 1,
                    priority: 1,
                },
                CpuOnlyTask {
                    period: 8,
                    deadline: 8,
                    wcet: 2,
                    priority: 2,
                },
            ],
        },
        CpuOnlyWorkload {
            name: "three-task-harmonic",
            horizon: 100,
            tasks: vec![
                CpuOnlyTask {
                    period: 5,
                    deadline: 5,
                    wcet: 1,
                    priority: 1,
                },
                CpuOnlyTask {
                    period: 10,
                    deadline: 10,
                    wcet: 2,
                    priority: 2,
                },
                CpuOnlyTask {
                    period: 20,
                    deadline: 20,
                    wcet: 3,
                    priority: 3,
                },
            ],
        },
    ]
}

pub fn run_cpu_only_scheduler(
    workload: &CpuOnlyWorkload,
    scheduler: CpuOnlySchedulerConfig,
) -> CpuOnlyRunSummary {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: workload.horizon,
            seed: 7,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );
    engine.register_tasks(build_periodic_cpu_tasks(workload));
    engine.schedule_initial_arrivals();

    match scheduler {
        CpuOnlySchedulerConfig::FixedPriority => {
            let mut fp = FixedPriorityScheduler;
            engine.run(&mut fp);
        }
        CpuOnlySchedulerConfig::Edf => {
            let mut edf = EdfScheduler;
            engine.run(&mut edf);
        }
    }

    CpuOnlyRunSummary {
        scheduler,
        deadline_misses: engine.metrics().deadline_misses,
        completion_count: engine.metrics().completed_jobs,
        miss_ratio: engine.metrics().miss_ratio(),
    }
}

pub fn run_cpu_only_differential(
    workload: &CpuOnlyWorkload,
    baseline: CpuOnlySchedulerConfig,
    candidate: CpuOnlySchedulerConfig,
) -> CpuOnlyDifferentialReport {
    let baseline = run_cpu_only_scheduler(workload, baseline);
    let candidate = run_cpu_only_scheduler(workload, candidate);
    CpuOnlyDifferentialReport {
        scope: LEVEL3_SCOPE,
        workload: workload.name.to_string(),
        outputs_match: summaries_match(&baseline, &candidate),
        baseline,
        candidate,
    }
}

fn summaries_match(lhs: &CpuOnlyRunSummary, rhs: &CpuOnlyRunSummary) -> bool {
    lhs.deadline_misses == rhs.deadline_misses
        && lhs.completion_count == rhs.completion_count
        && (lhs.miss_ratio - rhs.miss_ratio).abs() <= MISS_RATIO_EPS
}

fn build_periodic_cpu_tasks(workload: &CpuOnlyWorkload) -> Vec<Task> {
    workload
        .tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| Task {
            id: TaskId(idx as u32),
            name: format!("{}-task-{idx}", workload.name),
            priority: task.priority,
            arrival: ArrivalModel::Periodic {
                period: task.period,
            },
            deadline: task.deadline,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: task.wcet },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp_edf_consistency_selected_workloads() {
        let workloads = selected_cpu_only_workloads();
        assert!(
            !workloads.is_empty(),
            "selected CPU-only workloads should be non-empty"
        );

        for workload in &workloads {
            let report = run_cpu_only_differential(
                workload,
                CpuOnlySchedulerConfig::FixedPriority,
                CpuOnlySchedulerConfig::Edf,
            );
            assert!(
                report.outputs_match,
                "expected FP/EDF outputs to match for workload {}",
                workload.name
            );
            assert_eq!(report.baseline.deadline_misses, 0);
            assert_eq!(report.candidate.deadline_misses, 0);
        }
    }

    #[test]
    fn output_schema_contains_required_fields() {
        let workload = selected_cpu_only_workloads()
            .into_iter()
            .next()
            .expect("expected at least one selected CPU workload");
        let report = run_cpu_only_differential(
            &workload,
            CpuOnlySchedulerConfig::FixedPriority,
            CpuOnlySchedulerConfig::Edf,
        );

        let value: serde_json::Value = serde_json::from_str(
            &report
                .to_json()
                .expect("report should serialize to JSON schema"),
        )
        .expect("serialized report should be valid JSON");

        assert!(value.get("scope").is_some());
        assert!(value.get("workload").is_some());
        assert!(value.get("baseline").is_some());
        assert!(value.get("candidate").is_some());
        assert!(value.get("outputs_match").is_some());

        for side in ["baseline", "candidate"] {
            let obj = value
                .get(side)
                .and_then(serde_json::Value::as_object)
                .expect("comparison side should be a JSON object");
            assert!(obj.contains_key("scheduler"));
            assert!(obj.contains_key("deadline_misses"));
            assert!(obj.contains_key("completion_count"));
            assert!(obj.contains_key("miss_ratio"));
        }
    }
}
