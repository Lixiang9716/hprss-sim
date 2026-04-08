#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpTask {
    pub period: u64,
    pub deadline: u64,
    pub wcet: u64,
    pub priority: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtaConfig {
    pub max_iterations: usize,
}

impl Default for RtaConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnschedulableReason {
    DeadlineMiss {
        response_time: u64,
        deadline: u64,
    },
    IterationLimit {
        last_estimate: u64,
        max_iterations: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskSchedulability {
    Schedulable,
    Unschedulable(UnschedulableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRtaResult {
    pub task_index: usize,
    pub response_time: u64,
    pub iterations: usize,
    pub schedulability: TaskSchedulability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtaReport {
    pub task_results: Vec<TaskRtaResult>,
}

impl RtaReport {
    pub fn is_schedulable(&self) -> bool {
        self.task_results
            .iter()
            .all(|r| matches!(r.schedulability, TaskSchedulability::Schedulable))
    }
}

pub fn analyze_uniprocessor_fp(tasks: &[FpTask], config: RtaConfig) -> RtaReport {
    let max_iterations = config.max_iterations.max(1);
    let task_results = tasks
        .iter()
        .enumerate()
        .map(|(task_index, task)| analyze_task(tasks, task_index, task, max_iterations))
        .collect();

    RtaReport { task_results }
}

fn analyze_task(
    tasks: &[FpTask],
    task_index: usize,
    task: &FpTask,
    max_iterations: usize,
) -> TaskRtaResult {
    let mut response = task.wcet as u128;

    if response > task.deadline as u128 {
        return TaskRtaResult {
            task_index,
            response_time: saturating_u128_to_u64(response),
            iterations: 0,
            schedulability: TaskSchedulability::Unschedulable(UnschedulableReason::DeadlineMiss {
                response_time: saturating_u128_to_u64(response),
                deadline: task.deadline,
            }),
        };
    }

    for iteration in 1..=max_iterations {
        let next = task.wcet as u128
            + tasks
                .iter()
                .enumerate()
                .filter(|(idx, other)| {
                    other.priority < task.priority
                        || (other.priority == task.priority && *idx < task_index)
                })
                .map(|(_, hp)| {
                    if hp.period == 0 {
                        u128::MAX
                    } else {
                        response.div_ceil(hp.period as u128) * hp.wcet as u128
                    }
                })
                .sum::<u128>();

        let next_u64 = saturating_u128_to_u64(next);
        if next == response {
            return TaskRtaResult {
                task_index,
                response_time: next_u64,
                iterations: iteration,
                schedulability: TaskSchedulability::Schedulable,
            };
        }

        if next > task.deadline as u128 {
            return TaskRtaResult {
                task_index,
                response_time: next_u64,
                iterations: iteration,
                schedulability: TaskSchedulability::Unschedulable(
                    UnschedulableReason::DeadlineMiss {
                        response_time: next_u64,
                        deadline: task.deadline,
                    },
                ),
            };
        }

        if iteration == max_iterations {
            return TaskRtaResult {
                task_index,
                response_time: next_u64,
                iterations: iteration,
                schedulability: TaskSchedulability::Unschedulable(
                    UnschedulableReason::IterationLimit {
                        last_estimate: next_u64,
                        max_iterations,
                    },
                ),
            };
        }

        response = next;
    }

    unreachable!("loop always returns before exhausting iterations")
}

fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_schedulable_task_set() {
        let tasks = vec![
            FpTask {
                period: 4,
                deadline: 4,
                wcet: 1,
                priority: 1,
            },
            FpTask {
                period: 5,
                deadline: 5,
                wcet: 1,
                priority: 2,
            },
            FpTask {
                period: 10,
                deadline: 10,
                wcet: 2,
                priority: 3,
            },
        ];

        let report = analyze_uniprocessor_fp(&tasks, RtaConfig::default());
        assert!(report.is_schedulable());
        assert_eq!(report.task_results[0].response_time, 1);
        assert_eq!(report.task_results[1].response_time, 2);
        assert_eq!(report.task_results[2].response_time, 4);
    }

    #[test]
    fn known_unschedulable_task_set() {
        let tasks = vec![
            FpTask {
                period: 5,
                deadline: 5,
                wcet: 2,
                priority: 1,
            },
            FpTask {
                period: 7,
                deadline: 7,
                wcet: 3,
                priority: 2,
            },
            FpTask {
                period: 12,
                deadline: 12,
                wcet: 4,
                priority: 3,
            },
        ];

        let report = analyze_uniprocessor_fp(&tasks, RtaConfig::default());
        assert!(!report.is_schedulable());
        assert_eq!(
            report.task_results[2].schedulability,
            TaskSchedulability::Unschedulable(UnschedulableReason::DeadlineMiss {
                response_time: 14,
                deadline: 12,
            })
        );
    }

    #[test]
    fn iteration_limit_reports_reason() {
        let tasks = vec![
            FpTask {
                period: 3,
                deadline: 100,
                wcet: 1,
                priority: 1,
            },
            FpTask {
                period: 50,
                deadline: 100,
                wcet: 2,
                priority: 2,
            },
        ];

        let report = analyze_uniprocessor_fp(&tasks, RtaConfig { max_iterations: 1 });
        assert_eq!(
            report.task_results[1].schedulability,
            TaskSchedulability::Unschedulable(UnschedulableReason::IterationLimit {
                last_estimate: 3,
                max_iterations: 1,
            })
        );
    }
}
