use hprss_validate::{
    SHAPE_BASELINE_UTILIZATION_POINTS, ShapeAnalysisConfig, ShapeAnalysisError, ShapeCurveSample,
    analyze_shape_curve, baseline_shape_fixture,
};

#[test]
fn paper_baseline_fixture_matches_deterministic_seed_counts() {
    let fixture = baseline_shape_fixture();
    let observed: Vec<(f64, u32, u32)> = fixture
        .iter()
        .map(|sample| {
            (
                sample.utilization,
                sample.schedulable_runs,
                sample.total_runs,
            )
        })
        .collect();

    let expected = vec![
        (0.4, 5, 8),
        (0.7, 2, 8),
        (1.0, 0, 8),
        (1.3, 0, 8),
        (1.6, 0, 8),
    ];
    assert_eq!(observed, expected);
}

#[test]
fn paper_baseline_analysis_has_explicit_numeric_alignment() {
    let fixture = baseline_shape_fixture();
    let report = analyze_shape_curve(&fixture, ShapeAnalysisConfig::default())
        .expect("paper baseline fixture should analyze");

    let expected = vec![
        (0.4, 0.625, 0.244_863_216_367, 0.914_766_585_863),
        (0.7, 0.25, 0.031_854_026_25, 0.650_855_794_413),
        (1.0, 0.0, 0.0, 0.369_416_647_553),
        (1.3, 0.0, 0.0, 0.369_416_647_553),
        (1.6, 0.0, 0.0, 0.369_416_647_553),
    ];

    assert!(report.is_trend_consistent());
    assert_eq!(report.points.len(), SHAPE_BASELINE_UTILIZATION_POINTS.len());
    for (point, (expected_u, expected_ratio, expected_lo, expected_hi)) in
        report.points.iter().zip(expected.into_iter())
    {
        assert!((point.utilization - expected_u).abs() <= 1e-12);
        assert!((point.schedulability_ratio - expected_ratio).abs() <= 1e-12);
        assert!((point.lower_confidence_bound - expected_lo).abs() <= 1e-12);
        assert!((point.upper_confidence_bound - expected_hi).abs() <= 1e-12);
    }
}

#[test]
fn paper_alignment_rejects_increasing_trend_without_silent_fallback() {
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
fn paper_alignment_rejects_invalid_counts_without_silent_fallback() {
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
