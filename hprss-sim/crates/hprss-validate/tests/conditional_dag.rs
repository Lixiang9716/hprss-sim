use hprss_validate::{
    ConditionAssignment, ConditionLiteral, ConditionalDagAnalysisConfig,
    ConditionalDagAnalysisError, ConditionalDagEdge, ConditionalDagNode, analyze_conditional_dag,
};

fn fixture_nodes() -> Vec<ConditionalDagNode> {
    vec![
        ConditionalDagNode { wcet_ns: 2 },
        ConditionalDagNode { wcet_ns: 3 },
        ConditionalDagNode { wcet_ns: 5 },
        ConditionalDagNode { wcet_ns: 7 },
    ]
}

fn fixture_edges() -> Vec<ConditionalDagEdge> {
    vec![
        ConditionalDagEdge {
            from: 0,
            to: 1,
            transfer_ns: 1,
            condition: Some(ConditionLiteral {
                condition_id: 7,
                expected: true,
            }),
        },
        ConditionalDagEdge {
            from: 0,
            to: 2,
            transfer_ns: 2,
            condition: Some(ConditionLiteral {
                condition_id: 7,
                expected: false,
            }),
        },
        ConditionalDagEdge {
            from: 1,
            to: 3,
            transfer_ns: 1,
            condition: None,
        },
        ConditionalDagEdge {
            from: 2,
            to: 3,
            transfer_ns: 1,
            condition: None,
        },
    ]
}

fn response_for(report: &hprss_validate::ConditionalDagAnalysisReport, value: bool) -> Option<u64> {
    report
        .scenario_reports
        .iter()
        .find(|scenario| {
            scenario.assignment
                == vec![ConditionAssignment {
                    condition_id: 7,
                    value,
                }]
        })
        .map(|scenario| scenario.response_time_ns)
}

#[test]
fn deterministic_fixture_reports_bounds_and_schedulability() {
    let report = analyze_conditional_dag(
        &fixture_nodes(),
        &fixture_edges(),
        ConditionalDagAnalysisConfig { deadline_ns: 17 },
    )
    .expect("fixture should be analyzable");

    assert!(report.schedulable);
    assert_eq!(report.scenario_reports.len(), 2);
    assert_eq!(report.response_time_lower_bound_ns, 14);
    assert_eq!(report.response_time_upper_bound_ns, 17);
    assert_eq!(response_for(&report, true), Some(14));
    assert_eq!(response_for(&report, false), Some(17));
}

#[test]
fn report_is_unschedulable_when_deadline_is_tighter_than_upper_bound() {
    let report = analyze_conditional_dag(
        &fixture_nodes(),
        &fixture_edges(),
        ConditionalDagAnalysisConfig { deadline_ns: 16 },
    )
    .expect("fixture should be analyzable");

    assert!(!report.schedulable);
    assert_eq!(report.response_time_upper_bound_ns, 17);
}

#[test]
fn incomplete_condition_coverage_is_an_explicit_error() {
    let edges = vec![ConditionalDagEdge {
        from: 0,
        to: 1,
        transfer_ns: 0,
        condition: Some(ConditionLiteral {
            condition_id: 42,
            expected: true,
        }),
    }];

    let err = analyze_conditional_dag(
        &[
            ConditionalDagNode { wcet_ns: 1 },
            ConditionalDagNode { wcet_ns: 1 },
        ],
        &edges,
        ConditionalDagAnalysisConfig { deadline_ns: 10 },
    )
    .expect_err("analysis should reject partial binary branch coverage");

    assert_eq!(
        err,
        ConditionalDagAnalysisError::IncompleteConditionCoverage {
            node: 0,
            condition_id: 42,
            has_true: true,
            has_false: false,
        }
    );
}
