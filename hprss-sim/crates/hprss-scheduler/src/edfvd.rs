use std::collections::BTreeSet;

use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, Nanos, Scheduler, SchedulerView, task::Task,
};

const DEFAULT_VIRTUAL_DEADLINE_NUM: u64 = 1;
const DEFAULT_VIRTUAL_DEADLINE_DEN: u64 = 2;

/// EDF-VD scheduler for mixed-criticality systems.
#[derive(Debug, Clone)]
pub struct EdfVdScheduler {
    virtual_deadline_num: u64,
    virtual_deadline_den: u64,
}

impl Default for EdfVdScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_VIRTUAL_DEADLINE_NUM, DEFAULT_VIRTUAL_DEADLINE_DEN)
    }
}

impl EdfVdScheduler {
    pub fn new(virtual_deadline_num: u64, virtual_deadline_den: u64) -> Self {
        assert!(
            virtual_deadline_den > 0,
            "virtual deadline denominator must be > 0"
        );
        assert!(
            virtual_deadline_num <= virtual_deadline_den,
            "virtual deadline ratio must be in [0, 1]"
        );
        Self {
            virtual_deadline_num,
            virtual_deadline_den,
        }
    }

    fn is_schedulable_in_mode(task_criticality: CriticalityLevel, mode: CriticalityLevel) -> bool {
        !(mode == CriticalityLevel::Hi && task_criticality == CriticalityLevel::Lo)
    }

    fn effective_deadline(
        &self,
        task_criticality: CriticalityLevel,
        release_time: Nanos,
        absolute_deadline: Nanos,
        mode: CriticalityLevel,
    ) -> Nanos {
        if mode == CriticalityLevel::Lo && task_criticality == CriticalityLevel::Hi {
            let relative = absolute_deadline.saturating_sub(release_time);
            let vd_relative =
                (relative.saturating_mul(self.virtual_deadline_num)) / self.virtual_deadline_den;
            release_time.saturating_add(vd_relative)
        } else {
            absolute_deadline
        }
    }

    fn select_target_device(task: &Task, view: &SchedulerView<'_>) -> Option<DeviceId> {
        task.affinity
            .iter()
            .flat_map(|dt| {
                view.devices
                    .iter()
                    .filter(move |d| d.device_type == *dt)
                    .map(|d| d.id)
            })
            .min_by_key(|device_id| {
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
                (is_running as u8, queue_len, device_id.0)
            })
    }

    fn best_waiting_job<'a>(
        &self,
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::QueuedJobInfo> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| {
                q.iter()
                    .filter(|job| {
                        Self::is_schedulable_in_mode(job.criticality, view.criticality_level)
                    })
                    .min_by_key(|job| {
                        (
                            self.effective_deadline(
                                job.criticality,
                                job.release_time,
                                job.absolute_deadline,
                                view.criticality_level,
                            ),
                            job.absolute_deadline,
                            job.release_time,
                            job.job_id.0,
                        )
                    })
            })
    }
}

