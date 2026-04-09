use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, TaskId, task::Task,
};

/// Gang scheduler baseline:
/// choose one active task as the current gang and avoid mixing other tasks
/// on devices while the active gang still has runnable work.
#[derive(Debug, Default)]
pub struct GangScheduler {
    active_task: Option<TaskId>,
}

impl GangScheduler {
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

    fn refresh_active_task(&mut self, view: &SchedulerView<'_>) {
        if let Some(active_task) = self.active_task
            && (Self::has_running_task(view, active_task)
                || Self::has_queued_task(view, active_task))
        {
            return;
        }

        self.active_task = Self::select_next_active_task(view);
    }

    fn has_running_task(view: &SchedulerView<'_>, task_id: TaskId) -> bool {
        view.running_jobs.iter().any(|(_, running)| {
            running
                .as_ref()
                .is_some_and(|running_job| running_job.task_id == task_id)
        })
    }

    fn has_queued_task(view: &SchedulerView<'_>, task_id: TaskId) -> bool {
        view.ready_queues
            .iter()
            .any(|(_, queue)| queue.iter().any(|job| job.task_id == task_id))
    }

    fn select_next_active_task(view: &SchedulerView<'_>) -> Option<TaskId> {
        let best_running = view
            .running_jobs
            .iter()
            .filter_map(|(_, running)| running.as_ref())
            .min_by_key(|job| (job.priority, job.absolute_deadline, job.task_id.0))
            .map(|job| job.task_id);

        if best_running.is_some() {
            return best_running;
        }

        view.ready_queues
            .iter()
            .flat_map(|(_, queue)| queue.iter())
            .min_by_key(|job| {
                (
                    job.priority,
                    job.absolute_deadline,
                    job.task_id.0,
                    job.job_id.0,
                )
            })
            .map(|job| job.task_id)
    }

