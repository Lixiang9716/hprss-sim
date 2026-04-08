use std::collections::{HashMap, HashSet};

use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, JobId, ReevaluationPolicy, ReevaluationTrigger,
    Scheduler, SchedulerView, task::Task,
};

/// Least-Laxity-First scheduler for heterogeneous devices.
#[derive(Debug, Clone)]
pub struct LlfScheduler {
    reevaluation_policy: ReevaluationPolicy,
    remaining_work_ns: HashMap<JobId, u64>,
    running_elapsed_ns: HashMap<JobId, u64>,
}

impl Default for LlfScheduler {
    fn default() -> Self {
        Self::new(ReevaluationPolicy::Hybrid {
            interval_ns: 5_000,
            min_interval_ns: 1_000,
        })
    }
}

impl LlfScheduler {
    pub fn new(reevaluation_policy: ReevaluationPolicy) -> Self {
        Self {
            reevaluation_policy,
            remaining_work_ns: HashMap::new(),
            running_elapsed_ns: HashMap::new(),
        }
    }

    fn observe_view(&mut self, view: &SchedulerView<'_>) {
        let mut visible_jobs = HashSet::new();
        let mut running_jobs = HashSet::new();

        for (_, running) in view.running_jobs {
            if let Some(running) = running {
                visible_jobs.insert(running.job_id);
                running_jobs.insert(running.job_id);

                let last_elapsed = self.running_elapsed_ns.get(&running.job_id).copied();
                let delta = match last_elapsed {
                    Some(prev) if running.elapsed_ns >= prev => running.elapsed_ns - prev,
                    Some(_) | None => running.elapsed_ns,
                };

                if let Some(remaining) = self.remaining_work_ns.get_mut(&running.job_id) {
                    *remaining = remaining.saturating_sub(delta);
                }
                self.running_elapsed_ns
                    .insert(running.job_id, running.elapsed_ns);
            }
        }

        for (_, queue) in view.ready_queues {
            for queued in queue {
                visible_jobs.insert(queued.job_id);
            }
        }

        self.running_elapsed_ns
            .retain(|job_id, _| running_jobs.contains(job_id));
        self.remaining_work_ns
            .retain(|job_id, _| visible_jobs.contains(job_id));
    }

    fn estimate_total_work_ns(job: &Job, task: &Task) -> u64 {
        job.actual_exec_ns.unwrap_or_else(|| {
            task.exec_times
                .iter()
                .map(|(_, model)| model.wcet())
                .min()
                .unwrap_or(0)
        })
    }

    fn laxity(now: u64, absolute_deadline: u64, remaining_work: u64) -> i128 {
        i128::from(absolute_deadline) - i128::from(now) - i128::from(remaining_work)
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
                q.iter().min_by_key(|job| {
                    let remaining = self
                        .remaining_work_ns
                        .get(&job.job_id)
                        .copied()
                        .unwrap_or(0);
                    (
                        Self::laxity(view.now, job.absolute_deadline, remaining),
                        job.absolute_deadline,
                        job.release_time,
                        job.job_id.0,
                    )
                })
            })
    }

    fn running_laxity(&self, running: &hprss_types::RunningJobInfo, now: u64) -> i128 {
        let remaining = self
            .remaining_work_ns
            .get(&running.job_id)
            .copied()
            .unwrap_or(0);
        Self::laxity(now, running.absolute_deadline, remaining)
    }
}

impl Scheduler for LlfScheduler {
    fn name(&self) -> &str {
        "LLF-Het"
    }

    fn reevaluation_policy(&self) -> ReevaluationPolicy {
        self.reevaluation_policy
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        self.observe_view(view);
        self.remaining_work_ns
            .insert(job.id, Self::estimate_total_work_ns(job, task));
        self.running_elapsed_ns.remove(&job.id);

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
                let incoming_remaining = self.remaining_work_ns.get(&job.id).copied().unwrap_or(0);
                let incoming_laxity =
                    Self::laxity(view.now, job.absolute_deadline, incoming_remaining);
                let running_laxity = self.running_laxity(running_info, view.now);

                if incoming_laxity < running_laxity {
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
        job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.observe_view(view);
        self.remaining_work_ns.remove(&job.id);
        self.running_elapsed_ns.remove(&job.id);

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
        self.observe_view(view);
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, info)| info.as_ref());
        let waiting = self.best_waiting_job(device_id, view);

