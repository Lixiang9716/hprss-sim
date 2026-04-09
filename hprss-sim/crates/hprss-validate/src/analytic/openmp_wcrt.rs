use thiserror::Error;

/// Scope marker for OpenMP fixed-priority WCRT analytics.
pub const OPENMP_WCRT_SCOPE: &str =
    "OpenMP region-level fixed-priority WCRT bound with explicit thread-pool assumptions";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMpWcrtModelAssumptions {
    /// A fixed-size thread pool is shared by all OpenMP regions in this analysis.
    pub pool_model: &'static str,
    /// Region cost model used before fixed-point interference.
    pub region_cost_model: &'static str,
    /// Interference model used for higher-priority regions.
    pub interference_model: &'static str,
    /// Priority interpretation.
    pub priority_model: &'static str,
}

impl Default for OpenMpWcrtModelAssumptions {
    fn default() -> Self {
        Self {
            pool_model: "single fixed-size OpenMP team pool; effective threads = min(requested_threads, available_threads)",
            region_cost_model: "C_i = serial_work + ceil(parallel_work / effective_threads) + runtime_overhead + critical_section",
            interference_model: "R_i^{k+1} = C_i + Σ ceil(R_i^k / T_h) * C_h over higher-priority regions",
            priority_model: "lower numeric value means higher priority (ties broken by task index)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMpWcrtTask {
    pub period_ns: u64,
    pub deadline_ns: u64,
    pub priority: u32,
    pub requested_threads: u32,
    pub serial_work_ns: u64,
    pub parallel_work_ns: u64,
    pub runtime_overhead_ns: u64,
    pub critical_section_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenMpWcrtConfig {
    pub max_iterations: usize,
    pub available_threads: u32,
}

impl Default for OpenMpWcrtConfig {
    fn default() -> Self {
        Self {
            max_iterations: 128,
            available_threads: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenMpWcrtStatus {
    Schedulable,
    Unschedulable(OpenMpWcrtUnschedulableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenMpWcrtUnschedulableReason {
    DeadlineMiss {
        response_time_ns: u64,
        deadline_ns: u64,
    },
    IterationLimit {
        last_estimate_ns: u64,
        max_iterations: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMpWcrtTaskResult {
    pub task_index: usize,
    pub effective_threads: u32,
    pub isolated_cost_ns: u64,
    pub response_time_ns: u64,
    pub iterations: usize,
    pub status: OpenMpWcrtStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMpWcrtReport {
    pub scope: &'static str,
    pub assumptions: OpenMpWcrtModelAssumptions,
    pub available_threads: u32,
    pub task_results: Vec<OpenMpWcrtTaskResult>,
}

impl OpenMpWcrtReport {
    pub fn is_schedulable(&self) -> bool {
        self.task_results
            .iter()
            .all(|result| matches!(result.status, OpenMpWcrtStatus::Schedulable))
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OpenMpWcrtError {
    #[error("config available_threads must be > 0")]
    InvalidAvailableThreads,
    #[error("config max_iterations must be > 0")]
    InvalidMaxIterations,
    #[error("task {index} period_ns must be > 0")]
    InvalidPeriod { index: usize },
    #[error("task {index} deadline_ns must be > 0")]
    InvalidDeadline { index: usize },
    #[error("task {index} requested_threads must be > 0")]
    InvalidRequestedThreads { index: usize },
}

pub fn analyze_openmp_wcrt(
    tasks: &[OpenMpWcrtTask],
    config: OpenMpWcrtConfig,
) -> Result<OpenMpWcrtReport, OpenMpWcrtError> {
    if config.available_threads == 0 {
        return Err(OpenMpWcrtError::InvalidAvailableThreads);
    }
    if config.max_iterations == 0 {
        return Err(OpenMpWcrtError::InvalidMaxIterations);
    }

    let mut isolated_costs = Vec::with_capacity(tasks.len());
    let mut effective_threads = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        if task.period_ns == 0 {
            return Err(OpenMpWcrtError::InvalidPeriod { index });
        }
        if task.deadline_ns == 0 {
            return Err(OpenMpWcrtError::InvalidDeadline { index });
        }
        if task.requested_threads == 0 {
            return Err(OpenMpWcrtError::InvalidRequestedThreads { index });
        }

        let task_effective_threads = task.requested_threads.min(config.available_threads);
        let parallel_component = div_ceil_u64(task.parallel_work_ns, task_effective_threads as u64);
        let isolated_cost = task
            .serial_work_ns
            .saturating_add(parallel_component)
            .saturating_add(task.runtime_overhead_ns)
            .saturating_add(task.critical_section_ns);
        effective_threads.push(task_effective_threads);
        isolated_costs.push(isolated_cost);
    }

    let task_results = tasks
        .iter()
        .enumerate()
        .map(|(task_index, task)| {
            analyze_task(
                tasks,
                task_index,
                task,
                &isolated_costs,
                effective_threads[task_index],
                config.max_iterations,
            )
        })
        .collect();

    Ok(OpenMpWcrtReport {
        scope: OPENMP_WCRT_SCOPE,
        assumptions: OpenMpWcrtModelAssumptions::default(),
        available_threads: config.available_threads,
        task_results,
    })
}

fn analyze_task(
    tasks: &[OpenMpWcrtTask],
    task_index: usize,
    task: &OpenMpWcrtTask,
    isolated_costs: &[u64],
    effective_threads: u32,
    max_iterations: usize,
) -> OpenMpWcrtTaskResult {
    let mut response = isolated_costs[task_index] as u128;
    if response > task.deadline_ns as u128 {
        let response_time_ns = saturating_u128_to_u64(response);
        return OpenMpWcrtTaskResult {
            task_index,
            effective_threads,
            isolated_cost_ns: isolated_costs[task_index],
            response_time_ns,
            iterations: 0,
            status: OpenMpWcrtStatus::Unschedulable(OpenMpWcrtUnschedulableReason::DeadlineMiss {
                response_time_ns,
                deadline_ns: task.deadline_ns,
            }),
        };
    }

    for iteration in 1..=max_iterations {
        let interference = tasks
            .iter()
            .enumerate()
            .filter(|(idx, hp)| {
                hp.priority < task.priority || (hp.priority == task.priority && *idx < task_index)
            })
            .map(|(idx, hp)| {
                response
                    .div_ceil(hp.period_ns as u128)
                    .saturating_mul(isolated_costs[idx] as u128)
            })
            .fold(0_u128, |sum, value| sum.saturating_add(value));

        let next = (isolated_costs[task_index] as u128).saturating_add(interference);
        let response_time_ns = saturating_u128_to_u64(next);

        if next == response {
            return OpenMpWcrtTaskResult {
                task_index,
                effective_threads,
                isolated_cost_ns: isolated_costs[task_index],
                response_time_ns,
                iterations: iteration,
                status: OpenMpWcrtStatus::Schedulable,
            };
        }

        if next > task.deadline_ns as u128 {
            return OpenMpWcrtTaskResult {
                task_index,
                effective_threads,
                isolated_cost_ns: isolated_costs[task_index],
                response_time_ns,
                iterations: iteration,
                status: OpenMpWcrtStatus::Unschedulable(
                    OpenMpWcrtUnschedulableReason::DeadlineMiss {
                        response_time_ns,
                        deadline_ns: task.deadline_ns,
                    },
                ),
            };
        }

        if iteration == max_iterations {
            return OpenMpWcrtTaskResult {
                task_index,
                effective_threads,
                isolated_cost_ns: isolated_costs[task_index],
                response_time_ns,
                iterations: iteration,
                status: OpenMpWcrtStatus::Unschedulable(
                    OpenMpWcrtUnschedulableReason::IterationLimit {
                        last_estimate_ns: response_time_ns,
                        max_iterations,
                    },
                ),
            };
        }

        response = next;
    }

    unreachable!("loop always returns before exhausting iterations")
}

fn div_ceil_u64(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    numerator.div_ceil(denominator)
}

fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_requested_threads() {
        let err = analyze_openmp_wcrt(
            &[OpenMpWcrtTask {
                period_ns: 100,
                deadline_ns: 100,
                priority: 1,
                requested_threads: 0,
                serial_work_ns: 10,
                parallel_work_ns: 20,
                runtime_overhead_ns: 2,
                critical_section_ns: 0,
            }],
            OpenMpWcrtConfig {
                max_iterations: 8,
                available_threads: 4,
            },
        )
        .expect_err("zero requested threads must fail");

        assert!(matches!(
            err,
            OpenMpWcrtError::InvalidRequestedThreads { .. }
        ));
    }
}
