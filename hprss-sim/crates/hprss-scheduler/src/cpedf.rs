use std::collections::{HashMap, HashSet};

use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, JobId, Scheduler, SchedulerView, SubTaskIdx,
    device::PreemptionModel,
    task::{DeviceType, Task},
};

/// Critical-Path-aware EDF baseline scheduler for DAG-heavy workloads.
#[derive(Debug, Default)]
pub struct CpEdfScheduler {
    remaining_work_ns: HashMap<JobId, u64>,
    running_elapsed_ns: HashMap<JobId, u64>,
    dag_stage: HashMap<JobId, SubTaskIdx>,
}

impl CpEdfScheduler {
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
        self.dag_stage
            .retain(|job_id, _| visible_jobs.contains(job_id));
    }

    fn compatible_devices(task: &Task, view: &SchedulerView<'_>) -> Vec<(DeviceId, DeviceType)> {
        task.affinity
            .iter()
            .flat_map(|dt| {
                view.devices
                    .iter()
                    .filter(move |d| d.device_type == *dt)
                    .map(|d| (d.id, d.device_type))
            })
            .collect()
    }

    fn select_target_device(
        task: &Task,
        view: &SchedulerView<'_>,
    ) -> Option<(DeviceId, DeviceType)> {
        let candidates = Self::compatible_devices(task, view);
        candidates.into_iter().min_by(|(a_id, _), (b_id, _)| {
            let a_running = view
                .running_jobs
                .iter()
                .find(|(did, _)| did == a_id)
                .and_then(|(_, info)| info.as_ref())
                .is_some();
            let b_running = view
                .running_jobs
                .iter()
                .find(|(did, _)| did == b_id)
                .and_then(|(_, info)| info.as_ref())
                .is_some();
            let a_queue_len = view
                .ready_queues
                .iter()
                .find(|(did, _)| did == a_id)
                .map_or(0, |(_, q)| q.len());
            let b_queue_len = view
                .ready_queues
                .iter()
                .find(|(did, _)| did == b_id)
                .map_or(0, |(_, q)| q.len());
            (a_running as u8, a_queue_len, a_id.0).cmp(&(b_running as u8, b_queue_len, b_id.0))
        })
    }

    fn estimate_remaining_work_ns(job: &Job, task: &Task, target_device_type: DeviceType) -> u64 {
        job.actual_exec_ns.unwrap_or_else(|| {
            task.exec_times
                .iter()
                .find_map(|(dt, model)| (*dt == target_device_type).then_some(model.wcet()))
                .or_else(|| task.exec_times.iter().map(|(_, model)| model.wcet()).max())
                .unwrap_or(0)
        })
    }

    fn preemption_allowed(
        view: &SchedulerView<'_>,
        device_id: DeviceId,
        at_preemption_point: bool,
    ) -> bool {
        let Some(device) = view.devices.iter().find(|d| d.id == device_id) else {
            return false;
        };
        match device.preemption {
            PreemptionModel::FullyPreemptive => true,
            PreemptionModel::LimitedPreemptive { .. } | PreemptionModel::InterruptLevel { .. } => {
                at_preemption_point
            }
            PreemptionModel::NonPreemptive { .. } => false,
        }
    }

    fn stage_key(&self, job_id: JobId) -> u32 {
        self.dag_stage
            .get(&job_id)
            .map_or(u32::MAX, |stage| u32::MAX - stage.0)
    }

    fn rank_tuple(
        &self,
        now: u64,
        job_id: JobId,
        absolute_deadline: u64,
        release_time: u64,
        remaining_work_ns: u64,
    ) -> (i128, u64, u32, u64, u64) {
        let slack = i128::from(absolute_deadline) - i128::from(now) - i128::from(remaining_work_ns);
        (
            slack,
            absolute_deadline,
            self.stage_key(job_id),
            release_time,
            job_id.0,
        )
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
                    self.rank_tuple(
                        view.now,
                        job.job_id,
                        job.absolute_deadline,
                        job.release_time,
                        self.remaining_work_ns
                            .get(&job.job_id)
                            .copied()
                            .unwrap_or(0),
                    )
                })
            })
    }
}

