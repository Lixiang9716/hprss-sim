use super::rta::{FpTask, RtaConfig, TaskSchedulability, analyze_uniprocessor_fp};

/// Scope of Level 1 validation.
pub const LEVEL1_SCOPE: &str = "cpu-only, fully-preemptive, uniprocessor";

const EPSILON: f64 = 1e-12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTask {
    pub period: u64,
    pub deadline: u64,
    pub wcet: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level1SimulationSummary {
    pub max_response_times: Vec<u64>,
    pub deadline_misses: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchPolicy {
    FixedPriority,
    Edf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SimJob {
    task_index: usize,
    release_time: u64,
    absolute_deadline: u64,
    remaining: u64,
    static_priority: u32,
    sequence: u64,
}

pub fn total_utilization(tasks: &[CpuTask]) -> f64 {
    tasks
        .iter()
        .filter(|task| task.period > 0)
        .map(|task| task.wcet as f64 / task.period as f64)
        .sum()
}

pub fn liu_layland_rm_bound(tasks: &[CpuTask]) -> bool {
    if tasks.is_empty() {
        return true;
    }

    let n = tasks.len() as f64;
    let bound = n * ((2.0f64).powf(1.0 / n) - 1.0);
    total_utilization(tasks) <= bound + EPSILON
}

pub fn edf_exact_bound(tasks: &[CpuTask]) -> bool {
    total_utilization(tasks) <= 1.0 + EPSILON
}

pub fn rm_priority_assignment(tasks: &[CpuTask]) -> Vec<u32> {
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&idx| (tasks[idx].period, idx));
    rank_to_priorities(tasks.len(), &order)
}

pub fn dm_priority_assignment(tasks: &[CpuTask]) -> Vec<u32> {
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&idx| (tasks[idx].deadline, idx));
    rank_to_priorities(tasks.len(), &order)
}

pub fn fp_tasks_with_priorities(tasks: &[CpuTask], priorities: &[u32]) -> Vec<FpTask> {
    assert_eq!(
        tasks.len(),
        priorities.len(),
        "tasks and priorities must have equal length"
    );

    tasks
        .iter()
        .zip(priorities.iter().copied())
        .map(|(task, priority)| FpTask {
            period: task.period,
            deadline: task.deadline,
            wcet: task.wcet,
            priority,
        })
        .collect()
}

pub fn simulate_fp(tasks: &[FpTask], horizon: u64) -> Level1SimulationSummary {
    let cpu_tasks: Vec<CpuTask> = tasks
        .iter()
        .map(|task| CpuTask {
            period: task.period,
            deadline: task.deadline,
            wcet: task.wcet,
        })
        .collect();
    let priorities: Vec<u32> = tasks.iter().map(|task| task.priority).collect();
    simulate(
        &cpu_tasks,
        &priorities,
        DispatchPolicy::FixedPriority,
        horizon,
    )
}

pub fn simulate_edf(tasks: &[CpuTask], horizon: u64) -> Level1SimulationSummary {
    let priorities = vec![0; tasks.len()];
    simulate(tasks, &priorities, DispatchPolicy::Edf, horizon)
}

pub fn hyperperiod(tasks: &[CpuTask]) -> u64 {
    tasks
        .iter()
        .map(|task| task.period)
        .filter(|period| *period > 0)
        .fold(1_u64, lcm)
}

pub fn audsley_opa(tasks: &[CpuTask], config: RtaConfig) -> Option<Vec<u32>> {
    let n = tasks.len();
    if n == 0 {
        return Some(vec![]);
    }

    let mut assigned = vec![None; n];
    let mut unassigned: Vec<usize> = (0..n).collect();

    for priority_level in (1..=n as u32).rev() {
        let mut selected = None;

        for candidate in unassigned.iter().copied() {
            let priorities =
                build_candidate_assignment(candidate, priority_level, &assigned, &unassigned, n);
            let report =
                analyze_uniprocessor_fp(&fp_tasks_with_priorities(tasks, &priorities), config);
            if matches!(
                report.task_results[candidate].schedulability,
                TaskSchedulability::Schedulable
            ) {
                selected = Some((candidate, priorities[candidate]));
                break;
            }
        }

        let (candidate, priority) = selected?;
        assigned[candidate] = Some(priority);
        unassigned.retain(|idx| *idx != candidate);
    }

    assigned.into_iter().collect()
}

pub fn non_preemptive_exact_theory_supported() -> bool {
    false
}

fn simulate(
    tasks: &[CpuTask],
    static_priorities: &[u32],
    policy: DispatchPolicy,
    horizon: u64,
) -> Level1SimulationSummary {
    if tasks.is_empty() || horizon == 0 {
        return Level1SimulationSummary {
            max_response_times: vec![0; tasks.len()],
            deadline_misses: 0,
        };
    }

    assert_eq!(
        tasks.len(),
        static_priorities.len(),
        "tasks and priorities must have equal length"
    );

    let mut now = 0_u64;
    let mut sequence = 0_u64;
    let mut ready = Vec::<SimJob>::new();
    let mut running = None::<SimJob>;
    let mut next_releases = vec![0_u64; tasks.len()];
    let mut max_response_times = vec![0_u64; tasks.len()];
    let mut deadline_misses = 0_u64;

    while now < horizon {
        for (task_index, task) in tasks.iter().enumerate() {
            if next_releases[task_index] == now {
                ready.push(SimJob {
                    task_index,
                    release_time: now,
                    absolute_deadline: now.saturating_add(task.deadline),
                    remaining: task.wcet,
                    static_priority: static_priorities[task_index],
                    sequence,
                });
                sequence = sequence.saturating_add(1);
                next_releases[task_index] = if task.period == 0 {
                    u64::MAX
                } else {
                    now.saturating_add(task.period)
                };
            }
        }

        maybe_context_switch(&mut running, &mut ready, policy);

        if running.is_none() {
            let next_release = next_releases.iter().copied().min().unwrap_or(u64::MAX);
            if next_release >= horizon {
                break;
            }
            now = next_release;
            continue;
        }

        let completion_time = running
            .as_ref()
            .map(|job| now.saturating_add(job.remaining))
            .unwrap_or(now);
        let next_release = next_releases.iter().copied().min().unwrap_or(u64::MAX);
        let next_event_time = completion_time.min(next_release).min(horizon);
        let run_for = next_event_time.saturating_sub(now);

        if let Some(job) = running.as_mut() {
            job.remaining = job.remaining.saturating_sub(run_for);
        }
        now = next_event_time;

        if running.as_ref().is_some_and(|job| job.remaining == 0)
            && let Some(completed) = running.take()
        {
            let response = now.saturating_sub(completed.release_time);
            max_response_times[completed.task_index] =
                max_response_times[completed.task_index].max(response);
            if response > tasks[completed.task_index].deadline {
                deadline_misses = deadline_misses.saturating_add(1);
            }
        }
    }

    Level1SimulationSummary {
        max_response_times,
        deadline_misses,
    }
}

fn maybe_context_switch(
    running: &mut Option<SimJob>,
    ready: &mut Vec<SimJob>,
    policy: DispatchPolicy,
) {
    let Some(candidate_index) = best_job_index(ready, policy) else {
        return;
    };

    let candidate = ready[candidate_index];
    let should_switch = running
        .as_ref()
        .is_none_or(|current| job_key(&candidate, policy) < job_key(current, policy));

    if should_switch {
        let next = ready.swap_remove(candidate_index);
        if let Some(current) = running.replace(next) {
            ready.push(current);
        }
    }
}

fn best_job_index(ready: &[SimJob], policy: DispatchPolicy) -> Option<usize> {
    ready
        .iter()
        .enumerate()
        .min_by_key(|(_, job)| job_key(job, policy))
        .map(|(idx, _)| idx)
}

fn job_key(job: &SimJob, policy: DispatchPolicy) -> (u64, u64, usize, u64) {
    match policy {
        DispatchPolicy::FixedPriority => (
            job.static_priority as u64,
            job.release_time,
            job.task_index,
            job.sequence,
        ),
        DispatchPolicy::Edf => (
            job.absolute_deadline,
            job.release_time,
            job.task_index,
            job.sequence,
        ),
    }
}

fn rank_to_priorities(task_count: usize, order: &[usize]) -> Vec<u32> {
    let mut priorities = vec![0_u32; task_count];
    for (rank, &task_index) in order.iter().enumerate() {
        priorities[task_index] = (rank as u32) + 1;
    }
    priorities
}

fn build_candidate_assignment(
    candidate: usize,
    candidate_priority: u32,
    assigned: &[Option<u32>],
    unassigned: &[usize],
    task_count: usize,
) -> Vec<u32> {
    let mut priorities = vec![0_u32; task_count];
    for (idx, priority) in assigned.iter().copied().enumerate() {
        if let Some(priority) = priority {
            priorities[idx] = priority;
        }
    }
    priorities[candidate] = candidate_priority;

    let mut next_priority = 1_u32;
    for idx in unassigned.iter().copied().filter(|idx| *idx != candidate) {
        while priorities.contains(&next_priority) || next_priority == candidate_priority {
            next_priority = next_priority.saturating_add(1);
        }
        priorities[idx] = next_priority;
    }

    priorities
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

fn lcm(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        0
    } else {
        (a / gcd(a, b)).saturating_mul(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liu_layland_rm_bound() {
        let tasks = vec![
            CpuTask {
                period: 3,
                deadline: 3,
                wcet: 1,
            },
            CpuTask {
                period: 5,
                deadline: 5,
                wcet: 1,
            },
            CpuTask {
                period: 15,
                deadline: 15,
                wcet: 2,
            },
        ];

        assert!(super::liu_layland_rm_bound(&tasks));

        let priorities = rm_priority_assignment(&tasks);
        let fp_tasks = fp_tasks_with_priorities(&tasks, &priorities);
        let summary = simulate_fp(&fp_tasks, hyperperiod(&tasks));
        assert_eq!(summary.deadline_misses, 0);
    }

    #[test]
    fn edf_exact_bound() {
        let schedulable = vec![
            CpuTask {
                period: 4,
                deadline: 4,
                wcet: 1,
            },
            CpuTask {
                period: 5,
                deadline: 5,
                wcet: 1,
            },
            CpuTask {
                period: 20,
                deadline: 20,
                wcet: 2,
            },
        ];
        assert!(super::edf_exact_bound(&schedulable));
        let summary = simulate_edf(&schedulable, hyperperiod(&schedulable));
        assert_eq!(summary.deadline_misses, 0);

        let unschedulable = vec![
            CpuTask {
                period: 2,
                deadline: 2,
                wcet: 2,
            },
            CpuTask {
                period: 3,
                deadline: 3,
                wcet: 2,
            },
        ];
        assert!(!super::edf_exact_bound(&unschedulable));
    }

    #[test]
    fn joseph_pandya_rta() {
        let tasks = vec![
            CpuTask {
                period: 4,
                deadline: 4,
                wcet: 1,
            },
            CpuTask {
                period: 5,
                deadline: 5,
                wcet: 1,
            },
            CpuTask {
                period: 10,
                deadline: 10,
                wcet: 2,
            },
        ];

        let priorities = rm_priority_assignment(&tasks);
        let fp_tasks = fp_tasks_with_priorities(&tasks, &priorities);
        let rta = analyze_uniprocessor_fp(&fp_tasks, RtaConfig::default());
        assert!(rta.is_schedulable());

        let sim = simulate_fp(&fp_tasks, hyperperiod(&tasks));
        for (sim_response, task_rta) in sim.max_response_times.iter().zip(rta.task_results.iter()) {
            assert!(
                *sim_response <= task_rta.response_time,
                "simulated response {} exceeded RTA bound {}",
                sim_response,
                task_rta.response_time
            );
        }
    }

    #[test]
    fn audsley_opa() {
        let tasks = vec![
            CpuTask {
                period: 3,
                deadline: 2,
                wcet: 1,
            },
            CpuTask {
                period: 4,
                deadline: 3,
                wcet: 1,
            },
            CpuTask {
                period: 5,
                deadline: 2,
                wcet: 1,
            },
        ];

        let priorities = super::audsley_opa(&tasks, RtaConfig::default())
            .expect("expected OPA to find a feasible assignment");
        let report = analyze_uniprocessor_fp(
            &fp_tasks_with_priorities(&tasks, &priorities),
            RtaConfig::default(),
        );
        assert!(report.is_schedulable());
    }

    #[test]
    fn dm_optimality() {
        let tasks = vec![
            CpuTask {
                period: 5,
                deadline: 5,
                wcet: 1,
            },
            CpuTask {
                period: 7,
                deadline: 7,
                wcet: 2,
            },
            CpuTask {
                period: 12,
                deadline: 12,
                wcet: 2,
            },
        ];

        let rm = rm_priority_assignment(&tasks);
        let dm = dm_priority_assignment(&tasks);
        assert_eq!(dm, rm, "DM should match RM for implicit-deadline task sets");

        let rm_report =
            analyze_uniprocessor_fp(&fp_tasks_with_priorities(&tasks, &rm), RtaConfig::default());
        let dm_report =
            analyze_uniprocessor_fp(&fp_tasks_with_priorities(&tasks, &dm), RtaConfig::default());
        assert_eq!(rm_report.task_results, dm_report.task_results);
    }

    #[test]
    fn non_preemptive_rta_scope_helper() {
        assert_eq!(LEVEL1_SCOPE, "cpu-only, fully-preemptive, uniprocessor");
        assert!(
            !non_preemptive_exact_theory_supported(),
            "non-preemptive exact theory is intentionally out-of-scope for this Level 1 suite"
        );
    }
}
