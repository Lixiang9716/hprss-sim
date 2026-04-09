use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, device::PreemptionModel,
    task::Task,
};

/// GPreempt scheduler baseline.
#[derive(Debug, Default)]
pub struct GPreemptScheduler;

impl GPreemptScheduler {
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

    fn running_on_device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::RunningJobInfo> {
        view.running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref())
    }

    fn queue_for_device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a Vec<hprss_types::QueuedJobInfo>> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .map(|(_, queue)| queue)
    }

    fn best_waiting_job<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::QueuedJobInfo> {
        Self::queue_for_device(device_id, view)
            .and_then(|queue| queue.iter().min_by_key(|job| Self::queued_rank(job)))
    }

    fn can_preempt_on_arrival(model: &PreemptionModel) -> bool {
        matches!(model, PreemptionModel::FullyPreemptive)
    }

    fn can_preempt_at_checkpoint(model: &PreemptionModel) -> bool {
        !matches!(model, PreemptionModel::NonPreemptive { .. })
    }

    fn pick_idle_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates
            .iter()
            .copied()
            .filter(|device_id| Self::running_on_device(*device_id, view).is_none())
            .min_by_key(|device_id| {
                let queue_len = Self::queue_for_device(*device_id, view).map_or(0, |q| q.len());
                (queue_len, device_id.0)
            })
    }

    fn pick_enqueue_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates.iter().copied().min_by_key(|device_id| {
            let running = Self::running_on_device(*device_id, view).is_some() as u8;
            let queue_len = Self::queue_for_device(*device_id, view).map_or(0, |q| q.len());
            (running, queue_len, device_id.0)
        })
    }

    fn pick_arrival_preemption_target<'a>(
        candidates: &[DeviceId],
        view: &'a SchedulerView<'_>,
    ) -> Option<(DeviceId, &'a hprss_types::RunningJobInfo)> {
        candidates
            .iter()
            .filter_map(|device_id| {
                let running = Self::running_on_device(*device_id, view)?;
                let device = view.devices.iter().find(|device| device.id == *device_id)?;
                if !Self::can_preempt_on_arrival(&device.preemption) {
                    return None;
                }
                Some((*device_id, running))
            })
            .max_by_key(|(device_id, running)| {
                let rank = Self::running_rank(running);
                (rank.0, rank.1, rank.2, rank.3, rank.4, device_id.0)
            })
    }
}

impl Scheduler for GPreemptScheduler {
    fn name(&self) -> &str {
        "GPreempt"
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

        let incoming_rank = Self::arrival_rank(task, job);
        if let Some((device_id, running)) = Self::pick_arrival_preemption_target(&candidates, view)
            && incoming_rank < Self::running_rank(running)
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
        let Some(device) = view.devices.iter().find(|device| device.id == device_id) else {
            return vec![Action::NoOp];
        };
        if !Self::can_preempt_at_checkpoint(&device.preemption) {
            return vec![Action::NoOp];
        }

        let Some(running) = Self::running_on_device(device_id, view) else {
            return vec![Action::NoOp];
        };
        let Some(waiting) = Self::best_waiting_job(device_id, view) else {
            return vec![Action::NoOp];
        };

        if Self::queued_rank(waiting) < Self::running_rank(running) {
            vec![Action::Preempt {
                victim: running_job.id,
                by: waiting.job_id,
                device_id,
            }]
        } else {
            vec![Action::NoOp]
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

    fn cpu_device(id: u32, preemption: PreemptionModel) -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(id),
            name: format!("cpu-{id}"),
            device_group: None,
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption,
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
            release_time: id * 100,
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
    fn gpreempt_scheduler_name() {
        let sched = GPreemptScheduler;
        assert_eq!(sched.name(), "GPreempt");
    }

    #[test]
    fn arrival_prefers_idle_compatible_device_with_deterministic_tie_break() {
        let mut sched = GPreemptScheduler;
        let incoming = job(10, 20_000, 4);
        let task = cpu_task(1, CriticalityLevel::Lo);
        let devices = [
            cpu_device(0, PreemptionModel::FullyPreemptive),
            cpu_device(1, PreemptionModel::FullyPreemptive),
        ];
        let view = SchedulerView {
            now: 0,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

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
    fn arrival_on_limited_preemptive_device_enqueues_until_checkpoint() {
        let mut sched = GPreemptScheduler;
        let incoming = job(22, 10_000, 1);
        let high_task = cpu_task(22, CriticalityLevel::Hi);
        let devices = [cpu_device(
            0,
            PreemptionModel::LimitedPreemptive {
                granularity_ns: 5_000,
            },
        )];
        let view = SchedulerView {
            now: 100,
            devices: &devices,
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(7),
                    task_id: TaskId(7),
                    priority: 9,
                    release_time: 0,
                    absolute_deadline: 80_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 1_000,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &high_task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(22),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn preemption_point_preempts_on_limited_preemptive_device() {
        let mut sched = GPreemptScheduler;
        let running_job = job(5, 90_000, 7);
        let devices = [cpu_device(
            0,
            PreemptionModel::LimitedPreemptive {
                granularity_ns: 5_000,
            },
        )];
        let view = SchedulerView {
            now: 1_000,
            devices: &devices,
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(5),
                    task_id: TaskId(5),
                    priority: 7,
                    release_time: 0,
                    absolute_deadline: 90_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 2_000,
                }),
            )],
            ready_queues: &[(
                DeviceId(0),
                vec![queued(6, 20_000, 1, CriticalityLevel::Hi, 10)],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_preemption_point(DeviceId(0), &running_job, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(5),
                by: JobId(6),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn completion_and_idle_use_same_best_job_ordering() {
        let mut sched = GPreemptScheduler;
        let finished = job(30, 100_000, 10);
        let devices = [cpu_device(0, PreemptionModel::FullyPreemptive)];
        let view = SchedulerView {
            now: 2_000,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(40, 40_000, 5, CriticalityLevel::Lo, 0),
                    queued(41, 30_000, 8, CriticalityLevel::Lo, 0),
                    queued(42, 70_000, 1, CriticalityLevel::Hi, 1),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let on_complete = sched.on_job_complete(&finished, DeviceId(0), &view);
        let on_idle = sched.on_device_idle(DeviceId(0), &view);

        assert!(matches!(
            on_complete.as_slice(),
            [Action::Dispatch {
                job_id: JobId(42),
                device_id: DeviceId(0)
            }]
        ));
        assert!(matches!(
            on_idle.as_slice(),
            [Action::Dispatch {
                job_id: JobId(42),
                device_id: DeviceId(0)
            }]
        ));
    }
}