impl Scheduler for CpEdfScheduler {
    fn name(&self) -> &str {
        "CP-EDF"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        self.observe_view(view);
        let Some((device_id, target_device_type)) = Self::select_target_device(task, view) else {
            return vec![Action::NoOp];
        };

        self.remaining_work_ns.insert(
            job.id,
            Self::estimate_remaining_work_ns(job, task, target_device_type),
        );
        self.running_elapsed_ns.remove(&job.id);
        if let Some(prov) = job.dag_provenance {
            self.dag_stage.insert(job.id, prov.node);
        } else {
            self.dag_stage.remove(&job.id);
        }

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
                let incoming_rank = self.rank_tuple(
                    view.now,
                    job.id,
                    job.absolute_deadline,
                    job.release_time,
                    self.remaining_work_ns.get(&job.id).copied().unwrap_or(0),
                );
                let running_rank = self.rank_tuple(
                    view.now,
                    running_info.job_id,
                    running_info.absolute_deadline,
                    running_info.release_time,
                    self.remaining_work_ns
                        .get(&running_info.job_id)
                        .copied()
                        .unwrap_or(0),
                );
                if incoming_rank < running_rank && Self::preemption_allowed(view, device_id, false)
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
        self.observe_view(view);
        self.remaining_work_ns.remove(&job.id);
        self.running_elapsed_ns.remove(&job.id);
        self.dag_stage.remove(&job.id);
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
                if self.rank_tuple(
                    view.now,
                    waiting.job_id,
                    waiting.absolute_deadline,
                    waiting.release_time,
                    self.remaining_work_ns
                        .get(&waiting.job_id)
                        .copied()
                        .unwrap_or(0),
                ) < self.rank_tuple(
                    view.now,
                    running_info.job_id,
                    running_info.absolute_deadline,
                    running_info.release_time,
                    self.remaining_work_ns
                        .get(&running_info.job_id)
                        .copied()
                        .unwrap_or(0),
                ) && Self::preemption_allowed(view, device_id, true) =>
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

    fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
        self.observe_view(view);
        match self.best_waiting_job(device_id, view) {
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
        DagInstanceId, DagProvenance, JobState, TaskId,
        device::DeviceConfig,
        scheduler::{QueuedJobInfo, RunningJobInfo},
        task::{ArrivalModel, DeviceType, ExecutionTimeModel},
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

    fn gpu_limited_device() -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(1),
            name: "gpu0".to_string(),
            device_group: None,
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive {
                granularity_ns: 1_000,
            },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        }
    }

    fn task_cpu(id: u32) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 1,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 5_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        }
    }

    fn task_gpu(id: u32) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 1,
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 5_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        }
    }

    fn dag_job(id: u64, task_id: u32, deadline: u64, node_idx: u32) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(task_id),
            state: JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: deadline,
            actual_exec_ns: Some(5_000),
            dag_provenance: Some(DagProvenance {
                dag_instance_id: DagInstanceId(0),
                node: SubTaskIdx(node_idx),
            }),
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: 1,
        }
    }

    #[test]
    fn cpedf_prefers_deeper_dag_node_when_deadlines_match() {
        let mut sched = CpEdfScheduler::default();
        let device = cpu_device();
        let running = dag_job(1, 1, 20_000, 0);
        let incoming = dag_job(2, 2, 20_000, 3);
        let task = task_cpu(2);

        let idle_view = SchedulerView {
            now: 0,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(DeviceId(0), None)],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&running, &task_cpu(1), &idle_view);

        let busy_view = SchedulerView {
            now: 1_000,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: running.release_time,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 500,
                }),
            )],
            ready_queues: &[(DeviceId(0), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let actions = sched.on_job_arrival(&incoming, &task, &busy_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(1),
                by: JobId(2),
                device_id: DeviceId(0)
            }]
        ));
    }

    #[test]
    fn cpedf_preempts_limited_device_only_at_preemption_point() {
        let mut sched = CpEdfScheduler::default();
        let device = gpu_limited_device();
        let running = dag_job(10, 10, 40_000, 0);
        let waiting = dag_job(11, 11, 15_000, 4);

        let busy_view = SchedulerView {
            now: 2_000,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 1_000,
                }),
            )],
            ready_queues: &[(DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let arrival_actions = sched.on_job_arrival(&waiting, &task_gpu(11), &busy_view);
        assert!(matches!(
            arrival_actions.as_slice(),
            [Action::Enqueue {
                job_id: JobId(11),
                device_id: DeviceId(1)
            }]
        ));

        let preemption_view = SchedulerView {
            now: 2_000,
            devices: std::slice::from_ref(&device),
            running_jobs: &[(
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: running.id,
                    task_id: running.task_id,
                    priority: 1,
                    release_time: 0,
                    absolute_deadline: running.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 1_000,
                }),
            )],
            ready_queues: &[(
                DeviceId(1),
                vec![QueuedJobInfo {
                    job_id: waiting.id,
                    task_id: waiting.task_id,
                    priority: 1,
                    release_time: waiting.release_time,
                    absolute_deadline: waiting.absolute_deadline,
                    criticality: CriticalityLevel::Lo,
                }],
            )],
            criticality_level: CriticalityLevel::Lo,
        };
        let actions = sched.on_preemption_point(DeviceId(1), &running, &preemption_view);
        assert!(matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(10),
                by: JobId(11),
                device_id: DeviceId(1)
            }]
        ));
    }
}