    fn best_running_for_task<'a>(
        view: &'a SchedulerView<'_>,
        task_id: TaskId,
    ) -> Option<&'a hprss_types::RunningJobInfo> {
        view.running_jobs
            .iter()
            .filter_map(|(_, running)| running.as_ref())
            .filter(|running| running.task_id == task_id)
            .min_by_key(|running| {
                (
                    running.priority,
                    running.absolute_deadline,
                    running.job_id.0,
                )
            })
    }

    fn best_job_for_task(
        device_id: DeviceId,
        task_id: TaskId,
        view: &SchedulerView<'_>,
    ) -> Option<hprss_types::JobId> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| {
                queue
                    .iter()
                    .filter(|job| job.task_id == task_id)
                    .min_by_key(|job| {
                        (
                            job.priority,
                            job.absolute_deadline,
                            job.release_time,
                            job.job_id.0,
                        )
                    })
            })
            .map(|job| job.job_id)
    }

    fn pick_idle_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates
            .iter()
            .copied()
            .filter(|device_id| {
                view.running_jobs
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .and_then(|(_, running)| running.as_ref())
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
}

impl Scheduler for GangScheduler {
    fn name(&self) -> &str {
        "Gang"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        self.refresh_active_task(view);
        let candidates = Self::compatible_devices(task, view);
        if candidates.is_empty() {
            return vec![Action::NoOp];
        }

        let active = self.active_task.get_or_insert(job.task_id);
        if *active != job.task_id && Self::has_running_task(view, *active) {
            let can_switch = Self::best_running_for_task(view, *active).is_some_and(|running| {
                job.effective_priority < running.priority
                    || (job.effective_priority == running.priority
                        && job.absolute_deadline < running.absolute_deadline)
            });
            if can_switch {
                *active = job.task_id;
            } else {
                let Some(device_id) = Self::pick_enqueue_device(&candidates, view) else {
                    return vec![Action::NoOp];
                };
                return vec![Action::Enqueue {
                    job_id: job.id,
                    device_id,
                }];
            }
        }

        if *active != job.task_id {
            *active = job.task_id;
        }

        if let Some(device_id) = Self::pick_idle_device(&candidates, view) {
            return vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }];
        }

        let preemption_target = candidates
            .iter()
            .filter_map(|device_id| {
                view.running_jobs
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .and_then(|(_, running)| running.as_ref())
                    .map(|running| (*device_id, running))
            })
            .filter(|(_, running)| running.task_id != job.task_id)
            .max_by_key(|(_, running)| {
                (
                    running.priority,
                    running.absolute_deadline,
                    running.job_id.0,
                )
            });

        if let Some((device_id, running)) = preemption_target
            && (job.effective_priority < running.priority
                || (job.effective_priority == running.priority
                    && job.absolute_deadline < running.absolute_deadline))
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
        self.refresh_active_task(view);
        let Some(active_task) = self.active_task else {
            return vec![Action::NoOp];
        };
        match Self::best_job_for_task(device_id, active_task, view) {
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
        self.refresh_active_task(view);
        let Some(active_task) = self.active_task else {
            return vec![Action::NoOp];
        };
        if running_job.task_id == active_task {
            return vec![Action::NoOp];
        }

        let next = Self::best_job_for_task(device_id, active_task, view);
        match next {
            Some(job_id) => vec![Action::Preempt {
                victim: running_job.id,
                by: job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }

    fn on_criticality_change(
        &mut self,
        new_level: CriticalityLevel,
        _trigger_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.refresh_active_task(view);
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
        self.refresh_active_task(view);
        let Some(active_task) = self.active_task else {
            return vec![Action::NoOp];
        };
        match Self::best_job_for_task(device_id, active_task, view) {
            Some(job_id) => vec![Action::Dispatch { job_id, device_id }],
            None => vec![Action::NoOp],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        DeviceId, JobId, JobState,
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

    fn cpu_task(id: u32, priority: u32) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority,
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

    fn job(id: u64, task_id: u32, priority: u32, deadline: u64) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(task_id),
            state: JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: deadline,
            actual_exec_ns: Some(10_000),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: priority,
        }
    }

    #[test]
    fn arrival_of_non_active_task_is_queued_while_active_gang_runs() {
        let mut sched = GangScheduler::default();
        let active_task = cpu_task(1, 1);
        let first = job(1, 1, 1, 40_000);
        let devices = [cpu_device(0), cpu_device(1)];
        let start_view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&first, &active_task, &start_view);

        let other_task = cpu_task(2, 5);
        let other = job(2, 2, 5, 30_000);
        let running_view = SchedulerView {
            now: 1_000,
            devices: &devices,
            running_jobs: &[
                (
                    DeviceId(0),
                    Some(RunningJobInfo {
                        job_id: first.id,
                        task_id: first.task_id,
                        priority: first.effective_priority,
                        release_time: first.release_time,
                        absolute_deadline: first.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 100,
                    }),
                ),
                (DeviceId(1), None),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&other, &other_task, &running_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(2),
                ..
            }]
        ));
    }

    #[test]
    fn higher_priority_arrival_can_preempt_non_gang_job() {
        let mut sched = GangScheduler::default();
        sched.active_task = Some(TaskId(1));
        let high_task = cpu_task(1, 1);
        let high = job(10, 1, 1, 20_000);
        let devices = [cpu_device(0)];
        let view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(7),
                    task_id: TaskId(3),
                    priority: 7,
                    release_time: 0,
                    absolute_deadline: 90_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&high, &high_task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(7),
                by: JobId(10),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn idle_keeps_device_idle_when_other_gang_running_elsewhere() {
        let mut sched = GangScheduler::default();
        sched.active_task = Some(TaskId(1));
        let devices = [cpu_device(0), cpu_device(1)];
        let view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[
                (DeviceId(0), None),
                (
                    DeviceId(1),
                    Some(RunningJobInfo {
                        job_id: JobId(9),
                        task_id: TaskId(1),
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: 30_000,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 100,
                    }),
                ),
            ],
            ready_queues: &[
                (
                    DeviceId(0),
                    vec![hprss_types::QueuedJobInfo {
                        job_id: JobId(2),
                        task_id: TaskId(2),
                        priority: 5,
                        release_time: 0,
                        absolute_deadline: 50_000,
                        criticality: CriticalityLevel::Lo,
                    }],
                ),
                (DeviceId(1), vec![]),
            ],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_device_idle(DeviceId(0), &view);
        assert!(matches!(actions.as_slice(), [Action::NoOp]));
    }
}
