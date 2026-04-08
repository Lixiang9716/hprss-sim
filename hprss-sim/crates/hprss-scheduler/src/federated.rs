use std::collections::HashMap;

use hprss_types::{
    Action, CriticalityLevel, DagInstanceId, DeviceId, Job, Scheduler, SchedulerView,
    device::{DeviceConfig, PreemptionModel},
    task::{DeviceType, Task},
};

/// Federated baseline scheduler:
/// dedicate one compatible device per (DAG instance, device type), EDF within each device.
#[derive(Debug, Default)]
pub struct FederatedScheduler {
    dedicated_devices: HashMap<(DagInstanceId, DeviceType), DeviceId>,
}

impl FederatedScheduler {
    fn compatible_devices<'a>(task: &Task, view: &'a SchedulerView<'_>) -> Vec<&'a DeviceConfig> {
        task.affinity
            .iter()
            .flat_map(|dt| view.devices.iter().filter(move |d| d.device_type == *dt))
            .collect()
    }

    fn select_least_loaded_device(
        &self,
        candidates: &[&DeviceConfig],
        view: &SchedulerView<'_>,
    ) -> Option<DeviceId> {
        candidates.iter().map(|d| d.id).min_by_key(|device_id| {
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

    fn select_target_device(
        &mut self,
        job: &Job,
        task: &Task,
        view: &SchedulerView<'_>,
    ) -> Option<DeviceId> {
        let candidates = Self::compatible_devices(task, view);
        if candidates.is_empty() {
            return None;
        }

        if let Some(prov) = job.dag_provenance {
            for affinity_type in &task.affinity {
                if let Some(device_id) = self
                    .dedicated_devices
                    .get(&(prov.dag_instance_id, *affinity_type))
                    .copied()
                    .filter(|did| candidates.iter().any(|d| d.id == *did))
                {
                    return Some(device_id);
                }
            }

            let device_id = self.select_least_loaded_device(&candidates, view)?;
            let device_type = view
                .devices
                .iter()
                .find(|d| d.id == device_id)
                .map(|d| d.device_type)
                .unwrap_or(DeviceType::Cpu);
            self.dedicated_devices
                .insert((prov.dag_instance_id, device_type), device_id);
            Some(device_id)
        } else {
            self.select_least_loaded_device(&candidates, view)
        }
    }

    fn next_queued_job<'a>(
        device_id: DeviceId,
        view: &'a SchedulerView<'_>,
    ) -> Option<&'a hprss_types::QueuedJobInfo> {
        view.ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| {
                q.iter()
                    .min_by_key(|job| (job.absolute_deadline, job.release_time, job.job_id.0))
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
}

impl Scheduler for FederatedScheduler {
    fn name(&self) -> &str {
        "Federated"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let Some(device_id) = self.select_target_device(job, task, view) else {
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
                if job.absolute_deadline < running_info.absolute_deadline
                    && Self::preemption_allowed(view, device_id, false)
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
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        match Self::next_queued_job(device_id, view) {
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
        match Self::next_queued_job(device_id, view) {
            Some(waiting)
                if waiting.absolute_deadline < running_job.absolute_deadline
                    && Self::preemption_allowed(view, device_id, true) =>
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
        match Self::next_queued_job(device_id, view) {
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
        DagProvenance, JobId, JobState, SubTaskIdx, TaskId,
        device::DeviceConfig,
        scheduler::RunningJobInfo,
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

    fn cpu_task(id: u32) -> Task {
        Task {
            id: TaskId(id),
            name: format!("t{id}"),
            priority: 1,
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

    fn dag_job(id: u64, dag_instance: u64) -> Job {
        Job {
            id: JobId(id),
            task_id: TaskId(id as u32),
            state: JobState::Ready,
            version: 0,
            release_time: 0,
            absolute_deadline: 50_000,
            actual_exec_ns: Some(10_000),
            dag_provenance: Some(DagProvenance {
                dag_instance_id: DagInstanceId(dag_instance),
                node: SubTaskIdx(id as u32),
            }),
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: 1,
        }
    }

    #[test]
    fn federated_reuses_same_device_for_same_dag_instance() {
        let mut sched = FederatedScheduler::default();
        let cpu0 = cpu_device(0);
        let cpu1 = cpu_device(1);
        let task = cpu_task(1);
        let first = dag_job(1, 9);
        let second = dag_job(2, 9);

        let idle_view = SchedulerView {
            now: 0,
            devices: &[cpu0.clone(), cpu1.clone()],
            running_jobs: &[(cpu0.id, None), (cpu1.id, None)],
            ready_queues: &[(cpu0.id, vec![]), (cpu1.id, vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let first_actions = sched.on_job_arrival(&first, &task, &idle_view);
        let first_device = match first_actions.as_slice() {
            [Action::Dispatch { device_id, .. }] => *device_id,
            _ => panic!("expected first job dispatch"),
        };

        let busy_view = SchedulerView {
            now: 1_000,
            devices: &[cpu0, cpu1],
            running_jobs: &[
                (
                    first_device,
                    Some(RunningJobInfo {
                        job_id: first.id,
                        task_id: first.task_id,
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: first.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 0,
                    }),
                ),
                (DeviceId((first_device.0 + 1) % 2), None),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let second_actions = sched.on_job_arrival(&second, &task, &busy_view);
        assert!(matches!(
            second_actions.as_slice(),
            [Action::Enqueue { device_id, .. }] if *device_id == first_device
        ));
    }

    #[test]
    fn federated_separates_distinct_dag_instances_when_capacity_exists() {
        let mut sched = FederatedScheduler::default();
        let cpu0 = cpu_device(0);
        let cpu1 = cpu_device(1);
        let task = cpu_task(1);
        let first = dag_job(1, 1);
        let second = dag_job(2, 2);

        let first_view = SchedulerView {
            now: 0,
            devices: &[cpu0.clone(), cpu1.clone()],
            running_jobs: &[(cpu0.id, None), (cpu1.id, None)],
            ready_queues: &[(cpu0.id, vec![]), (cpu1.id, vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let _ = sched.on_job_arrival(&first, &task, &first_view);

        let second_view = SchedulerView {
            now: 1_000,
            devices: &[cpu0, cpu1],
            running_jobs: &[
                (
                    DeviceId(0),
                    Some(RunningJobInfo {
                        job_id: first.id,
                        task_id: first.task_id,
                        priority: 1,
                        release_time: 0,
                        absolute_deadline: first.absolute_deadline,
                        criticality: CriticalityLevel::Lo,
                        elapsed_ns: 500,
                    }),
                ),
                (DeviceId(1), None),
            ],
            ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
            criticality_level: CriticalityLevel::Lo,
        };
        let second_actions = sched.on_job_arrival(&second, &task, &second_view);
        assert!(matches!(
            second_actions.as_slice(),
            [Action::Dispatch {
                job_id: JobId(2),
                device_id: DeviceId(1)
            }]
        ));
    }
}
