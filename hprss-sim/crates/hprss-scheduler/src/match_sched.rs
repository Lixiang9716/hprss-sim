use std::collections::{HashMap, HashSet};

use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, JobId, ReevaluationPolicy, Scheduler, SchedulerView,
    TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::Task,
};

/// MATCH scheduler baseline with deterministic mapping + allocation policy.
#[derive(Debug, Default)]
pub struct MatchScheduler {
    task_mapping: HashMap<TaskId, DeviceId>,
    job_allocations: HashMap<JobId, DeviceId>,
}

impl MatchScheduler {
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

    fn observe_view(&mut self, view: &SchedulerView<'_>) {
        let mut visible_jobs = HashSet::new();
        let mut visible_devices = HashSet::new();
        let mut visible_tasks = HashSet::new();

        for device in view.devices {
            visible_devices.insert(device.id);
        }

        for (_, running) in view.running_jobs {
            if let Some(running) = running {
                visible_jobs.insert(running.job_id);
                visible_tasks.insert(running.task_id);
            }
        }
        for (_, queue) in view.ready_queues {
            for queued in queue {
                visible_jobs.insert(queued.job_id);
                visible_tasks.insert(queued.task_id);
            }
        }

        self.job_allocations.retain(|job_id, device_id| {
            visible_jobs.contains(job_id) && visible_devices.contains(device_id)
        });
        self.task_mapping.retain(|task_id, device_id| {
            visible_tasks.contains(task_id) && visible_devices.contains(device_id)
        });
    }

    fn compatible_devices<'a>(task: &Task, view: &'a SchedulerView<'_>) -> Vec<&'a DeviceConfig> {
        task.affinity
            .iter()
            .flat_map(|device_type| {
                view.devices
                    .iter()
                    .filter(move |device| device.device_type == *device_type)
            })
            .collect()
    }

    fn queue_len(device_id: DeviceId, view: &SchedulerView<'_>) -> usize {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .map_or(0, |(_, queue)| queue.len())
    }

    fn is_running(device_id: DeviceId, view: &SchedulerView<'_>) -> bool {
        view.running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref())
            .is_some()
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

    fn best_waiting_job<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::QueuedJobInfo> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| queue.iter().min_by_key(|job| Self::queued_rank(job)))
    }

    fn service_cost(task: &Task, device: &DeviceConfig) -> u64 {
        let wcet = task.wcet_on(device.device_type).unwrap_or(u64::MAX / 8) as f64;
        let speed = device.speed_factor.max(0.001);
        (wcet / speed).ceil() as u64
    }

    fn allocation_pressure(task: &Task, device: &DeviceConfig, view: &SchedulerView<'_>) -> u64 {
        let service = Self::service_cost(task, device);
        let queue_len = Self::queue_len(device.id, view) as u64;
        let running = Self::is_running(device.id, view) as u64;
        (queue_len + running) * service
    }

    fn preemption_allowed_on_arrival(view: &SchedulerView<'_>, device_id: DeviceId) -> bool {
        let Some(device) = view.devices.iter().find(|device| device.id == device_id) else {
            return false;
        };
        matches!(device.preemption, PreemptionModel::FullyPreemptive)
    }

    fn preemption_allowed_at_checkpoint(view: &SchedulerView<'_>, device_id: DeviceId) -> bool {
        let Some(device) = view.devices.iter().find(|device| device.id == device_id) else {
            return false;
        };
        !matches!(device.preemption, PreemptionModel::NonPreemptive { .. })
    }

    fn choose_target_device(&mut self, task: &Task, view: &SchedulerView<'_>) -> Option<DeviceId> {
        let candidates = Self::compatible_devices(task, view);
        if candidates.is_empty() {
            return None;
        }

        if let Some(mapped) = self.task_mapping.get(&task.id).copied()
            && candidates.iter().any(|device| device.id == mapped)
        {
            return Some(mapped);
        }

        let target = candidates
            .iter()
            .min_by_key(|device| {
                let pressure = Self::allocation_pressure(task, device, view);
                let service_cost = Self::service_cost(task, device);
                let running = Self::is_running(device.id, view) as u8;
                let queue_len = Self::queue_len(device.id, view);
                (pressure, service_cost, running, queue_len, device.id.0)
            })
            .map(|device| device.id)?;
        self.task_mapping.insert(task.id, target);
        Some(target)
    }
}

