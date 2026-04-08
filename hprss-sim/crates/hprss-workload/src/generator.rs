//! Task set generation from utilization vectors.

use hprss_types::{
    TaskId,
    device::DeviceConfig,
    task::{ArrivalModel, CriticalityLevel, DeviceType, ExecutionTimeModel, Task},
};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::uunifast::uunifast_discard;

/// Workload generation configuration.
#[derive(Debug, Clone)]
pub struct WorkloadConfig {
    pub num_tasks: usize,
    pub total_utilization: f64,
    /// Period range in milliseconds (min, max).
    pub period_range_ms: (u64, u64),
    pub seed: u64,
}

impl Default for WorkloadConfig {
    fn default() -> Self {
        Self {
            num_tasks: 10,
            total_utilization: 0.6,
            period_range_ms: (10, 1000),
            seed: 42,
        }
    }
}

/// Generate a task set using UUniFast-Discard.
///
/// Tasks are assigned to devices round-robin to exercise all device types.
/// Priority is Rate Monotonic (shorter period = higher priority = lower number).
pub fn generate_taskset(config: &WorkloadConfig, devices: &[DeviceConfig]) -> Vec<Task> {
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let utils = uunifast_discard(config.num_tasks, config.total_utilization, &mut rng);

    let mut tasks: Vec<Task> = utils
        .iter()
        .enumerate()
        .map(|(i, &u)| {
            // Log-uniform period sampling
            let log_min = (config.period_range_ms.0 as f64).ln();
            let log_max = (config.period_range_ms.1 as f64).ln();
            let log_p = rng.gen_range(log_min..=log_max);
            let period_ns = (log_p.exp() * 1_000_000.0) as u64;

            // CPU baseline WCET = utilization × period
            let wcet_cpu = (u * period_ns as f64).round().max(1.0) as u64;

            // Pick primary device: round-robin across available device types
            let primary_device = &devices[i % devices.len()];
            let is_cpu = primary_device.device_type == DeviceType::Cpu;

            // Build exec_times: always include CPU baseline + primary device if different
            let mut exec_times = vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: wcet_cpu },
            )];
            let affinity = if is_cpu {
                vec![DeviceType::Cpu]
            } else {
                let wcet_device = (wcet_cpu as f64 / primary_device.speed_factor)
                    .ceil()
                    .max(1.0) as u64;
                exec_times.push((
                    primary_device.device_type,
                    ExecutionTimeModel::Deterministic { wcet: wcet_device },
                ));
                vec![primary_device.device_type]
            };

            // data_size: accelerator tasks need transfer, CPU tasks don't
            let data_size = if is_cpu {
                0
            } else {
                (wcet_cpu / 1_000).max(64)
            };

            Task {
                id: TaskId(i as u32),
                name: format!("τ{}", i),
                priority: 0, // assigned after sorting
                arrival: ArrivalModel::Periodic { period: period_ns },
                deadline: period_ns, // implicit deadline
                criticality: CriticalityLevel::Lo,
                exec_times,
                affinity,
                data_size,
            }
        })
        .collect();

    // Rate Monotonic priority: sort by period ascending
    tasks.sort_by_key(|t| t.period().unwrap_or(u64::MAX));
    for (i, task) in tasks.iter_mut().enumerate() {
        task.id = TaskId(i as u32);
        task.priority = (i as u32) + 1; // 1 = highest priority
    }

    tasks
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::DeviceId;
    use hprss_types::device::PreemptionModel;

    fn test_devices() -> Vec<DeviceConfig> {
        vec![
            DeviceConfig {
                id: DeviceId(0),
                name: "CPU".into(),
                device_group: None,
                device_type: DeviceType::Cpu,
                cores: 4,
                preemption: PreemptionModel::FullyPreemptive,
                context_switch_ns: 5_000,
                speed_factor: 1.0,
                multicore_policy: None,
                power_watts: None,
            },
            DeviceConfig {
                id: DeviceId(1),
                name: "GPU".into(),
                device_group: None,
                device_type: DeviceType::Gpu,
                cores: 1,
                preemption: PreemptionModel::LimitedPreemptive {
                    granularity_ns: 50_000,
                },
                context_switch_ns: 10_000,
                speed_factor: 5.0,
                multicore_policy: None,
                power_watts: None,
            },
            DeviceConfig {
                id: DeviceId(2),
                name: "DSP".into(),
                device_group: None,
                device_type: DeviceType::Dsp,
                cores: 1,
                preemption: PreemptionModel::InterruptLevel {
                    isr_overhead_ns: 2_000,
                    dma_non_preemptive_ns: 20_000,
                },
                context_switch_ns: 3_000,
                speed_factor: 2.0,
                multicore_policy: None,
                power_watts: None,
            },
            DeviceConfig {
                id: DeviceId(3),
                name: "FPGA".into(),
                device_group: None,
                device_type: DeviceType::Fpga,
                cores: 1,
                preemption: PreemptionModel::NonPreemptive {
                    reconfig_time_ns: 2_000_000,
                },
                context_switch_ns: 100_000,
                speed_factor: 3.0,
                multicore_policy: None,
                power_watts: None,
            },
        ]
    }

    #[test]
    fn test_generate_correct_count() {
        let config = WorkloadConfig {
            num_tasks: 10,
            ..Default::default()
        };
        let tasks = generate_taskset(&config, &test_devices());
        assert_eq!(tasks.len(), 10);
    }

    #[test]
    fn test_all_periodic_with_deadline() {
        let tasks = generate_taskset(&WorkloadConfig::default(), &test_devices());
        for t in &tasks {
            let period = t.period().expect("task should be periodic");
            assert_eq!(t.deadline, period);
        }
    }

    #[test]
    fn test_rate_monotonic_priority() {
        let tasks = generate_taskset(&WorkloadConfig::default(), &test_devices());
        for pair in tasks.windows(2) {
            let p0 = pair[0].period().unwrap();
            let p1 = pair[1].period().unwrap();
            assert!(p0 <= p1, "tasks not sorted by period");
            assert!(
                pair[0].priority < pair[1].priority,
                "priority not ascending"
            );
        }
    }

    #[test]
    fn test_heterogeneous_affinity() {
        let config = WorkloadConfig {
            num_tasks: 12,
            ..Default::default()
        };
        let tasks = generate_taskset(&config, &test_devices());
        let non_cpu: Vec<_> = tasks
            .iter()
            .filter(|t| !t.affinity.contains(&DeviceType::Cpu))
            .collect();
        assert!(
            !non_cpu.is_empty(),
            "should have tasks targeting non-CPU devices"
        );
        // With 4 devices round-robin over 12 tasks, 9 should target accelerators
        assert!(non_cpu.len() >= 3);
    }
}