impl Scheduler for EdfVdScheduler {
    fn name(&self) -> &str {
        "EDF-VD-Het"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        if !Self::is_schedulable_in_mode(task.criticality, view.criticality_level) {
            return vec![Action::DropJob { job_id: job.id }];
        }

        let Some(device_id) = Self::select_target_device(task, view) else {
            return vec![Action::NoOp];
        };

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
                if !Self::is_schedulable_in_mode(running_info.criticality, view.criticality_level) {
                    return vec![
                        Action::DropJob {
                            job_id: running_info.job_id,
                        },
                        Action::Dispatch {
                            job_id: job.id,
                            device_id,
                        },
                    ];
                }

                let incoming_deadline = self.effective_deadline(
                    task.criticality,
                    job.release_time,
                    job.absolute_deadline,
                    view.criticality_level,
                );
                let running_deadline = self.effective_deadline(
                    running_info.criticality,
                    running_info.release_time,
                    running_info.absolute_deadline,
                    view.criticality_level,
                );
                if incoming_deadline < running_deadline
                    || (incoming_deadline == running_deadline
                        && (job.absolute_deadline, job.release_time, job.id.0)
                            < (
                                running_info.absolute_deadline,
                                running_info.release_time,
                                running_info.job_id.0,
                            ))
                {
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

    fn on_job_complete(
        &mut self,
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        match self.best_waiting_job(device_id, view) {
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
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, info)| info.as_ref());
        let waiting = self.best_waiting_job(device_id, view);

        match (running, waiting) {
            (Some(running_info), Some(waiting))
                if !Self::is_schedulable_in_mode(
                    running_info.criticality,
                    view.criticality_level,
                ) =>
            {
                vec![
                    Action::DropJob {
                        job_id: running_info.job_id,
                    },
                    Action::Dispatch {
                        job_id: waiting.job_id,
                        device_id,
                    },
                ]
            }
            (Some(running_info), Some(waiting))
                if self.effective_deadline(
                    waiting.criticality,
                    waiting.release_time,
                    waiting.absolute_deadline,
                    view.criticality_level,
                ) < self.effective_deadline(
                    running_info.criticality,
                    running_info.release_time,
                    running_info.absolute_deadline,
                    view.criticality_level,
                ) =>
            {
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
        if new_level != CriticalityLevel::Hi {
            return vec![Action::NoOp];
        }

        let mut to_drop = BTreeSet::new();
        for (_, running) in view.running_jobs {
            if let Some(running) = running
                && running.criticality == CriticalityLevel::Lo
            {
                to_drop.insert(running.job_id);
            }
        }
        for (_, queue) in view.ready_queues {
            for job in queue {
                if job.criticality == CriticalityLevel::Lo {
                    to_drop.insert(job.job_id);
                }
            }
        }

        if to_drop.is_empty() {
            vec![Action::NoOp]
        } else {
            to_drop
                .into_iter()
                .map(|job_id| Action::DropJob { job_id })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        DeviceId, JobId, JobState, SchedulerView, TaskId,
        device::{DeviceConfig, PreemptionModel},
        scheduler::{QueuedJobInfo, RunningJobInfo},
        task::{ArrivalModel, DeviceType, ExecutionTimeModel},
    };

    fn cpu_device() -> DeviceConfig {
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
        }
    }

    fn task(id: u32, criticality: CriticalityLevel, deadline: Nanos) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: id,
            arrival: ArrivalModel::Aperiodic,
            deadline,
            criticality,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 10_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        }
    }

    fn job(id: u64, task_id: u32, release_time: Nanos, deadline: Nanos) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(task_id),
            state: JobState::Ready,
            version: 0,
            release_time,
            absolute_deadline: deadline,
            actual_exec_ns: Some(10_000),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: task_id,
        }
    }

    #[test]
    fn lo_mode_uses_virtual_deadline_for_hi_tasks() {
        let mut scheduler = EdfVdScheduler::default();
        let running = RunningJobInfo {
            job_id: JobId(1),
            task_id: TaskId(1),
            priority: 1,
            release_time: 0,
            absolute_deadline: 40_000,
            criticality: CriticalityLevel::Lo,
            elapsed_ns: 5_000,
        };
        let view = SchedulerView {
            now: 1_000,
            devices: &[cpu_device()],
            running_jobs: &[(DeviceId(0), Some(running))],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let incoming_task = task(2, CriticalityLevel::Hi, 50_000);
        let incoming_job = job(2, 2, 0, 50_000);

        let actions = scheduler.on_job_arrival(&incoming_job, &incoming_task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(1),
                by: JobId(2),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn mode_switch_drops_lo_jobs() {
        let mut scheduler = EdfVdScheduler::default();
        let trigger = job(10, 10, 0, 100_000);
        let running = [
            (
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(1),
                    task_id: TaskId(1),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 50_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 2_000,
                }),
            ),
            (
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: JobId(2),
                    task_id: TaskId(2),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 50_000,
                    criticality: CriticalityLevel::Hi,
                    elapsed_ns: 2_000,
                }),
            ),
        ];
        let ready = [(
            DeviceId(0),
            vec![
                QueuedJobInfo {
                    job_id: JobId(3),
                    task_id: TaskId(3),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 60_000,
                    criticality: CriticalityLevel::Lo,
                },
                QueuedJobInfo {
                    job_id: JobId(4),
                    task_id: TaskId(4),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 60_000,
                    criticality: CriticalityLevel::Hi,
                },
            ],
        )];
        let devices = [cpu_device()];
        let view = SchedulerView {
            now: 10_000,
            devices: &devices,
            running_jobs: &running,
            ready_queues: &ready,
            criticality_level: CriticalityLevel::Hi,
        };

        let actions = scheduler.on_criticality_change(CriticalityLevel::Hi, &trigger, &view);
        assert_eq!(
            actions
                .into_iter()
                .map(|a| match a {
                    Action::DropJob { job_id } => job_id,
                    other => panic!("unexpected action: {other:?}"),
                })
                .collect::<Vec<_>>(),
            vec![JobId(1), JobId(3)]
        );
    }

    #[test]
    fn hi_mode_drops_lo_arrivals() {
        let mut scheduler = EdfVdScheduler::default();
        let view = SchedulerView {
            now: 0,
            devices: &[cpu_device()],
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Hi,
        };
        let lo_task = task(3, CriticalityLevel::Lo, 20_000);
        let lo_job = job(7, 3, 0, 20_000);

        let actions = scheduler.on_job_arrival(&lo_job, &lo_task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::DropJob { job_id: JobId(7) }]
        ));
    }

    #[test]
    fn hi_mode_ignores_lo_jobs_when_selecting_next() {
        let mut scheduler = EdfVdScheduler::default();
        let ready = [(
            DeviceId(0),
            vec![
                QueuedJobInfo {
                    job_id: JobId(11),
                    task_id: TaskId(11),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 10_000,
                    criticality: CriticalityLevel::Lo,
                },
                QueuedJobInfo {
                    job_id: JobId(12),
                    task_id: TaskId(12),
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: 20_000,
                    criticality: CriticalityLevel::Hi,
                },
            ],
        )];
        let devices = [cpu_device()];
        let view = SchedulerView {
            now: 10_000,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &ready,
            criticality_level: CriticalityLevel::Hi,
        };
        let completed = job(1, 1, 0, 5_000);

        let actions = scheduler.on_job_complete(&completed, DeviceId(0), &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(12),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn hi_mode_preemption_point_drops_running_lo_job() {
        let mut scheduler = EdfVdScheduler::default();
        let running = [(
            DeviceId(0),
            Some(RunningJobInfo {
                job_id: JobId(20),
                task_id: TaskId(20),
                priority: 1,
                release_time: 0,
                absolute_deadline: 15_000,
                criticality: CriticalityLevel::Lo,
                elapsed_ns: 1_000,
            }),
        )];
        let ready = [(
            DeviceId(0),
            vec![QueuedJobInfo {
                job_id: JobId(21),
                task_id: TaskId(21),
                priority: 1,
                release_time: 0,
                absolute_deadline: 20_000,
                criticality: CriticalityLevel::Hi,
            }],
        )];
        let devices = [cpu_device()];
        let view = SchedulerView {
            now: 5_000,
            devices: &devices,
            running_jobs: &running,
            ready_queues: &ready,
            criticality_level: CriticalityLevel::Hi,
        };
        let running_job = job(20, 20, 0, 15_000);

        let actions = scheduler.on_preemption_point(DeviceId(0), &running_job, &view);
        assert!(matches!(
            actions.as_slice(),
            [
                Action::DropJob { job_id: JobId(20) },
                Action::Dispatch {
                    job_id: JobId(21),
                    device_id: DeviceId(0)
                }
            ]
        ));
    }
}
