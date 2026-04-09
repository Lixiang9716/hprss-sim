use hprss_validate::{
    OPENMP_WCRT_SCOPE, OpenMpWcrtConfig, OpenMpWcrtError, OpenMpWcrtStatus, OpenMpWcrtTask,
    OpenMpWcrtUnschedulableReason, analyze_openmp_wcrt,
};

fn fixture_tasks() -> Vec<OpenMpWcrtTask> {
    vec![
        OpenMpWcrtTask {
            period_ns: 20,
            deadline_ns: 20,
            priority: 1,
            requested_threads: 4,
            serial_work_ns: 2,
            parallel_work_ns: 8,
            runtime_overhead_ns: 1,
            critical_section_ns: 0,
        },
        OpenMpWcrtTask {
            period_ns: 40,
            deadline_ns: 40,
            priority: 2,
            requested_threads: 2,
            serial_work_ns: 3,
            parallel_work_ns: 12,
            runtime_overhead_ns: 1,
            critical_section_ns: 1,
        },
        OpenMpWcrtTask {
            period_ns: 60,
            deadline_ns: 30,
            priority: 3,
            requested_threads: 4,
            serial_work_ns: 4,
            parallel_work_ns: 20,
            runtime_overhead_ns: 2,
            critical_section_ns: 2,
        },
    ]
}

#[test]
fn deterministic_fixture_produces_expected_response_bounds() {
    let report = analyze_openmp_wcrt(
        &fixture_tasks(),
        OpenMpWcrtConfig {
            max_iterations: 16,
            available_threads: 4,
        },
    )
    .expect("fixture should analyze");

    assert_eq!(report.scope, OPENMP_WCRT_SCOPE);
    assert_eq!(report.available_threads, 4);
    assert_eq!(report.task_results.len(), 3);
    assert!(!report.is_schedulable());
    assert!(
        report
            .assumptions
            .region_cost_model
            .contains("ceil(parallel_work / effective_threads)")
    );

    assert_eq!(report.task_results[0].effective_threads, 4);
    assert_eq!(report.task_results[0].isolated_cost_ns, 5);
    assert_eq!(report.task_results[0].response_time_ns, 5);
    assert!(matches!(
        report.task_results[0].status,
        OpenMpWcrtStatus::Schedulable
    ));

    assert_eq!(report.task_results[1].effective_threads, 2);
    assert_eq!(report.task_results[1].isolated_cost_ns, 11);
    assert_eq!(report.task_results[1].response_time_ns, 16);
    assert!(matches!(
        report.task_results[1].status,
        OpenMpWcrtStatus::Schedulable
    ));

    assert_eq!(report.task_results[2].effective_threads, 4);
    assert_eq!(report.task_results[2].isolated_cost_ns, 13);
    assert_eq!(report.task_results[2].response_time_ns, 34);
    assert!(matches!(
        report.task_results[2].status,
        OpenMpWcrtStatus::Unschedulable(OpenMpWcrtUnschedulableReason::DeadlineMiss {
            response_time_ns: 34,
            deadline_ns: 30
        })
    ));
}

#[test]
fn analysis_is_reproducible_for_fixed_fixture_inputs() {
    let tasks = fixture_tasks();
    let config = OpenMpWcrtConfig {
        max_iterations: 16,
        available_threads: 4,
    };

    let first = analyze_openmp_wcrt(&tasks, config).expect("analysis should succeed");
    let second = analyze_openmp_wcrt(&tasks, config).expect("analysis should be deterministic");

    assert_eq!(first, second);
}

#[test]
fn invalid_input_is_reported_without_fallback() {
    let err = analyze_openmp_wcrt(
        &[OpenMpWcrtTask {
            period_ns: 100,
            deadline_ns: 100,
            priority: 1,
            requested_threads: 0,
            serial_work_ns: 4,
            parallel_work_ns: 8,
            runtime_overhead_ns: 1,
            critical_section_ns: 0,
        }],
        OpenMpWcrtConfig {
            max_iterations: 8,
            available_threads: 4,
        },
    )
    .expect_err("invalid requested_threads must be explicit");

    assert!(matches!(
        err,
        OpenMpWcrtError::InvalidRequestedThreads { .. }
    ));
}

#[test]
fn paper_style_vector_matches_fixed_point_numeric_baseline() {
    let tasks = vec![
        OpenMpWcrtTask {
            period_ns: 10,
            deadline_ns: 10,
            priority: 1,
            requested_threads: 4,
            serial_work_ns: 1,
            parallel_work_ns: 4,
            runtime_overhead_ns: 0,
            critical_section_ns: 0,
        },
        OpenMpWcrtTask {
            period_ns: 20,
            deadline_ns: 20,
            priority: 2,
            requested_threads: 2,
            serial_work_ns: 2,
            parallel_work_ns: 6,
            runtime_overhead_ns: 0,
            critical_section_ns: 0,
        },
        OpenMpWcrtTask {
            period_ns: 40,
            deadline_ns: 40,
            priority: 3,
            requested_threads: 1,
            serial_work_ns: 7,
            parallel_work_ns: 0,
            runtime_overhead_ns: 0,
            critical_section_ns: 0,
        },
    ];
    let report = analyze_openmp_wcrt(
        &tasks,
        OpenMpWcrtConfig {
            max_iterations: 16,
            available_threads: 4,
        },
    )
    .expect("paper-style vector should analyze");

    assert_eq!(report.task_results[0].isolated_cost_ns, 2);
    assert_eq!(report.task_results[0].response_time_ns, 2);
    assert_eq!(report.task_results[0].iterations, 1);
    assert_eq!(report.task_results[1].isolated_cost_ns, 5);
    assert_eq!(report.task_results[1].response_time_ns, 7);
    assert_eq!(report.task_results[1].iterations, 2);
    assert_eq!(report.task_results[2].isolated_cost_ns, 7);
    assert_eq!(report.task_results[2].response_time_ns, 16);
    assert_eq!(report.task_results[2].iterations, 3);
    assert!(report.is_schedulable());
}

#[test]
fn equal_priority_tasks_do_not_interfere_in_hp_sum() {
    let tasks = vec![
        OpenMpWcrtTask {
            period_ns: 10,
            deadline_ns: 10,
            priority: 1,
            requested_threads: 1,
            serial_work_ns: 3,
            parallel_work_ns: 0,
            runtime_overhead_ns: 0,
            critical_section_ns: 0,
        },
        OpenMpWcrtTask {
            period_ns: 20,
            deadline_ns: 20,
            priority: 1,
            requested_threads: 1,
            serial_work_ns: 4,
            parallel_work_ns: 0,
            runtime_overhead_ns: 0,
            critical_section_ns: 0,
        },
    ];

    let report = analyze_openmp_wcrt(
        &tasks,
        OpenMpWcrtConfig {
            max_iterations: 8,
            available_threads: 2,
        },
    )
    .expect("same-priority vector should analyze");

    assert_eq!(report.task_results[1].response_time_ns, 4);
    assert_eq!(report.task_results[1].iterations, 1);
    assert!(matches!(
        report.task_results[1].status,
        OpenMpWcrtStatus::Schedulable
    ));
    assert!(!report.assumptions.priority_model.contains("ties broken"));
}
