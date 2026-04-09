use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};

#[derive(Debug, Default)]
pub struct GlobalEdfScheduler;

impl GlobalEdfScheduler {
    fn compatible_devices(task: &Task, view: &SchedulerView<'_>) -> Vec<DeviceId> {
        task.affinity
            .iter()
            .flat_map(|device_type| {
                view.devices
                    .iter()
                    .filter(move |device| device.device_type == *device_type)
                    .map(|device| device.id)
            })
            .collect()
    }

    fn pick_idle_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates
            .iter()
            .copied()
            .filter(|device_id| {
                view.running_jobs
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .and_then(|(_, info)| info.as_ref())
                    .is_none()
            })
            .min_by_key(|device_id| {
                let queue_len = view
                    .ready_queues
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .map_or(0, |(_, queue)| queue.len());
                (queue_len, device_id.0)
            })
    }

    fn pick_enqueue_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates.iter().copied().min_by_key(|device_id| {
            let queue_len = view
                .ready_queues
                .iter()
                .find(|(did, _)| *did == *device_id)
                .map_or(0, |(_, queue)| queue.len());
            (queue_len, device_id.0)
        })
    }

    fn pick_preemption_target<'a>(
        candidates: &[DeviceId],
        view: &'a SchedulerView<'_>,
    ) -> Option<(DeviceId, &'a hprss_types::RunningJobInfo)> {
        candidates
            .iter()
            .filter_map(|device_id| {
                view.running_jobs
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .and_then(|(_, running)| running.as_ref())
                    .map(|running| (*device_id, running))
            })
            .max_by_key(|(_, running)| {
                (
                    running.absolute_deadline,
                    running.release_time,
                    running.job_id.0,
                )
            })
    }

    fn next_local_edf_job(
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Option<hprss_types::JobId> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| {
                queue
                    .iter()
                    .min_by_key(|job| (job.absolute_deadline, job.release_time, job.job_id.0))
            })
            .map(|job| job.job_id)
    }
}

impl Scheduler for GlobalEdfScheduler {
    fn name(&self) -> &str {
        "Global-EDF"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let candidates = Self::compatible_devices(task, view);
        if candidates.is_empty() {
            return vec![Action::NoOp];
        }

        if let Some(device_id) = Self::pick_idle_device(&candidates, view) {
            return vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }];
        }

        if let Some((device_id, running)) = Self::pick_preemption_target(&candidates, view)
            && job.absolute_deadline < running.absolute_deadline
        {
            return vec![Action::Preempt {
                victim: running.job_id,
                by: job.id,
                device_id,
            }];
        }

        let Some(device_id) = Self::pick_enqueue_device(&candidates, view) else {
            return vec![Action::NoOp];
        };
        vec![Action::Enqueue {
            job_id: job.id,
            device_id,
        }]
    }

    fn on_job_complete(
        &mut self,
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        match Self::next_local_edf_job(device_id, view) {
            Some(job_id) => vec![Action::Dispatch { job_id, device_id }],
            None => vec![Action::NoOp],
        }
    }

    fn on_preemption_point(
        &mut self,
        device_id: DeviceId,
        running_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| {
                queue
                    .iter()
                    .min_by_key(|job| (job.absolute_deadline, job.release_time, job.job_id.0))
            });

        match next {
            Some(waiting) if waiting.absolute_deadline < running_job.absolute_deadline => {
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
        let mut actions = Vec::new();
        for (_, queue) in view.ready_queues {
            for job in queue {
                if job.criticality == CriticalityLevel::Lo {
                    actions.push(Action::DropJob { job_id: job.job_id });
                }
            }
        }
        actions
    }

    fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
        match Self::next_local_edf_job(device_id, view) {
            Some(job_id) => vec![Action::Dispatch { job_id, device_id }],
            None => vec![Action::NoOp],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        DeviceId, JobId, JobState, TaskId,
        device::{DeviceConfig, PreemptionModel},
        scheduler::RunningJobInfo,
        task::{ArrivalModel, CriticalityLevel, DeviceType, ExecutionTimeModel},
    };

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

    fn cpu_task(id: u32) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 1,
            arrival: ArrivalModel::Aperiodic,
            deadline: 50_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 10_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        }
    }

    fn job(id: u64, deadline: u64) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(id as u32),
            state: JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: deadline,
            actual_exec_ns: Some(10_000),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: 1,
        }
    }

    #[test]
    fn arrival_preempts_worst_running_compatible_job_globally() {
        let mut sched = GlobalEdfScheduler;
        let incoming = job(10, 30_000);
        let task = cpu_task(1);
        let devices = [cpu_device(0), cpu_device(1)];
        let view = SchedulerView {
            now: 1_000,
            devices: &devices,
            running_jobs: &[
                (
                    DeviceId(0),
                    Some(RunningJobInfo {
                        job_id: JobId(1),
                        task_id: TaskId(1),
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: 80_000,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 100,
                    }),
                ),
                (
                    DeviceId(1),
                    Some(RunningJobInfo {
                        job_id: JobId(2),
                        task_id: TaskId(2),
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: 40_000,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 100,
                    }),
                ),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(1),
                by: JobId(10),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn arrival_dispatches_to_idle_compatible_device() {
        let mut sched = GlobalEdfScheduler;
        let incoming = job(3, 20_000);
        let task = cpu_task(1);
        let devices = [cpu_device(0), cpu_device(1)];
        let view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[
                (
                    DeviceId(0),
                    Some(RunningJobInfo {
                        job_id: JobId(1),
                        task_id: TaskId(1),
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: 40_000,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 100,
                    }),
                ),
                (DeviceId(1), None),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(3),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn idle_dispatches_earliest_deadline_from_local_queue() {
        let mut sched = GlobalEdfScheduler;
        let devices = [cpu_device(0)];
        let view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    hprss_types::QueuedJobInfo {
                        job_id: JobId(4),
                        task_id: TaskId(1),
                        priority: 2,
                        release_time: 0,
                        absolute_deadline: 80_000,
                        criticality: CriticalityLevel::Lo,
                    },
                    hprss_types::QueuedJobInfo {
                        job_id: JobId(5),
                        task_id: TaskId(1),
                        priority: 2,
                        release_time: 0,
                        absolute_deadline: 20_000,
                        criticality: CriticalityLevel::Lo,
                    },
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_device_idle(DeviceId(0), &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(5),
                device_id: DeviceId(0)
            }]
        ));
    }
}
