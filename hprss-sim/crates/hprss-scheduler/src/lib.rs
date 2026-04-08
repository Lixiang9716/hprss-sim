//! Scheduler trait implementations.
//!
//! Built-in algorithms: Fixed Priority, EDF, LLF (heterogeneous variants).

pub mod edf;
pub mod heft;

use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};

pub use edf::EdfScheduler;
pub use heft::{HeftPlan, HeftPlanner, HeftScheduler};

/// Fixed-Priority scheduler (Rate Monotonic as default priority assignment)
pub struct FixedPriorityScheduler;

impl Scheduler for FixedPriorityScheduler {
    fn name(&self) -> &str {
        "FP-Het"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        // Choose a compatible device, preferring idle then shorter ready queue.
        let candidates: Vec<DeviceId> = task
            .affinity
            .iter()
            .flat_map(|dt| {
                view.devices
                    .iter()
                    .filter(move |d| d.device_type == *dt)
                    .map(|d| d.id)
            })
            .collect();

        let target_device = candidates.into_iter().min_by_key(|device_id| {
            let is_running = view
                .running_jobs
                .iter()
                .find(|(did, _)| did == device_id)
                .and_then(|(_, info)| info.as_ref())
                .is_some();
            let queue_len = view
                .ready_queues
                .iter()
                .find(|(did, _)| did == device_id)
                .map_or(0, |(_, q)| q.len());
            (is_running as u8, queue_len)
        });

        match target_device {
            Some(device_id) => {
                // Check if device is idle or if we should preempt
                let running = view
                    .running_jobs
                    .iter()
                    .find(|(did, _)| *did == device_id)
                    .and_then(|(_, info)| info.as_ref());

                match running {
                    None => vec![Action::Dispatch {
                        job_id: job.id,
                        device_id,
                    }],
                    Some(running_info) => {
                        if job.effective_priority < running_info.priority {
                            // Higher priority (lower number) → preempt
                            vec![Action::Preempt {
                                victim: running_info.job_id,
                                by: job.id,
                                device_id,
                            }]
                        } else {
                            vec![Action::Enqueue {
                                job_id: job.id,
                                device_id,
                            }]
                        }
                    }
                }
            }
            None => {
                tracing::warn!("No compatible device for task {:?}", task.id);
                vec![Action::NoOp]
            }
        }
    }

