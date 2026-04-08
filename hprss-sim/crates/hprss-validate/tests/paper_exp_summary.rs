use hprss_validate::run_paper_experiment_summary;

#[test]
fn paper_summary_artifact_is_deterministic_and_compact() {
    let first = run_paper_experiment_summary();
    let second = run_paper_experiment_summary();
    assert_eq!(first, second, "paper summary must be reproducible");

    assert!(
        !first.heft_makespan_rows.is_empty(),
        "HEFT baseline summary should not be empty"
    );
    assert!(
        !first.shape_schedulability_curve.is_empty(),
        "SHAPE-style baseline summary should not be empty"
    );
    assert!(
        first
            .heft_makespan_rows
            .iter()
            .all(|row| row.heft_makespan <= row.fp_makespan)
    );

    let csv = first.to_csv();
    let json = first
        .to_json_pretty()
        .expect("paper summary should serialize to JSON");
    let table = first.to_table();

    assert!(csv.starts_with("section,label,fp_makespan_ns"));
    assert_eq!(
        csv.lines().count(),
        1 + first.heft_makespan_rows.len() + first.shape_schedulability_curve.len()
    );
    assert!(json.contains("\"heft_makespan_rows\""));
    assert!(json.contains("\"shape_schedulability_curve\""));
    assert!(table.contains("HEFT makespan baseline"));
    assert!(table.contains("SHAPE-style schedulability baseline"));
}
