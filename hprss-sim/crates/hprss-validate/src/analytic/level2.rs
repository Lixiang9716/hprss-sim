/// Scope of Level 2 validation.
pub const LEVEL2_SCOPE: &str = "tiny exact checks: 2-task CPU, 3-task 2-device, small DAG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TinyCpuTask {
    pub period: u64,
    pub deadline: u64,
    pub wcet: u64,
    pub priority: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyCpuScheduleSummary {
    pub completion_times: Vec<(usize, u64)>,
    pub max_response_times: Vec<u64>,
    pub deadline_misses: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TinyHeteroTask {
    pub period: u64,
    pub deadline: u64,
    pub wcet: u64,
    pub priority: u32,
    pub device_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyHeteroScheduleSummary {
    pub completion_times: Vec<(usize, u64)>,
    pub max_response_times: Vec<u64>,
    pub deadline_misses: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TinyDagNode {
    pub wcet: u64,
    pub device_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TinyDagEdge {
    pub from: usize,
    pub to: usize,
    pub transfer_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyDagReferenceSummary {
    pub start_times: Vec<u64>,
    pub finish_times: Vec<u64>,
    pub makespan: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TinyRuntimeJob {
    task_index: usize,
    release_time: u64,
    absolute_deadline: u64,
    remaining: u64,
    priority: u32,
    sequence: u64,
}

pub fn exact_tiny_fp_uniprocessor(tasks: &[TinyCpuTask], horizon: u64) -> TinyCpuScheduleSummary {
    if tasks.is_empty() || horizon == 0 {
        return TinyCpuScheduleSummary {
            completion_times: vec![],
            max_response_times: vec![0; tasks.len()],
            deadline_misses: 0,
        };
    }

    let mut next_releases = vec![0_u64; tasks.len()];
    let mut sequence = 0_u64;
    let mut ready = Vec::<TinyRuntimeJob>::new();
    let mut completion_times = Vec::<(usize, u64)>::new();
    let mut max_response_times = vec![0_u64; tasks.len()];
    let mut deadline_misses = 0_u64;

    for now in 0..horizon {
        release_jobs_cpu(
            tasks,
            now,
            &mut next_releases,
            &mut ready,
            &mut sequence,
            |task_index, task, now, sequence| TinyRuntimeJob {
                task_index,
                release_time: now,
                absolute_deadline: now.saturating_add(task.deadline),
                remaining: task.wcet,
                priority: task.priority,
                sequence,
            },
        );

        let Some(job_index) = ready
            .iter()
            .enumerate()
            .min_by_key(|(_, job)| (job.priority, job.release_time, job.task_index, job.sequence))
            .map(|(idx, _)| idx)
        else {
            continue;
        };

        if let Some(job) = ready.get_mut(job_index) {
            job.remaining = job.remaining.saturating_sub(1);
        }

        if ready.get(job_index).is_some_and(|job| job.remaining == 0) {
            let completed = ready.swap_remove(job_index);
            let completion_time = now.saturating_add(1);
            completion_times.push((completed.task_index, completion_time));
            let response = completion_time.saturating_sub(completed.release_time);
            max_response_times[completed.task_index] =
                max_response_times[completed.task_index].max(response);
            if completion_time > completed.absolute_deadline {
                deadline_misses = deadline_misses.saturating_add(1);
            }
        }
    }

    TinyCpuScheduleSummary {
        completion_times,
        max_response_times,
        deadline_misses,
    }
}

pub fn exact_tiny_fp_hetero(
    tasks: &[TinyHeteroTask],
    device_count: usize,
    horizon: u64,
) -> TinyHeteroScheduleSummary {
    if tasks.is_empty() || device_count == 0 || horizon == 0 {
        return TinyHeteroScheduleSummary {
            completion_times: vec![],
            max_response_times: vec![0; tasks.len()],
            deadline_misses: 0,
        };
    }

    let mut next_releases = vec![0_u64; tasks.len()];
    let mut sequence = 0_u64;
    let mut ready = Vec::<TinyRuntimeJob>::new();
    let mut completion_times = Vec::<(usize, u64)>::new();
    let mut max_response_times = vec![0_u64; tasks.len()];
    let mut deadline_misses = 0_u64;

    for now in 0..horizon {
        release_jobs_hetero(tasks, now, &mut next_releases, &mut ready, &mut sequence);

        let mut selected = Vec::<usize>::new();
        for device in 0..device_count {
            let candidate = ready
                .iter()
                .enumerate()
                .filter(|(_, job)| tasks[job.task_index].device_index == device)
                .min_by_key(|(_, job)| {
                    (job.priority, job.release_time, job.task_index, job.sequence)
                })
                .map(|(idx, _)| idx);
            if let Some(idx) = candidate {
                selected.push(idx);
            }
        }

        selected.sort_unstable();
        selected.dedup();
        for idx in selected.into_iter().rev() {
            if let Some(job) = ready.get_mut(idx) {
                job.remaining = job.remaining.saturating_sub(1);
            }
            if ready.get(idx).is_some_and(|job| job.remaining == 0) {
                let completed = ready.swap_remove(idx);
                let completion_time = now.saturating_add(1);
                completion_times.push((completed.task_index, completion_time));
                let response = completion_time.saturating_sub(completed.release_time);
                max_response_times[completed.task_index] =
                    max_response_times[completed.task_index].max(response);
                if completion_time > completed.absolute_deadline {
                    deadline_misses = deadline_misses.saturating_add(1);
                }
            }
        }
    }

    TinyHeteroScheduleSummary {
        completion_times,
        max_response_times,
        deadline_misses,
    }
}

pub fn exact_tiny_dag_reference(
    nodes: &[TinyDagNode],
    edges: &[TinyDagEdge],
    device_count: usize,
) -> TinyDagReferenceSummary {
    if nodes.is_empty() || device_count == 0 {
        return TinyDagReferenceSummary {
            start_times: vec![],
            finish_times: vec![],
            makespan: 0,
        };
    }

    let mut preds = vec![Vec::<(usize, u64)>::new(); nodes.len()];
    for edge in edges {
        if edge.from < nodes.len() && edge.to < nodes.len() {
            preds[edge.to].push((edge.from, edge.transfer_time));
        }
    }

    let mut device_available = vec![0_u64; device_count];
    let mut start_times = vec![None::<u64>; nodes.len()];
    let mut finish_times = vec![None::<u64>; nodes.len()];
    let mut scheduled = 0usize;

    while scheduled < nodes.len() {
        let mut best = None::<(usize, u64, u64)>;
        for node_idx in 0..nodes.len() {
            if start_times[node_idx].is_some() {
                continue;
            }

            let mut pred_ready = 0_u64;
            let mut all_ready = true;
            for (pred, transfer) in preds[node_idx].iter().copied() {
                let Some(pred_finish) = finish_times[pred] else {
                    all_ready = false;
                    break;
                };
                pred_ready = pred_ready.max(pred_finish.saturating_add(transfer));
            }
            if !all_ready {
                continue;
            }

            let device = nodes[node_idx].device_index;
            if device >= device_available.len() {
                continue;
            }
            let start = pred_ready.max(device_available[device]);
            let finish = start.saturating_add(nodes[node_idx].wcet);

            match best {
                Some((best_idx, best_start, _)) if (start, node_idx) >= (best_start, best_idx) => {}
                _ => best = Some((node_idx, start, finish)),
            }
        }

        let Some((node_idx, start, finish)) = best else {
            panic!("tiny DAG reference requires an acyclic schedulable graph");
        };
        start_times[node_idx] = Some(start);
        finish_times[node_idx] = Some(finish);
        let device = nodes[node_idx].device_index;
        device_available[device] = finish;
        scheduled += 1;
    }

    let start_times: Vec<u64> = start_times.into_iter().map(|t| t.unwrap_or(0)).collect();
    let finish_times: Vec<u64> = finish_times.into_iter().map(|t| t.unwrap_or(0)).collect();
    let makespan = finish_times.iter().copied().max().unwrap_or(0);

    TinyDagReferenceSummary {
        start_times,
        finish_times,
        makespan,
    }
}

fn release_jobs_cpu<TTask, TJob, F>(
    tasks: &[TTask],
    now: u64,
    next_releases: &mut [u64],
    ready: &mut Vec<TJob>,
    sequence: &mut u64,
    mut build_job: F,
) where
    TTask: CpuLikeTask,
    F: FnMut(usize, &TTask, u64, u64) -> TJob,
{
    for (task_index, task) in tasks.iter().enumerate() {
        if next_releases[task_index] != now {
            continue;
        }
        ready.push(build_job(task_index, task, now, *sequence));
        *sequence = sequence.saturating_add(1);
        next_releases[task_index] = if task.period() == 0 {
            u64::MAX
        } else {
            now.saturating_add(task.period())
        };
    }
}

fn release_jobs_hetero(
    tasks: &[TinyHeteroTask],
    now: u64,
    next_releases: &mut [u64],
    ready: &mut Vec<TinyRuntimeJob>,
    sequence: &mut u64,
) {
    release_jobs_cpu(
        tasks,
        now,
        next_releases,
        ready,
        sequence,
        |task_index, task, now, sequence| TinyRuntimeJob {
            task_index,
            release_time: now,
            absolute_deadline: now.saturating_add(task.deadline),
            remaining: task.wcet,
            priority: task.priority,
            sequence,
        },
    );
}

trait CpuLikeTask {
    fn period(&self) -> u64;
}

impl CpuLikeTask for TinyCpuTask {
    fn period(&self) -> u64 {
        self.period
    }
}

impl CpuLikeTask for TinyHeteroTask {
    fn period(&self) -> u64 {
        self.period
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CpuTask, fp_tasks_with_priorities, hyperperiod, rm_priority_assignment, simulate_fp,
    };
    use hprss_engine::engine::{SimConfig, SimEngine};
    use hprss_scheduler::FixedPriorityScheduler;
    use hprss_types::{
        BusArbitration, CriticalityLevel, DeviceId, JobId, TaskId,
        device::{DeviceConfig, InterconnectConfig, PreemptionModel, SharedBusConfig},
        task::{ArrivalModel, DagTask, DeviceType, ExecutionTimeModel, SubTask, Task},
    };
    use std::collections::HashMap;

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

    fn gpu_device(id: u32) -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(id),
            name: format!("gpu-{id}"),
            device_group: None,
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive {
                granularity_ns: 10_000,
            },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        }
    }

    #[test]
    fn exact_two_tasks_one_cpu_matches_level1_simulation() {
        let tasks = vec![
            CpuTask {
                period: 4,
                deadline: 4,
                wcet: 1,
            },
            CpuTask {
                period: 6,
                deadline: 6,
                wcet: 2,
            },
        ];
        let priorities = rm_priority_assignment(&tasks);
        let fp_tasks = fp_tasks_with_priorities(&tasks, &priorities);
        let horizon = hyperperiod(&tasks);
        let sim = simulate_fp(&fp_tasks, horizon);
        let reference = exact_tiny_fp_uniprocessor(
            &[
                TinyCpuTask {
                    period: 4,
                    deadline: 4,
                    wcet: 1,
                    priority: 1,
                },
                TinyCpuTask {
                    period: 6,
                    deadline: 6,
                    wcet: 2,
                    priority: 2,
                },
            ],
            horizon,
        );

        assert_eq!(reference.deadline_misses, sim.deadline_misses);
        assert_eq!(reference.max_response_times, sim.max_response_times);
        assert_eq!(reference.max_response_times, vec![1, 3]);
    }

    #[test]
    fn exact_three_tasks_two_devices_matches_engine() {
        let mut engine = SimEngine::new(
            SimConfig {
                duration_ns: 100,
                seed: 11,
            },
            vec![cpu_device(0), gpu_device(1)],
            vec![],
            vec![],
        );
        engine.register_tasks(vec![
            Task {
                id: TaskId(0),
                name: "cpu-hi".to_string(),
                priority: 1,
                arrival: ArrivalModel::Periodic { period: 100 },
                deadline: 50,
                criticality: CriticalityLevel::Lo,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 4 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_size: 0,
            },
            Task {
                id: TaskId(1),
                name: "cpu-lo".to_string(),
                priority: 2,
                arrival: ArrivalModel::Periodic { period: 100 },
                deadline: 50,
                criticality: CriticalityLevel::Lo,
                exec_times: vec![(
                    DeviceType::Cpu,
                    ExecutionTimeModel::Deterministic { wcet: 6 },
                )],
                affinity: vec![DeviceType::Cpu],
                data_size: 0,
            },
            Task {
                id: TaskId(2),
                name: "gpu".to_string(),
                priority: 1,
                arrival: ArrivalModel::Periodic { period: 100 },
                deadline: 50,
                criticality: CriticalityLevel::Lo,
                exec_times: vec![(
                    DeviceType::Gpu,
                    ExecutionTimeModel::Deterministic { wcet: 5 },
                )],
                affinity: vec![DeviceType::Gpu],
                data_size: 0,
            },
        ]);
        engine.schedule_initial_arrivals();

        let mut scheduler = FixedPriorityScheduler;
        engine.run(&mut scheduler);

        let reference = exact_tiny_fp_hetero(
            &[
                TinyHeteroTask {
                    period: 100,
                    deadline: 50,
                    wcet: 4,
                    priority: 1,
                    device_index: 0,
                },
                TinyHeteroTask {
                    period: 100,
                    deadline: 50,
                    wcet: 6,
                    priority: 2,
                    device_index: 0,
                },
                TinyHeteroTask {
                    period: 100,
                    deadline: 50,
                    wcet: 5,
                    priority: 1,
                    device_index: 1,
                },
            ],
            2,
            100,
        );

        let mut sim_completions = vec![0_u64; 3];
        for record in &engine.metrics().completions {
            if (record.task_id.0 as usize) < sim_completions.len() {
                sim_completions[record.task_id.0 as usize] = record.completion_time;
            }
        }

        assert_eq!(engine.metrics().completed_jobs, 3);
        assert_eq!(engine.metrics().deadline_misses, 0);
        assert_eq!(reference.deadline_misses, 0);
        assert_eq!(reference.max_response_times, vec![4, 10, 5]);
        assert_eq!(sim_completions, vec![4, 10, 5]);
    }

    #[test]
    fn exact_small_heterogeneous_dag_matches_engine() {
        let mut engine = SimEngine::new(
            SimConfig {
                duration_ns: 50_000,
                seed: 19,
            },
            vec![cpu_device(0), gpu_device(1)],
            vec![InterconnectConfig {
                from: DeviceId(0),
                to: DeviceId(1),
                latency_ns: 2_000,
                bandwidth_bytes_per_ns: 1.0,
                shared_bus: None,
                arbitration: BusArbitration::Dedicated,
            }],
            vec![SharedBusConfig {
                id: hprss_types::BusId(0),
                name: "unused".to_string(),
                total_bandwidth_bytes_per_ns: 1.0,
                arbitration: BusArbitration::RoundRobin,
            }],
        );

        let dag = DagTask {
            id: TaskId(77),
            name: "tiny-dag".to_string(),
            arrival: ArrivalModel::Aperiodic,
            deadline: 50_000,
            criticality: CriticalityLevel::Lo,
            nodes: vec![
                SubTask {
                    index: 0,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 8_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![],
                },
                SubTask {
                    index: 1,
                    exec_times: vec![(
                        DeviceType::Gpu,
                        ExecutionTimeModel::Deterministic { wcet: 6_000 },
                    )],
                    affinity: vec![DeviceType::Gpu],
                    data_deps: vec![(0, 128)],
                },
            ],
            edges: vec![(0, 1)],
        };
        engine.register_dags(vec![dag]);
        let mut scheduler = FixedPriorityScheduler;
        engine.run(&mut scheduler);

        let reference = exact_tiny_dag_reference(
            &[
                TinyDagNode {
                    wcet: 8_000,
                    device_index: 0,
                },
                TinyDagNode {
                    wcet: 6_000,
                    device_index: 1,
                },
            ],
            &[TinyDagEdge {
                from: 0,
                to: 1,
                transfer_time: 2_128,
            }],
            2,
        );

        let completion_by_job: HashMap<JobId, u64> = engine
            .metrics()
            .completions
            .iter()
            .map(|record| (record.job_id, record.completion_time))
            .collect();
        let mut sim_finish_by_node = vec![0_u64; 2];
        for job_id in [JobId(0), JobId(1)] {
            let job = engine
                .get_job(job_id)
                .expect("expected DAG node job to exist in tiny scenario");
            let node_idx = job
                .dag_provenance
                .expect("tiny DAG jobs must carry provenance")
                .node
                .0 as usize;
            sim_finish_by_node[node_idx] = *completion_by_job
                .get(&job_id)
                .expect("completion record should exist for DAG node");
        }

        assert_eq!(engine.metrics().total_jobs, 2);
        assert_eq!(engine.metrics().deadline_misses, 0);
        assert_eq!(sim_finish_by_node, reference.finish_times);
        assert_eq!(reference.makespan, 16_128);
    }
}
