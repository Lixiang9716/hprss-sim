use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};
use hprss_types::{QueuedJobInfo, RunningJobInfo, device::PreemptionModel};

/// RTGPU scheduler baseline.
#[derive(Debug, Default)]
pub struct RtgpuScheduler;

impl Scheduler for RtgpuScheduler {
    fn name(&self) -> &str {
        "RTGPU"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let candidates = Self::compatible_devices(task, view);
        if candidates.is_empty() {
            return vec![Action::NoOp];
        }

        if let Some(device_id) = Self::pick_idle_device(task, &candidates, view) {
            return vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }];
        }

        let incoming_rank = Self::arrival_rank(task, job);
        if let Some((device_id, running)) =
            Self::pick_arrival_preemption_target(&candidates, view, incoming_rank)
        {
            return vec![Action::Preempt {
                victim: running.job_id,
                by: job.id,
                device_id,
            }];
        }

        let Some(device_id) = Self::pick_enqueue_device(task, &candidates, view) else {
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
        let Some(device) = Self::device(device_id, view) else {
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
            for queued in queue {
                if queued.criticality == CriticalityLevel::Lo {
                    actions.push(Action::DropJob {
                        job_id: queued.job_id,
                    });
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

impl RtgpuScheduler {
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

    fn running_rank(running: &RunningJobInfo) -> (u8, u64, u32, u64, u64) {
        (
            Self::criticality_rank(running.criticality),
            running.absolute_deadline,
            running.priority,
            running.release_time,
            running.job_id.0,
        )
    }

    fn queued_rank(job: &QueuedJobInfo) -> (u8, u64, u32, u64, u64) {
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
                    .filter(move |d| d.device_type == *device_type)
                    .map(|d| d.id)
            })
            .collect()
    }

    fn device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::DeviceConfig> {
        view.devices.iter().find(|device| device.id == device_id)
    }

    fn running_on_device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a RunningJobInfo> {
        view.running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref())
    }

    fn queue_for_device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a Vec<QueuedJobInfo>> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .map(|(_, queue)| queue)
    }

    fn best_waiting_job<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a QueuedJobInfo> {
        Self::queue_for_device(device_id, view)
            .and_then(|queue| queue.iter().min_by_key(|job| Self::queued_rank(job)))
    }

    fn can_preempt_on_arrival(model: &PreemptionModel) -> bool {
        matches!(model, PreemptionModel::FullyPreemptive)
    }

    fn can_preempt_at_checkpoint(model: &PreemptionModel) -> bool {
        !matches!(model, PreemptionModel::NonPreemptive { .. })
    }

    fn estimated_service_ns(task: &Task, device: &hprss_types::DeviceConfig) -> u64 {
        let wcet = task.wcet_on(device.device_type).unwrap_or(u64::MAX / 4) as f64;
        let speed = device.speed_factor.max(0.001);
        (wcet / speed).ceil() as u64
    }

    fn queue_pressure(task: &Task, device_id: DeviceId, view: &SchedulerView<'_>) -> Option<u64> {
        let device = Self::device(device_id, view)?;
        let service = Self::estimated_service_ns(task, device);
        let queue_len = Self::queue_for_device(device_id, view)
            .map(|queue| queue.len() as u64)
            .unwrap_or(0);
        let has_running = Self::running_on_device(device_id, view).is_some() as u64;
        let blocking = if has_running == 0 {
            0
        } else {
            device.preemption.max_blocking().min(1_000_000_000)
        };
        Some((queue_len + has_running) * service + blocking)
    }

    fn device_kind_rank(device: &hprss_types::DeviceConfig) -> u8 {
        match device.device_type {
            hprss_types::task::DeviceType::Gpu => 0,
            _ => 1,
        }
    }

    fn pick_idle_device(
        task: &Task,
        candidates: &[DeviceId],
        view: &SchedulerView<'_>,
    ) -> Option<DeviceId> {
        candidates
            .iter()
            .copied()
            .filter(|device_id| Self::running_on_device(*device_id, view).is_none())
            .min_by_key(|device_id| {
                let pressure = Self::queue_pressure(task, *device_id, view).unwrap_or(u64::MAX);
                let device = Self::device(*device_id, view).expect("candidate must exist");
                (pressure, Self::device_kind_rank(device), device_id.0)
            })
    }

    fn pick_enqueue_device(
        task: &Task,
        candidates: &[DeviceId],
        view: &SchedulerView<'_>,
    ) -> Option<DeviceId> {
        candidates.iter().copied().min_by_key(|device_id| {
            let pressure = Self::queue_pressure(task, *device_id, view).unwrap_or(u64::MAX);
            let device = Self::device(*device_id, view).expect("candidate must exist");
            (
                pressure,
                Self::running_on_device(*device_id, view).is_some() as u8,
                Self::device_kind_rank(device),
                device_id.0,
            )
        })
    }

    fn pick_arrival_preemption_target<'a>(
        candidates: &[DeviceId],
        view: &'a SchedulerView<'_>,
        incoming_rank: (u8, u64, u32, u64, u64),
    ) -> Option<(DeviceId, &'a RunningJobInfo)> {
        candidates
            .iter()
            .filter_map(|device_id| {
                let device = Self::device(*device_id, view)?;
                if !Self::can_preempt_on_arrival(&device.preemption) {
                    return None;
                }
                let running = Self::running_on_device(*device_id, view)?;
                if incoming_rank < Self::running_rank(running) {
                    Some((*device_id, running))
                } else {
                    None
                }
            })
            .max_by_key(|(device_id, running)| {
                let rank = Self::running_rank(running);
                (rank.0, rank.1, rank.2, rank.3, rank.4, device_id.0)
            })
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

    fn device(
        id: u32,
        device_type: DeviceType,
        preemption: PreemptionModel,
        speed_factor: f64,
    ) -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(id),
            name: format!("{device_type:?}-{id}"),
            device_group: None,
            device_type,
            cores: 1,
            preemption,
            context_switch_ns: 0,
            speed_factor,
            multicore_policy: None,
            power_watts: None,
        }
    }

    fn task(id: u32, affinity: Vec<DeviceType>, criticality: CriticalityLevel) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 10,
            arrival: ArrivalModel::Aperiodic,
            deadline: 50_000,
            criticality,
            exec_times: vec![
                (
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 15_000 },
                ),
                (
                    DeviceType::Gpu,
                    ExecutionTimeModel::Deterministic { wcet: 8_000 },
                ),
            ],
            affinity,
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
    fn rtgpu_scheduler_name() {
        assert_eq!(RtgpuScheduler.name(), "RTGPU");
    }

    #[test]
    fn arrival_prefers_gpu_when_cpu_and_gpu_are_idle() {
        let mut sched = RtgpuScheduler;
        let incoming = job(20, 40_000, 4);
        let task = task(
            1,
            vec![DeviceType::Cpu, DeviceType::Gpu],
            CriticalityLevel::Lo,
        );
        let devices = [
            device(0, DeviceType::Cpu, PreemptionModel::FullyPreemptive, 1.0),
            device(
                1,
                DeviceType::Gpu,
                PreemptionModel::LimitedPreemptive {
                    granularity_ns: 5_000,
                },
                4.0,
            ),
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
                job_id: JobId(20),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn arrival_uses_less_contended_cpu_when_gpu_queue_is_backlogged() {
        let mut sched = RtgpuScheduler;
        let incoming = job(30, 45_000, 3);
        let task = task(
            2,
            vec![DeviceType::Cpu, DeviceType::Gpu],
            CriticalityLevel::Lo,
        );
        let devices = [
            device(0, DeviceType::Cpu, PreemptionModel::FullyPreemptive, 1.0),
            device(
                1,
                DeviceType::Gpu,
                PreemptionModel::LimitedPreemptive {
                    granularity_ns: 8_000,
                },
                4.0,
            ),
        ];
        let view = SchedulerView {
            now: 100,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[
                (DeviceId(0), vec![]),
                (
                    DeviceId(1),
                    vec![
                        queued(31, 60_000, 5, CriticalityLevel::Lo, 10),
                        queued(32, 70_000, 6, CriticalityLevel::Lo, 20),
                        queued(33, 80_000, 7, CriticalityLevel::Lo, 30),
                    ],
                ),
            ],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(30),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn arrival_on_limited_preemptive_gpu_enqueues_instead_of_immediate_preempt() {
        let mut sched = RtgpuScheduler;
        let incoming = job(41, 20_000, 1);
        let task = task(3, vec![DeviceType::Gpu], CriticalityLevel::Hi);
        let devices = [device(
            1,
            DeviceType::Gpu,
            PreemptionModel::LimitedPreemptive {
                granularity_ns: 5_000,
            },
            3.0,
        )];
        let view = SchedulerView {
            now: 200,
            devices: &devices,
            running_jobs: &[(
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: JobId(9),
                    task_id: TaskId(9),
                    priority: 9,
                    release_time: 0,
                    absolute_deadline: 90_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 1_000,
                }),
            )],
            ready_queues: &[(DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &task, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(41),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn preemption_point_can_preempt_gpu_job_at_checkpoint() {
        let mut sched = RtgpuScheduler;
        let running_job = job(50, 60_000, 8);
        let devices = [device(
            1,
            DeviceType::Gpu,
            PreemptionModel::LimitedPreemptive {
                granularity_ns: 5_000,
            },
            3.0,
        )];
        let view = SchedulerView {
            now: 1_000,
            devices: &devices,
            running_jobs: &[(
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: JobId(50),
                    task_id: TaskId(50),
                    priority: 8,
                    release_time: 0,
                    absolute_deadline: 60_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 4_000,
                }),
            )],
            ready_queues: &[(
                DeviceId(1),
                vec![queued(51, 30_000, 2, CriticalityLevel::Hi, 10)],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_preemption_point(DeviceId(1), &running_job, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(50),
                by: JobId(51),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn criticality_change_drops_low_criticality_ready_jobs() {
        let mut sched = RtgpuScheduler;
        let trigger = job(99, 100_000, 5);
        let devices = [device(
            0,
            DeviceType::Cpu,
            PreemptionModel::FullyPreemptive,
            1.0,
        )];
        let view = SchedulerView {
            now: 5_000,
            devices: &devices,
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(70, 50_000, 5, CriticalityLevel::Lo, 1),
                    queued(71, 55_000, 4, CriticalityLevel::Hi, 2),
                    queued(72, 60_000, 6, CriticalityLevel::Lo, 3),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_criticality_change(CriticalityLevel::Hi, &trigger, &view);
        assert!(matches!(
            actions.as_slice(),
            [
                Action::DropJob { job_id: JobId(70) },
                Action::DropJob { job_id: JobId(72) }
            ]
        ));
    }
}
