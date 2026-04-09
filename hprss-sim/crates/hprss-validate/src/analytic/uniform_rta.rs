use crate::analytic::rta::{FpTask, InconclusiveReason, UnschedulableReason};

/// Scope of uniform multiprocessor RTA support.
pub const UNIFORM_RTA_SCOPE: &str = "global fixed-priority, uniform multiprocessor, conservative";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniformTaskStatus {
    Schedulable,
    Unschedulable(UnschedulableReason),
    Inconclusive(InconclusiveReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformTaskResult {
    pub task_index: usize,
    pub response_time_lower_bound: u64,
    pub response_time_upper_bound: u64,
    pub iterations: usize,
    pub status: UniformTaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformRtaReport {
    pub task_results: Vec<UniformTaskResult>,
    pub total_utilization_ppm: u64,
    pub total_capacity_ppm: u64,
}

impl UniformRtaReport {
    pub fn is_schedulable(&self) -> bool {
        self.task_results
            .iter()
            .all(|result| matches!(result.status, UniformTaskStatus::Schedulable))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UniformRtaConfig {
    pub max_iterations: usize,
    pub processor_count: usize,
    pub speed_factors: Vec<f64>,
}

impl Default for UniformRtaConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            processor_count: 1,
            speed_factors: Vec::new(),
        }
    }
}

pub fn analyze_uniform_global_fp(tasks: &[FpTask], config: UniformRtaConfig) -> UniformRtaReport {
    let speeds_ppm = normalized_speeds_ppm(&config);
    let total_capacity_ppm = speeds_ppm
        .iter()
        .fold(0_u128, |sum, speed| sum.saturating_add(*speed as u128))
        .min(u64::MAX as u128) as u64;
    let total_utilization_ppm = utilization_ppm(tasks);
    let max_iterations = config.max_iterations.max(1);

    if total_utilization_ppm > total_capacity_ppm {
        let reason = UnschedulableReason::CapacityExceeded {
            total_utilization_ppm,
            total_capacity_ppm,
        };
        return UniformRtaReport {
            task_results: tasks
                .iter()
                .enumerate()
                .map(|(task_index, _)| UniformTaskResult {
                    task_index,
                    response_time_lower_bound: 0,
                    response_time_upper_bound: 0,
                    iterations: 0,
                    status: UniformTaskStatus::Unschedulable(reason.clone()),
                })
                .collect(),
            total_utilization_ppm,
            total_capacity_ppm,
        };
    }

    let max_speed_ppm = speeds_ppm.iter().copied().max().unwrap_or(0);
    let min_positive_speed_ppm = speeds_ppm.iter().copied().filter(|speed| *speed > 0).min();

    let task_results = tasks
        .iter()
        .enumerate()
        .map(|(task_index, task)| {
            let response_time_lower_bound = service_time_ns(task.wcet, max_speed_ppm);

            if response_time_lower_bound > task.deadline {
                return UniformTaskResult {
                    task_index,
                    response_time_lower_bound,
                    response_time_upper_bound: response_time_lower_bound,
                    iterations: 0,
                    status: UniformTaskStatus::Unschedulable(UnschedulableReason::DeadlineMiss {
                        response_time: response_time_lower_bound,
                        deadline: task.deadline,
                    }),
                };
            }

            let Some(min_speed_ppm) = min_positive_speed_ppm else {
                return UniformTaskResult {
                    task_index,
                    response_time_lower_bound,
                    response_time_upper_bound: response_time_lower_bound,
                    iterations: 0,
                    status: UniformTaskStatus::Inconclusive(
                        InconclusiveReason::NoProgressOnAnyProcessor,
                    ),
                };
            };

            let own_pessimistic = service_time_ns(task.wcet, min_speed_ppm);
            let mut response = own_pessimistic as u128;

            for iteration in 1..=max_iterations {
                let interference = tasks
                    .iter()
                    .enumerate()
                    .filter(|(idx, other)| {
                        other.priority < task.priority
                            || (other.priority == task.priority && *idx < task_index)
                    })
                    .map(|(_, hp)| {
                        if hp.period == 0 {
                            return u128::MAX;
                        }
                        let hp_service = service_time_ns(hp.wcet, min_speed_ppm) as u128;
                        response
                            .div_ceil(hp.period as u128)
                            .saturating_mul(hp_service)
                    })
                    .fold(0_u128, |sum, value| sum.saturating_add(value));
                let next = (own_pessimistic as u128).saturating_add(interference);
                let next_u64 = saturating_u128_to_u64(next);

                if next == response {
                    return UniformTaskResult {
                        task_index,
                        response_time_lower_bound,
                        response_time_upper_bound: next_u64,
                        iterations: iteration,
                        status: UniformTaskStatus::Schedulable,
                    };
                }

                if next > task.deadline as u128 {
                    return UniformTaskResult {
                        task_index,
                        response_time_lower_bound,
                        response_time_upper_bound: next_u64,
                        iterations: iteration,
                        status: UniformTaskStatus::Inconclusive(
                            InconclusiveReason::NeedsDetailedInterferenceModel,
                        ),
                    };
                }

                if iteration == max_iterations {
                    return UniformTaskResult {
                        task_index,
                        response_time_lower_bound,
                        response_time_upper_bound: next_u64,
                        iterations: iteration,
                        status: UniformTaskStatus::Inconclusive(
                            InconclusiveReason::IterationLimitReached {
                                last_estimate: next_u64,
                                max_iterations,
                            },
                        ),
                    };
                }

                response = next;
            }

            unreachable!("loop always returns before exhausting iterations")
        })
        .collect();

    UniformRtaReport {
        task_results,
        total_utilization_ppm,
        total_capacity_ppm,
    }
}

fn normalized_speeds_ppm(config: &UniformRtaConfig) -> Vec<u64> {
    if config.speed_factors.is_empty() {
        return vec![1_000_000; config.processor_count.max(1)];
    }

    config
        .speed_factors
        .iter()
        .map(|speed| {
            if speed.is_finite() && *speed > 0.0 {
                (speed * 1_000_000.0).round() as u64
            } else {
                0
            }
        })
        .collect()
}

fn service_time_ns(wcet_ns: u64, speed_ppm: u64) -> u64 {
    if speed_ppm == 0 {
        return u64::MAX;
    }
    let numerator = (wcet_ns as u128).saturating_mul(1_000_000_u128);
    saturating_u128_to_u64(numerator.div_ceil(speed_ppm as u128))
}

fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

fn utilization_ppm(tasks: &[FpTask]) -> u64 {
    tasks
        .iter()
        .filter(|task| task.period > 0)
        .fold(0_u128, |sum, task| {
            sum.saturating_add(
                (task.wcet as u128).saturating_mul(1_000_000_u128) / task.period as u128,
            )
        })
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_schedulable_tasks() -> Vec<FpTask> {
        vec![
            FpTask {
                period: 10,
                deadline: 10,
                wcet: 2,
                priority: 1,
            },
            FpTask {
                period: 20,
                deadline: 20,
                wcet: 2,
                priority: 2,
            },
        ]
    }

    #[test]
    fn sufficient_test_accepts_simple_two_processor_set() {
        let report = analyze_uniform_global_fp(
            &sample_schedulable_tasks(),
            UniformRtaConfig {
                max_iterations: 32,
                processor_count: 2,
                speed_factors: vec![1.0, 1.0],
            },
        );

        assert!(report.is_schedulable());
        assert!(
            report
                .task_results
                .iter()
                .all(|result| matches!(result.status, UniformTaskStatus::Schedulable))
        );
    }

    #[test]
    fn flags_capacity_exceeded_as_unschedulable() {
        let tasks = vec![
            FpTask {
                period: 10,
                deadline: 10,
                wcet: 8,
                priority: 1,
            },
            FpTask {
                period: 10,
                deadline: 10,
                wcet: 8,
                priority: 2,
            },
        ];

        let report = analyze_uniform_global_fp(
            &tasks,
            UniformRtaConfig {
                max_iterations: 32,
                processor_count: 2,
                speed_factors: vec![0.75, 0.75],
            },
        );

        assert!(!report.is_schedulable());
        assert!(report.task_results.iter().all(|result| matches!(
            result.status,
            UniformTaskStatus::Unschedulable(UnschedulableReason::CapacityExceeded { .. })
        )));
    }

    #[test]
    fn insufficient_bound_produces_inconclusive_status() {
        let tasks = vec![
            FpTask {
                period: 5,
                deadline: 5,
                wcet: 4,
                priority: 1,
            },
            FpTask {
                period: 6,
                deadline: 6,
                wcet: 4,
                priority: 2,
            },
        ];
        let report = analyze_uniform_global_fp(
            &tasks,
            UniformRtaConfig {
                max_iterations: 32,
                processor_count: 2,
                speed_factors: vec![1.0, 1.0],
            },
        );

        assert!(!report.is_schedulable());
        assert!(report.task_results.iter().any(|result| matches!(
            result.status,
            UniformTaskStatus::Inconclusive(InconclusiveReason::NeedsDetailedInterferenceModel)
        )));
    }
}
