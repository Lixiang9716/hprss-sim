//! Scheduler trait implementations.
//!
//! Built-in algorithms: Fixed Priority, EDF, LLF (heterogeneous variants).

use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};

/// Fixed-Priority scheduler (Rate Monotonic as default priority assignment)
pub struct FixedPriorityScheduler;

impl Scheduler for FixedPriorityScheduler {
    fn name(&self) -> &str {
        "FP-Het"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        // Basic strategy: find the first available device in affinity list,
        // check if preemption is needed
        let target_device = task
            .affinity
            .iter()
            .filter_map(|dt| {
                view.devices
                    .iter()
                    .find(|d| d.device_type == *dt)
                    .map(|d| d.id)
            })
            .next();

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

    #[test]
    fn fp_scheduler_name() {
        let sched = FixedPriorityScheduler;
        assert_eq!(sched.name(), "FP-Het");
    }
}