    fn on_job_complete(
        &mut self,
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        // Dispatch highest priority waiting job on this device
        let queue = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .map(|(_, q)| q);

        match queue.and_then(|q| q.first()) {
            Some(next) => vec![Action::Dispatch {
                job_id: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }

    fn on_preemption_point(
        &mut self,
        device_id: DeviceId,
        running_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        // Check if a higher priority job is waiting
        let queue = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .map(|(_, q)| q);

        match queue.and_then(|q| q.first()) {
            Some(waiting) if waiting.priority < running_job.effective_priority => {
                vec![Action::Preempt {
                    victim: running_job.id,
                    by: waiting.job_id,
                    device_id,
                }]
            }
            _ => vec![Action::NoOp],
        }
    }

    fn on_criticality_change(
        &mut self,
        new_level: CriticalityLevel,
        _trigger_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        if new_level == CriticalityLevel::Hi {
            // Drop all Lo-criticality jobs
            let mut actions = Vec::new();
            for (_, queue) in view.ready_queues.iter() {
                for job_info in queue.iter() {
                    if job_info.criticality == CriticalityLevel::Lo {
                        actions.push(Action::DropJob {
                            job_id: job_info.job_id,
                        });
                    }
                }
            }
            actions
        } else {
            vec![Action::NoOp]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        DeviceId, JobId, ReevaluationPolicy, TaskId,
        device::{DeviceConfig, PreemptionModel},
        scheduler::RunningJobInfo,
        task::{ArrivalModel, CriticalityLevel, DeviceType, ExecutionTimeModel, Task},
    };

    #[test]
    fn fp_scheduler_name() {
        let sched = FixedPriorityScheduler;
        assert_eq!(sched.name(), "FP-Het");
    }

    #[test]
    fn edf_scheduler_name() {
        let sched = crate::edf::EdfScheduler;
        assert_eq!(sched.name(), "EDF-Het");
    }

    #[test]
    fn built_in_schedulers_keep_reevaluation_disabled_by_default() {
        assert_eq!(
            FixedPriorityScheduler.reevaluation_policy(),
            ReevaluationPolicy::Disabled
        );
        assert_eq!(
            crate::edf::EdfScheduler.reevaluation_policy(),
            ReevaluationPolicy::Disabled
        );
        assert_eq!(
            crate::heft::HeftScheduler::default().reevaluation_policy(),
            ReevaluationPolicy::Disabled
        );
    }

    #[test]
    fn edf_preempts_running_job_with_later_deadline() {
        let mut sched = crate::edf::EdfScheduler;
        let device = DeviceConfig {
            id: DeviceId(0),
            name: "cpu0".to_string(),
            device_group: None,
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption: PreemptionModel::FullyPreemptive,
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        };
        let view = SchedulerView {
            now: 1_000,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(7),
                    task_id: TaskId(7),
                    priority: 10,
                    release_time: 0,
                    absolute_deadline: 50_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let task = Task {
            id: TaskId(1),
            name: "edf-task".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 10_000 },
            deadline: 10_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 1_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        };
        let incoming = Job {
            id: JobId(8),
            task_id: TaskId(1),
            state: hprss_types::JobState::Ready,
            version: 0,
            release_time: 1_000,
            absolute_deadline: 20_000,
            actual_exec_ns: Some(1_000),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: Some(DeviceId(0)),
            exec_start_time: None,
            effective_priority: 1,
        };

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(7),
                by: JobId(8),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn heft_planner_maps_node_to_fastest_device() {
        let devices = vec![
            DeviceConfig {
                id: DeviceId(0),
                name: "cpu0".to_string(),
                device_group: None,
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
                name: "gpu0".to_string(),
                device_group: None,
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
        ];

        let dag = hprss_types::task::DagTask {
            id: TaskId(42),
            name: "heft".to_string(),
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            nodes: vec![
                hprss_types::task::SubTask {
                    index: 0,
                    exec_times: vec![
                        (
                            DeviceType::Cpu,
                            ExecutionTimeModel::Deterministic { wcet: 100_000 },
                        ),
                        (
                            DeviceType::Gpu,
                            ExecutionTimeModel::Deterministic { wcet: 40_000 },
                        ),
                    ],
                    affinity: vec![DeviceType::Cpu, DeviceType::Gpu],
                    data_deps: vec![],
                },
                hprss_types::task::SubTask {
                    index: 1,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 50_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![(0, 128)],
                },
            ],
            edges: vec![(0, 1)],
        };

        let mut planner = crate::heft::HeftPlanner::default();
        let plan = planner.build_plan(&dag, &devices);
        assert_eq!(plan.device_mapping.len(), 2);
        assert_eq!(plan.rank_u.len(), 2);
        assert_eq!(plan.device_mapping[0], DeviceId(1));
        assert!(plan.rank_u[0] > plan.rank_u[1]);
    }

    #[test]
    fn heft_scheduler_dispatches_dag_node_to_planned_device() {
        let mut sched = crate::heft::HeftScheduler::default();
        sched.set_instance_plan(
            hprss_types::DagInstanceId(9),
            crate::heft::HeftPlan {
                rank_u: vec![10.0],
                device_mapping: vec![DeviceId(1)],
            },
        );

        let task = Task {
            id: TaskId(3),
            name: "node0".to_string(),
            priority: 1,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 5_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        };
        let job = Job {
            id: JobId(11),
            task_id: TaskId(3),
            state: hprss_types::JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: 100_000,
            actual_exec_ns: Some(5_000),
            dag_provenance: Some(hprss_types::DagProvenance {
                dag_instance_id: hprss_types::DagInstanceId(9),
                node: hprss_types::SubTaskIdx(0),
            }),
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: 1,
        };

        let cpu = DeviceConfig {
            id: DeviceId(0),
            name: "cpu0".to_string(),
            device_group: None,
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption: PreemptionModel::FullyPreemptive,
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        };
        let gpu = DeviceConfig {
            id: DeviceId(1),
            name: "gpu0".to_string(),
            device_group: None,
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive {
                granularity_ns: 10_000,
            },
            context_switch_ns: 0,
            speed_factor: 4.0,
            multicore_policy: None,
            power_watts: None,
        };
        let view = SchedulerView {
            now: 0,
            devices: &[cpu, gpu],
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&job, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(11),
                device_id: DeviceId(1)
            }]
        ));
    }
}
