use hprss_validate::{
    FpTask, InconclusiveReason, UniformRtaConfig, UniformTaskStatus, analyze_uniform_global_fp,
};

fn sample_tasks() -> Vec<FpTask> {
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
fn deterministic_known_schedulable_set() {
    let report = analyze_uniform_global_fp(
        &sample_tasks(),
        UniformRtaConfig {
            max_iterations: 32,
            processor_count: 2,
            speed_factors: vec![1.0, 1.0],
        },
    );

    assert!(report.is_schedulable());
}

#[test]
fn response_bounds_improve_with_faster_platform() {
    let tasks = sample_tasks();
    let slow = analyze_uniform_global_fp(
        &tasks,
        UniformRtaConfig {
            max_iterations: 32,
            processor_count: 2,
            speed_factors: vec![1.0, 1.0],
        },
    );
    let fast = analyze_uniform_global_fp(
        &tasks,
        UniformRtaConfig {
            max_iterations: 32,
            processor_count: 2,
            speed_factors: vec![2.0, 2.0],
        },
    );

    assert_eq!(slow.task_results.len(), fast.task_results.len());
    for (slow_result, fast_result) in slow.task_results.iter().zip(fast.task_results.iter()) {
        assert!(fast_result.response_time_lower_bound <= slow_result.response_time_lower_bound);
        assert!(fast_result.response_time_upper_bound <= slow_result.response_time_upper_bound);
    }
}

#[test]
fn deterministic_inconclusive_case_is_reported() {
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
