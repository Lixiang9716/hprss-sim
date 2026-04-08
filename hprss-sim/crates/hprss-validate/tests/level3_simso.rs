use std::path::PathBuf;
use std::process::Command;

use hprss_validate::{
    CpuOnlySchedulerConfig, SimsoAdapterConfig, default_simso_adapter_runner,
    normalize_simso_output, run_level3_simso_differential, selected_cpu_only_workloads,
};

fn fixture_runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simso_adapter_fixture.py")
}

#[test]
fn normalization_maps_legacy_fields() {
    let normalized = normalize_simso_output(
        r#"{"scheduler":"fp","misses":3,"completions":19,"miss_ratio":0.15789473684210525}"#,
    )
    .expect("legacy key mapping should normalize");

    assert_eq!(normalized.deadline_misses, 3);
    assert_eq!(normalized.completion_count, 19);
    assert_eq!(normalized.scheduler.as_deref(), Some("fp"));
}

#[test]
fn adapter_invocation_contract_and_mapping_are_wired() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");
    let config = SimsoAdapterConfig::for_runner(fixture_runner())
        .with_fixture_mode("legacy_sentinel")
        .with_tolerance(1e-12);

    let report =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect("fixture adapter should execute");

    assert_eq!(report.simso.deadline_misses, 7);
    assert_eq!(report.simso.completion_count, 777);
    assert_eq!(report.simso.scheduler.as_deref(), Some("fp"));
    assert!(!report.outputs_match);
}

#[test]
fn comparison_matches_when_adapter_output_aligns() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .find(|w| w.name == "single-task-control")
        .expect("single-task-control fixture must exist");
    let config = SimsoAdapterConfig::for_runner(fixture_runner())
        .with_fixture_mode("single_task_match")
        .with_tolerance(1e-12);

    let report =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect("fixture adapter should execute");

    assert!(
        report.outputs_match,
        "expected aligned deterministic summaries"
    );
    assert_eq!(report.hprss.deadline_misses, report.simso.deadline_misses);
    assert_eq!(report.hprss.completion_count, report.simso.completion_count);
}

#[test]
fn real_simso_adapter_smoke_when_available() {
    let simso_available = Command::new("python3")
        .args(["-c", "import simso"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !simso_available {
        eprintln!("skipping real SimSo smoke test: python package simso unavailable");
        return;
    }

    let workload = selected_cpu_only_workloads()
        .into_iter()
        .find(|w| w.name == "single-task-control")
        .expect("single-task-control fixture must exist");
    let config =
        SimsoAdapterConfig::for_runner(default_simso_adapter_runner()).with_tolerance(1e-12);

    let report =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect("real SimSo adapter path should execute");

    assert_eq!(report.simso.scheduler.as_deref(), Some("fp"));
    assert_eq!(report.simso.deadline_misses, 0);
    assert_eq!(report.simso.completion_count, 10);
    assert!(report.outputs_match);
}
