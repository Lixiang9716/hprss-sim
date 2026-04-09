use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, QueuedJobInfo, RunningJobInfo, Scheduler,
    SchedulerView, device::PreemptionModel, task::Task,
};

/// GPU-focused fixed-priority scheduler with limited-preemption awareness.
#[derive(Debug, Default)]
pub struct GpuPreemptivePriorityScheduler;

impl Scheduler for GpuPreemptivePriorityScheduler {
    fn name(&self) -> &str {
        "GPU-Preemptive-Priority"
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

        let incoming_rank = Self::arrival_rank(task.criticality, job);
        if let Some((device_id, running)) =
            Self::pick_arrival_preemption_target(&candidates, view, incoming_rank)
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
        _running_job: &Job,
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
                victim: running.job_id,
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

impl GpuPreemptivePriorityScheduler {
    fn criticality_rank(level: CriticalityLevel) -> u8 {
        match level {
            CriticalityLevel::Hi => 0,
            CriticalityLevel::Lo => 1,
        }
    }

    fn arrival_rank(criticality: CriticalityLevel, job: &Job) -> (u8, u32, u64, u64) {
        (
            Self::criticality_rank(criticality),
            job.effective_priority,
            job.release_time,
            job.id.0,
        )
    }

    fn running_rank(running: &RunningJobInfo) -> (u8, u32, u64, u64) {
        (
            Self::criticality_rank(running.criticality),
            running.priority,
            running.release_time,
            running.job_id.0,
        )
    }

    fn queued_rank(queued: &QueuedJobInfo) -> (u8, u32, u64, u64) {
        (
            Self::criticality_rank(queued.criticality),
            queued.priority,
            queued.release_time,
            queued.job_id.0,
        )
    }

    fn device<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::DeviceConfig> {
        view.devices.iter().find(|device| device.id == device_id)
    }

    fn compatible_devices(task: &Task, view: &SchedulerView<'_>) -> Vec<DeviceId> {
        let mut gpu = Vec::new();
        let mut non_gpu = Vec::new();
        for affinity in &task.affinity {
            for device in view.devices.iter().filter(|d| d.device_type == *affinity) {
                if device.device_type == hprss_types::task::DeviceType::Gpu {
                    gpu.push(device.id);
                } else {
                    non_gpu.push(device.id);
                }
            }
        }
        if !gpu.is_empty() { gpu } else { non_gpu }
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
            .and_then(|queue| queue.iter().min_by_key(|queued| Self::queued_rank(queued)))
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
                let queue_len = Self::queue_for_device(*device_id, view).map_or(0, Vec::len);
                (queue_len, device_id.0)
            })
    }

    fn pick_enqueue_device(candidates: &[DeviceId], view: &SchedulerView<'_>) -> Option<DeviceId> {
        candidates.iter().copied().min_by_key(|device_id| {
            let running = Self::running_on_device(*device_id, view).is_some() as u8;
            let queue_len = Self::queue_for_device(*device_id, view).map_or(0, Vec::len);
            (running, queue_len, device_id.0)
        })
    }

    fn pick_arrival_preemption_target<'a>(
        candidates: &[DeviceId],
        view: &'a SchedulerView<'_>,
        incoming_rank: (u8, u32, u64, u64),
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
                (rank.0, rank.1, rank.2, rank.3, device_id.0)
            })
    }
}
