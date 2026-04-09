use hprss_types::{Action, CriticalityLevel, DeviceId, Job, Scheduler, SchedulerView, task::Task};

/// Earliest-Deadline-First scheduler for heterogeneous devices.
pub struct EdfScheduler;

impl Scheduler for EdfScheduler {
    fn name(&self) -> &str {
        "EDF-Het"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
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
                        if job.absolute_deadline < running_info.absolute_deadline {
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
            None => vec![Action::NoOp],
        }
    }

    fn on_job_complete(
        &mut self,
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| {
                q.iter()
                    .min_by_key(|j| (j.absolute_deadline, j.release_time, j.job_id.0))
            });

        match next {
            Some(job) => vec![Action::Dispatch {
                job_id: job.job_id,
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
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| {
                q.iter()
                    .min_by_key(|j| (j.absolute_deadline, j.release_time, j.job_id.0))
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
        if new_level == CriticalityLevel::Hi {
            let mut actions = Vec::new();
            for (_, queue) in view.ready_queues {
                for job in queue {
                    if job.criticality == CriticalityLevel::Lo {
                        actions.push(Action::DropJob { job_id: job.job_id });
                    }
                }
            }
            actions
        } else {
            vec![Action::NoOp]
        }
    }

    fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| {
                q.iter()
                    .min_by_key(|j| (j.absolute_deadline, j.release_time, j.job_id.0))
            });

        match next {
            Some(job) => vec![Action::Dispatch {
                job_id: job.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }
}
