use hprss_validate::{
    UtilizationVectorConfig, UtilizationVectorTask, UtilizationVectorViolationKind,
    analyze_utilization_vectors,
};

fn assert_vec_close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.iter().zip(expected.iter()) {
        assert!((a - e).abs() <= 1e-12, "expected {e}, got {a}");
    }
}

fn task(name: &str, components: &[f64]) -> UtilizationVectorTask {
    UtilizationVectorTask {
        name: name.to_string(),
        components: components.to_vec(),
    }
}

#[test]
fn feasible_utilization_vectors_pass_paper_inspired_checks() {
    let tasks = vec![
        task("cpu-heavy", &[0.30, 0.20]),
        task("gpu-heavy", &[0.20, 0.30]),
        task("balanced", &[0.10, 0.10]),
    ];

    let report =
        analyze_utilization_vectors(&tasks, &[1.0, 1.0], UtilizationVectorConfig::default());

    assert!(report.passes, "expected feasible vector set to pass");
    assert_eq!(report.dimension_count, 2);
    assert_eq!(report.task_count, 3);
    assert_vec_close(&report.aggregate_utilization, &[0.60, 0.60]);
    assert_vec_close(&report.dominant_shares, &[0.30, 0.30, 0.10]);
    assert!((report.dominant_share_sum - 0.70).abs() <= 1e-12);
    assert!(report.violations.is_empty());
}

#[test]
fn overloaded_dimension_fails_capacity_check() {
    let tasks = vec![task("a", &[0.55, 0.10]), task("b", &[0.50, 0.20])];

    let report = analyze_utilization_vectors(
        &tasks,
        &[1.0, 1.0],
        UtilizationVectorConfig {
            dominant_share_limit: None,
            ..UtilizationVectorConfig::default()
        },
    );

    assert!(!report.passes);
    assert_eq!(report.violations.len(), 1);
    let violation = &report.violations[0];
    assert_eq!(
        violation.kind,
        UtilizationVectorViolationKind::CapacityExceeded
    );
    assert_eq!(violation.dimension, Some(0));
}

#[test]
fn dominant_share_property_can_fail_even_when_each_dimension_is_under_capacity() {
    let tasks = vec![task("a", &[0.60, 0.05]), task("b", &[0.05, 0.60])];

    let report = analyze_utilization_vectors(
        &tasks,
        &[1.0, 1.0],
        UtilizationVectorConfig {
            dominant_share_limit: Some(1.0),
            ..UtilizationVectorConfig::default()
        },
    );

    assert_vec_close(&report.aggregate_utilization, &[0.65, 0.65]);
    assert_vec_close(&report.dominant_shares, &[0.60, 0.60]);
    assert!((report.dominant_share_sum - 1.20).abs() <= 1e-12);
    assert!(!report.passes);
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == UtilizationVectorViolationKind::DominantShareExceeded)
    );
    assert!(
        !report
            .violations
            .iter()
            .any(|v| v.kind == UtilizationVectorViolationKind::CapacityExceeded)
    );
}

#[test]
fn deterministic_fixture_produces_stable_report() {
    let tasks = vec![
        task("tau0", &[0.25, 0.10, 0.05]),
        task("tau1", &[0.15, 0.30, 0.10]),
        task("tau2", &[0.10, 0.15, 0.20]),
    ];

    let first =
        analyze_utilization_vectors(&tasks, &[1.0, 1.0, 1.0], UtilizationVectorConfig::default());
    let second =
        analyze_utilization_vectors(&tasks, &[1.0, 1.0, 1.0], UtilizationVectorConfig::default());

    assert_eq!(first, second, "report should be deterministic");
}
