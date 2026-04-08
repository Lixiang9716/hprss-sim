use hprss_validate::{run_heft_fp_makespan_repro, selected_heft_repro_workloads};

#[test]
fn heft_reproduces_makespan_advantage_over_fp_on_curated_dags() {
    let workloads = selected_heft_repro_workloads();
    assert!(
        !workloads.is_empty(),
        "curated HEFT repro workloads must not be empty"
    );

    let mut strict_improvements = 0usize;
    for workload in &workloads {
        let report = run_heft_fp_makespan_repro(workload);
        assert_eq!(
            report.fp_makespan, workload.expected_fp_makespan,
            "FP makespan drifted for workload {}",
            workload.name
        );
        assert_eq!(
            report.heft_makespan, workload.expected_heft_makespan,
            "HEFT makespan drifted for workload {}",
            workload.name
        );
        assert!(
            report.heft_makespan <= report.fp_makespan,
            "expected HEFT to be no worse than FP for workload {} (FP={}, HEFT={})",
            workload.name,
            report.fp_makespan,
            report.heft_makespan
        );
        if report.heft_makespan < report.fp_makespan {
            strict_improvements += 1;
        }
        assert!(
            report.heft_speedup >= 1.0,
            "expected speedup >= 1.0 for workload {}",
            workload.name
        );
    }

    assert_eq!(
        strict_improvements,
        workloads.len(),
        "each curated workload should show strict HEFT makespan improvement"
    );
}