impl Scheduler for MatchScheduler {
    fn name(&self) -> &str {
        "MATCH"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        self.observe_view(view);
        let Some(device_id) = self.choose_target_device(task, view) else {
            return vec![Action::NoOp];
        };
        self.job_allocations.insert(job.id, device_id);

        let running = Self::running_on_device(device_id, view);
        match running {
            None => vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }],
            Some(running_info) => {
                if Self::arrival_rank(task, job) < Self::running_rank(running_info)
                    && Self::preemption_allowed_on_arrival(view, device_id)
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
        job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.job_allocations.remove(&job.id);
        self.observe_view(view);
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
        self.observe_view(view);
        if !Self::preemption_allowed_at_checkpoint(view, device_id) {
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
        self.observe_view(view);
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
        self.observe_view(view);
        match Self::best_waiting_job(device_id, view) {
            Some(next) => vec![Action::Dispatch {
                job_id: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }

    fn reevaluation_policy(&self) -> ReevaluationPolicy {
        ReevaluationPolicy::Disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        JobState,
        device::PreemptionModel,
        scheduler::{QueuedJobInfo, RunningJobInfo},
        task::{ArrivalModel, DeviceType, ExecutionTimeModel},
    };

    fn device(
        id: u32,
        device_type: DeviceType,
        speed_factor: f64,
        preemption: PreemptionModel,
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

    fn task(id: u32, affinity: Vec<DeviceType>, exec_times: Vec<(DeviceType, u64)>) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 8,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: exec_times
                .into_iter()
                .map(|(device_type, wcet)| {
                    (device_type, ExecutionTimeModel::Deterministic { wcet })
                })
                .collect(),
            affinity,
            data_size: 0,
        }
    }

    fn job(id: u64, task_id: u32, deadline: u64, priority: u32) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(task_id),
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

    fn queued(job: &Job, criticality: CriticalityLevel) -> QueuedJobInfo {
        QueuedJobInfo {
            job_id: job.id,
            task_id: job.task_id,
            priority: job.effective_priority,
            release_time: job.release_time,
            absolute_deadline: job.absolute_deadline,
            criticality,
        }
    }

    #[test]
    fn arrival_picks_fastest_compatible_device_with_deterministic_tie_break() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(0, DeviceType::Cpu, 1.0, PreemptionModel::FullyPreemptive);
        let cpu1 = device(1, DeviceType::Cpu, 2.0, PreemptionModel::FullyPreemptive);
        let t = task(1, vec![DeviceType::Cpu], vec![(DeviceType::Cpu, 20_000)]);
        let j = job(1, 1, 60_000, 5);

        let view = SchedulerView {
            now: 0,
            devices: &[cpu0, cpu1],
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&j, &t, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(1),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn mapping_is_sticky_for_same_task_across_arrivals() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(0, DeviceType::Cpu, 1.0, PreemptionModel::FullyPreemptive);
        let cpu1 = device(1, DeviceType::Cpu, 2.0, PreemptionModel::FullyPreemptive);
        let t = task(2, vec![DeviceType::Cpu], vec![(DeviceType::Cpu, 20_000)]);
        let first = job(10, 2, 60_000, 6);
        let second = job(11, 2, 60_500, 6);

        let first_view = SchedulerView {
            now: 0,
            devices: &[cpu0.clone(), cpu1.clone()],
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let first_actions = sched.on_job_arrival(&first, &t, &first_view);
        assert!(matches!(
            first_actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(10),
                device_id: DeviceId(1)
            }]
        ));

        let busy_view = SchedulerView {
            now: 2_000,
            devices: &[cpu0, cpu1],
            running_jobs: &[
                (DeviceId(0), None),
                (
                    DeviceId(1),
                    Some(RunningJobInfo {
                        job_id: first.id,
                        task_id: first.task_id,
                        priority: first.effective_priority,
                        release_time: first.release_time,
                        absolute_deadline: first.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 2_000,
                    }),
                ),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let second_actions = sched.on_job_arrival(&second, &t, &busy_view);
        assert!(matches!(
            second_actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(11),
                device_id: DeviceId(1)
            }]
        ));
    }

    #[test]
    fn mapping_rebinds_when_previous_device_is_unavailable() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(0, DeviceType::Cpu, 1.0, PreemptionModel::FullyPreemptive);
        let cpu1 = device(1, DeviceType::Cpu, 2.0, PreemptionModel::FullyPreemptive);
        let t = task(3, vec![DeviceType::Cpu], vec![(DeviceType::Cpu, 20_000)]);
        let first = job(20, 3, 70_000, 7);
        let second = job(21, 3, 70_000, 7);

        let full_view = SchedulerView {
            now: 0,
            devices: &[cpu0.clone(), cpu1],
            running_jobs: &[(DeviceId(0), None), (DeviceId(1), None)],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&first, &t, &full_view);

        let reduced_view = SchedulerView {
            now: 5_000,
            devices: std::slice::from_ref(&cpu0),
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let actions = sched.on_job_arrival(&second, &t, &reduced_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(21),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn arrival_preempts_on_fully_preemptive_device_for_higher_ranked_job() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(0, DeviceType::Cpu, 1.0, PreemptionModel::FullyPreemptive);
        let t = task(4, vec![DeviceType::Cpu], vec![(DeviceType::Cpu, 10_000)]);
        let incoming = job(31, 4, 20_000, 1);
        let running = job(30, 4, 80_000, 10);

        let view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&cpu0),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: running.effective_priority,
                    release_time: running.release_time,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &t, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(30),
                by: JobId(31),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn arrival_on_non_preemptive_device_enqueues_even_for_higher_ranked_job() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(
            0,
            DeviceType::Cpu,
            1.0,
            PreemptionModel::NonPreemptive {
                reconfig_time_ns: 500,
            },
        );
        let t = task(5, vec![DeviceType::Cpu], vec![(DeviceType::Cpu, 10_000)]);
        let incoming = job(41, 5, 20_000, 1);
        let running = job(40, 5, 80_000, 10);

        let view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&cpu0),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: running.effective_priority,
                    release_time: running.release_time,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };

        let actions = sched.on_job_arrival(&incoming, &t, &view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(41),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn completion_and_idle_dispatch_use_same_rank_ordering() {
        let mut sched = MatchScheduler::default();
        let cpu0 = device(0, DeviceType::Cpu, 1.0, PreemptionModel::FullyPreemptive);
        let done_job = job(50, 6, 100_000, 10);
        let q1 = job(51, 6, 50_000, 5);
        let q2 = job(52, 6, 20_000, 3);

        let view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&cpu0),
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(
                DeviceId(0),
                vec![
                    queued(&q1, CriticalityLevel::Lo),
                    queued(&q2, CriticalityLevel::Lo),
                ],
            )],
            criticality_level: CriticalityLevel::Lo,
        };

        let complete_actions = sched.on_job_complete(&done_job, DeviceId(0), &view);
        assert!(matches!(
            complete_actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(52),
                device_id: DeviceId(0)
            }]
        ));

        let idle_actions = sched.on_device_idle(DeviceId(0), &view);
        assert!(matches!(
            idle_actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(52),
                device_id: DeviceId(0)
            }]
        ));
    }
}
