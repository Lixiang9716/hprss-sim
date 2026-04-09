use hprss_validate::{
    SHAPE_BASELINE_UTILIZATION_POINTS, ShapeAnalysisConfig, ShapeAnalysisError, ShapeCurveSample,
    analyze_shape_curve, baseline_shape_fixture,
};

#[test]
fn baseline_fixture_is_deterministic_and_trend_consistent() {
    let fixture = baseline_shape_fixture();
    let first = analyze_shape_curve(&fixture, ShapeAnalysisConfig::default())
        .expect("baseline fixture should analyze");
    let second = analyze_shape_curve(&fixture, ShapeAnalysisConfig::default())
        .expect("baseline fixture should analyze reproducibly");

    assert_eq!(first, second);
    assert_eq!(first.points.len(), SHAPE_BASELINE_UTILIZATION_POINTS.len());
    for (point, expected_util) in first
        .points
        .iter()
        .zip(SHAPE_BASELINE_UTILIZATION_POINTS.iter().copied())
    {
        assert!((point.utilization - expected_util).abs() <= 1e-12);
        assert!((0.0..=1.0).contains(&point.schedulability_ratio));
        assert!((0.0..=1.0).contains(&point.lower_confidence_bound));
        assert!((0.0..=1.0).contains(&point.upper_confidence_bound));
        assert!(point.lower_confidence_bound <= point.schedulability_ratio);
        assert!(point.schedulability_ratio <= point.upper_confidence_bound);
    }
}

#[test]
fn rejects_increasing_trend_without_silent_fallback() {
    let samples = vec![
        ShapeCurveSample {
            utilization: 0.4,
            schedulable_runs: 6,
            total_runs: 8,
        },
        ShapeCurveSample {
            utilization: 0.8,
            schedulable_runs: 7,
            total_runs: 8,
        },
    ];

    let err = analyze_shape_curve(&samples, ShapeAnalysisConfig::default())
        .expect_err("increasing trend must return an explicit error");

    assert!(matches!(err, ShapeAnalysisError::IncreasingTrend { .. }));
}

#[test]
fn rejects_invalid_counts_without_silent_fallback() {
    let err = analyze_shape_curve(
        &[ShapeCurveSample {
            utilization: 1.0,
            schedulable_runs: 9,
            total_runs: 8,
        }],
        ShapeAnalysisConfig::default(),
    )
    .expect_err("invalid counts must return an explicit error");

    assert!(matches!(err, ShapeAnalysisError::InvalidRunCount { .. }));
}
