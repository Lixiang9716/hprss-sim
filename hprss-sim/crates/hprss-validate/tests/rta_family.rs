use hprss_validate::{
    AnalysisAlgorithm, AnalysisConfig, AnalysisOutcome, InconclusiveReason, UnschedulableReason,
    analyze_fp_family, analyze_uniprocessor_fp,
};

fn sample_tasks() -> Vec<hprss_validate::FpTask> {
    vec![
        hprss_validate::FpTask {
            period: 4,
            deadline: 4,
            wcet: 1,
            priority: 1,
        },
        hprss_validate::FpTask {
            period: 5,
            deadline: 5,
            wcet: 1,
            priority: 2,
        },
        hprss_validate::FpTask {
            period: 10,
            deadline: 10,
            wcet: 2,
            priority: 3,
        },
    ]
}

#[test]
fn unified_uniprocessor_matches_legacy_rta() {
    let tasks = sample_tasks();
    let legacy = analyze_uniprocessor_fp(&tasks, Default::default());
    let report = analyze_fp_family(
        AnalysisAlgorithm::UniprocessorFixedPriority,
        &tasks,
        AnalysisConfig::default(),
    );

    assert!(report.is_schedulable());
    assert_eq!(report.task_results.len(), legacy.task_results.len());
    for (new_result, old_result) in report.task_results.iter().zip(legacy.task_results.iter()) {
        assert_eq!(new_result.task_index, old_result.task_index);
        assert_eq!(new_result.response_time, old_result.response_time);
        assert_eq!(new_result.iterations, old_result.iterations);
        assert_eq!(new_result.outcome, AnalysisOutcome::Schedulable);
    }
}

#[test]
fn uniform_global_fp_flags_capacity_exceeded() {
    let tasks = vec![
        hprss_validate::FpTask {
            period: 10,
            deadline: 10,
            wcet: 8,
            priority: 1,
        },
        hprss_validate::FpTask {
            period: 10,
            deadline: 10,
            wcet: 8,
            priority: 2,
        },
    ];

    let report = analyze_fp_family(
        AnalysisAlgorithm::UniformMultiprocessorGlobalFpScaffold,
        &tasks,
        AnalysisConfig {
            max_iterations: 16,
            processor_count: 2,
            speed_factors: vec![0.75, 0.75],
        },
    );

    assert!(!report.is_schedulable());
    assert_eq!(report.total_utilization_ppm, 1_600_000);
    assert_eq!(report.total_capacity_ppm, 1_500_000);
    assert!(report.task_results.iter().all(|r| matches!(
        r.outcome,
        AnalysisOutcome::Unschedulable(UnschedulableReason::CapacityExceeded { .. })
    )));
}

#[test]
fn uniform_global_fp_proves_schedulable_with_capacity_headroom() {
    let tasks = vec![
        hprss_validate::FpTask {
            period: 10,
            deadline: 20,
            wcet: 2,
            priority: 1,
        },
        hprss_validate::FpTask {
            period: 20,
            deadline: 20,
            wcet: 2,
            priority: 2,
        },
    ];

    let report = analyze_fp_family(
        AnalysisAlgorithm::UniformMultiprocessorGlobalFpScaffold,
        &tasks,
        AnalysisConfig {
            max_iterations: 8,
            processor_count: 2,
            speed_factors: vec![1.0, 1.0],
        },
    );

    assert_eq!(report.total_utilization_ppm, 300_000);
    assert_eq!(report.total_capacity_ppm, 2_000_000);
    assert!(report.is_schedulable());
    assert!(
        report
            .task_results
            .iter()
            .all(|r| matches!(r.outcome, AnalysisOutcome::Schedulable))
    );
}

#[test]
fn uniform_global_fp_reports_inconclusive_when_sufficient_test_fails() {
    let tasks = vec![
        hprss_validate::FpTask {
            period: 5,
            deadline: 5,
            wcet: 4,
            priority: 1,
        },
        hprss_validate::FpTask {
            period: 6,
            deadline: 6,
            wcet: 4,
            priority: 2,
        },
    ];

    let report = analyze_fp_family(
        AnalysisAlgorithm::UniformMultiprocessorGlobalFpScaffold,
        &tasks,
        AnalysisConfig {
            max_iterations: 8,
            processor_count: 2,
            speed_factors: vec![1.0, 1.0],
        },
    );

    assert!(!report.is_schedulable());
    assert!(report.task_results.iter().any(|r| {
        matches!(
            r.outcome,
            AnalysisOutcome::Inconclusive(InconclusiveReason::NeedsDetailedInterferenceModel)
        )
    }));
}
