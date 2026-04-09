use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};

/// Global Criticality-Aware Priority Scheduler baseline.
#[derive(Debug, Default)]
pub struct GcapsScheduler;

impl GcapsScheduler {
    fn criticality_rank(level: CriticalityLevel) -> u8 {
        match level {
            CriticalityLevel::Hi => 0,
            CriticalityLevel::Lo => 1,
        }
    }

    fn arrival_rank(task: &Task, job: &Job) -> (u8, u64, u32, u64, u64) {
        (
            Self::criticality_rank(task.criticality),
            job.absolute_deadline,
            job.effective_priority,
            job.release_time,
            job.id.0,
        )
    }

    fn running_rank(running: &hprss_types::RunningJobInfo) -> (u8, u64, u32, u64, u64) {
        (
            Self::criticality_rank(running.criticality),
            running.absolute_deadline,
            running.priority,
            running.release_time,
            running.job_id.0,
        )
    }

    fn queued_rank(job: &hprss_types::QueuedJobInfo) -> (u8, u64, u32, u64, u64) {
        (
            Self::criticality_rank(job.criticality),
            job.absolute_deadline,
            job.priority,
            job.release_time,
            job.job_id.0,
        )
    }

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

    fn select_target_device(task: &Task, view: &SchedulerView<'_>) -> Option<DeviceId> {
        Self::compatible_devices(task, view)
            .into_iter()
            .min_by_key(|device_id| {
                let is_running = view
                    .running_jobs
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .and_then(|(_, running)| running.as_ref())
                    .is_some();
                let queue_len = view
                    .ready_queues
                    .iter()
                    .find(|(did, _)| *did == *device_id)
                    .map_or(0, |(_, q)| q.len());
                (is_running as u8, queue_len, device_id.0)
            })
    }

    fn best_waiting_job<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::QueuedJobInfo> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| q.iter().min_by_key(|queued| Self::queued_rank(queued)))
    }
}

impl Scheduler for GcapsScheduler {
    fn name(&self) -> &str {
        "GCAPS"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let Some(device_id) = Self::select_target_device(task, view) else {
            return vec![Action::NoOp];
        };

        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref());

        match running {
            None => vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }],
            Some(running) => {
                if Self::arrival_rank(task, job) < Self::running_rank(running) {
                    vec![Action::Preempt {
                        victim: running.job_id,
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
        match Self::best_waiting_job(device_id, view) {
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
            .and_then(|(_, running)| running.as_ref());
        let waiting = Self::best_waiting_job(device_id, view);
        match (running, waiting) {
            (Some(running), Some(waiting))
                if Self::queued_rank(waiting) < Self::running_rank(running) =>
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
        let mut actions = Vec::new();
        for (_, queue) in view.ready_queues {
            for job in queue {
                if job.criticality == CriticalityLevel::Lo {
                    actions.push(Action::DropJob { job_id: job.job_id });
                }
            }
        }
        if actions.is_empty() {
            vec![Action::NoOp]
        } else {
            actions
        }
    }

    fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
        match Self::best_waiting_job(device_id, view) {
            Some(next) => vec![Action::Dispatch {
                job_id: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        JobId, JobState, TaskId,
        device::{DeviceConfig, PreemptionModel},
        scheduler::{QueuedJobInfo, RunningJobInfo},
        task::{ArrivalModel, DeviceType, ExecutionTimeModel},
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

    fn cpu_task(id: u32, criticality: CriticalityLevel) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 10,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 10_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        }
    }

    fn job(id: u64, deadline: u64, priority: u32) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(id as u32),
            state: JobState::Ready,
            version: 0,
            release_time: id * 1_000,
            absolute_deadline: deadline,
            actual_exec_ns: Some(10_000),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: priority,
        }
    }

    fn queued(
        id: u64,
        deadline: u64,
        priority: u32,
        criticality: CriticalityLevel,
        release_time: u64,
    ) -> QueuedJobInfo {
        QueuedJobInfo {
            job_id: JobId(id),
            task_id: TaskId(id as u32),
            priority,
            release_time,
            absolute_deadline: deadline,
            criticality,
        }
    }

    #[test]
    fn gcaps_scheduler_name() {
        let sched = GcapsScheduler;
        assert_eq!(sched.name(), "GCAPS");
    }

    #[test]
    fn arrival_dispatches_to_idle_compatible_device() {
        let mut sched = GcapsScheduler;
        let view = SchedulerView {
            now: 0,
            devices: &[cpu_device(0)],
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let task = cpu_task(1, CriticalityLevel::Lo);
        let incoming = job(10, 20_000, 4);

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(10),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn arrival_preempts_when_incoming_rank_is_better() {
        let mut sched = GcapsScheduler;
        let view = SchedulerView {
            now: 0,
            devices: &[cpu_device(0)],
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(7),
                    task_id: TaskId(7),
                    priority: 5,
                    release_time: 100,
                    absolute_deadline: 50_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 1_000,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let task = cpu_task(2, CriticalityLevel::Hi);
        let incoming = job(8, 90_000, 9);

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
    fn completion_dispatches_best_ranked_waiting_job() {
        let mut sched = GcapsScheduler;
        let finished = job(30, 10_000, 3);
        let view = SchedulerView {
            now: 100,
            devices: &[cpu_device(0)],
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(40, 9_000, 6, CriticalityLevel::Lo, 0),
                    queued(41, 20_000, 1, CriticalityLevel::Hi, 1),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_complete(&finished, DeviceId(0), &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(41),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn preemption_point_preempts_if_waiting_job_is_better() {
        let mut sched = GcapsScheduler;
        let running_job = job(9, 30_000, 7);
        let view = SchedulerView {
            now: 100,
            devices: &[cpu_device(0)],
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(9),
                    task_id: TaskId(9),
                    priority: 7,
                    release_time: 0,
                    absolute_deadline: 30_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 300,
                }),
            )],
            ready_queues: &[(
                DeviceId(0),
                vec![queued(10, 80_000, 9, CriticalityLevel::Hi, 1)],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_preemption_point(DeviceId(0), &running_job, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(9),
                by: JobId(10),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn device_idle_dispatches_best_ranked_waiting_job() {
        let mut sched = GcapsScheduler;
        let view = SchedulerView {
            now: 100,
            devices: &[cpu_device(0)],
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(50, 15_000, 1, CriticalityLevel::Lo, 1),
                    queued(51, 18_000, 9, CriticalityLevel::Hi, 2),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_device_idle(DeviceId(0), &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(51),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn idle_dispatch_uses_job_id_tie_breaker() {
        let mut sched = GcapsScheduler;
        let view = SchedulerView {
            now: 100,
            devices: &[cpu_device(0)],
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(70, 30_000, 5, CriticalityLevel::Lo, 10),
                    queued(71, 30_000, 5, CriticalityLevel::Lo, 10),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_device_idle(DeviceId(0), &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(70),
                device_id: DeviceId(0)
            }]
        ));
    }
}