        match (running, waiting) {
            (Some(running_info), Some(waiting))
                if Self::laxity(
                    view.now,
                    waiting.absolute_deadline,
                    self.remaining_work_ns
                        .get(&waiting.job_id)
                        .copied()
                        .unwrap_or(0),
                ) < self.running_laxity(running_info, view.now) =>
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
        self.observe_view(view);
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

    fn on_reevaluation(
        &mut self,
        _trigger: ReevaluationTrigger,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.observe_view(view);
        let mut actions = Vec::new();

        for (device_id, running) in view.running_jobs {
            let Some(running) = running else {
                continue;
            };
            let Some(waiting) = self.best_waiting_job(*device_id, view) else {
                continue;
            };

            let waiting_laxity = Self::laxity(
                view.now,
                waiting.absolute_deadline,
                self.remaining_work_ns
                    .get(&waiting.job_id)
                    .copied()
                    .unwrap_or(0),
            );
            let running_laxity = self.running_laxity(running, view.now);

            if waiting_laxity < running_laxity {
                actions.push(Action::Preempt {
                    victim: running.job_id,
                    by: waiting.job_id,
                    device_id: *device_id,
                });
            }
        }

        if actions.is_empty() {
            vec![Action::NoOp]
        } else {
            actions
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
        task::{ArrivalModel, CriticalityLevel, DeviceType, ExecutionTimeModel},
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

    fn task(id: u32, wcet: u64) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: id,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(DeviceType::Cpu, ExecutionTimeModel::Deterministic { wcet })],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        }
    }

    fn job(id: u64, task_id: u32, deadline: u64, exec_ns: u64) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(task_id),
            state: JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: deadline,
            actual_exec_ns: Some(exec_ns),
            dag_provenance: None,
            executed_ns: 0,
            assigned_device: Some(DeviceId(0)),
            exec_start_time: None,
            effective_priority: 1,
        }
    }

    #[test]
    fn llf_dispatches_smallest_laxity_on_completion() {
        let mut sched = LlfScheduler::new(ReevaluationPolicy::Disabled);
        let device = cpu_device();
        let running = job(1, 1, 100, 40);
        let waiting_a = job(2, 2, 70, 20);
        let waiting_b = job(3, 3, 65, 30);

        let arrival_view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 0,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&waiting_a, &task(2, 20), &arrival_view);
        let _ = sched.on_job_arrival(&waiting_b, &task(3, 30), &arrival_view);

        let complete_view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    QueuedJobInfo {
                        job_id: waiting_a.id,
                        task_id: waiting_a.task_id,
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: waiting_a.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                    },
                    QueuedJobInfo {
                        job_id: waiting_b.id,
                        task_id: waiting_b.task_id,
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: waiting_b.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                    },
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_complete(&running, DeviceId(0), &complete_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(3),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn llf_reevaluation_preempts_when_waiting_job_laxity_becomes_smaller() {
        let mut sched = LlfScheduler::default();
        let device = cpu_device();
        let running = job(1, 1, 100, 40);
        let waiting = job(2, 2, 90, 20);

        let first_arrival_view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&running, &task(1, 40), &first_arrival_view);

        let second_arrival_view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 0,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let arrival_actions = sched.on_job_arrival(&waiting, &task(2, 20), &second_arrival_view);
        assert!(matches!(
            arrival_actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(2),
                device_id: DeviceId(0)
            }]
        ));

        let reevaluation_view = SchedulerView {
            now: 30,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 30,
                }),
            )],
            ready_queues: &[(
                DeviceId(0),
                vec![QueuedJobInfo {
                    job_id: waiting.id,
                    task_id: waiting.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: waiting.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                }],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_reevaluation(ReevaluationTrigger::Periodic, &reevaluation_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(1),
                by: JobId(2),
                device_id: DeviceId(0)
            }]
        ));
    }
}
