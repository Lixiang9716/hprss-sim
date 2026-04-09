use std::process::Command;
use std::{path::PathBuf, time::Duration};

use hprss_validate::{
    CpuOnlySchedulerConfig, SimsoAdapterConfig, SimsoAdapterError, SimsoDiagnosticCategory,
    SimsoScenarioDomain, SimsoScenarioModel, SimsoTaskModel, default_simso_adapter_runner,
    normalize_simso_output, run_level3_simso_differential, selected_cpu_only_workloads,
    validate_simso_scenario,
};

fn fixture_runner(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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
    let config = SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_legacy_sentinel.py"))
        .with_tolerance(1e-12);

    let report =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect("fixture adapter should execute");

    assert_eq!(report.simso.deadline_misses, 7);
    assert_eq!(report.simso.completion_count, 777);
    assert_eq!(report.simso.scheduler.as_deref(), Some("fp"));
    assert!(!report.outputs_match);
    assert!(
        report
            .mismatches
            .iter()
            .any(|m| m.field == "deadline_misses")
    );
    assert!(
        report
            .mismatches
            .iter()
            .any(|m| m.field == "completion_count")
    );
    assert!(report.mismatches.iter().any(|m| m.field == "miss_ratio"));
}

#[test]
fn adapter_invocation_supports_extended_schema_contract() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .find(|w| w.name == "single-task-control")
        .expect("single-task-control fixture must exist");
    let config =
        SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_extended_schema_sentinel.py"))
            .with_tolerance(1e-12);

    let report =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect("extended-schema fixture adapter should execute");

    assert!(report.outputs_match);
    assert_eq!(report.simso.deadline_misses, 0);
    assert_eq!(report.simso.completion_count, 10);
    assert_eq!(report.simso.miss_ratio, 0.0);
    assert_eq!(report.simso.scheduler.as_deref(), Some("fp"));
    assert!(report.mismatches.is_empty());
}

#[test]
fn adapter_invocation_enforces_numeric_and_field_alignment() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .find(|w| w.name == "single-task-control")
        .expect("single-task-control fixture must exist");
    let config =
        SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_contract_alignment.py"))
            .with_tolerance(1e-12);

    let report = run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::Edf, &config)
        .expect("alignment fixture adapter should execute");

    assert!(report.outputs_match);
    assert!(report.mismatches.is_empty());
    assert_eq!(report.simso.scheduler.as_deref(), Some("edf"));
    assert_eq!(report.simso.deadline_misses, 0);
    assert_eq!(report.simso.completion_count, 10);
    assert_eq!(report.simso.miss_ratio, 0.0);
}

#[test]
fn comparison_matches_when_adapter_output_aligns() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .find(|w| w.name == "single-task-control")
        .expect("single-task-control fixture must exist");
    let config =
        SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_single_task_match.py"))
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
fn extended_scope_validation_reports_categorized_reasons() {
    let unsupported = [
        (
            SimsoScenarioModel {
                domain: SimsoScenarioDomain::Heterogeneous,
                ..SimsoScenarioModel::strict_cpu_only()
            },
            SimsoDiagnosticCategory::Domain,
            "domain_not_supported",
        ),
        (
            SimsoScenarioModel {
                core_count: 2,
                ..SimsoScenarioModel::strict_cpu_only()
            },
            SimsoDiagnosticCategory::ResourceTopology,
            "core_count_not_supported",
        ),
        (
            SimsoScenarioModel {
                task_model: SimsoTaskModel::Sporadic,
                ..SimsoScenarioModel::strict_cpu_only()
            },
            SimsoDiagnosticCategory::TaskModel,
            "task_model_not_supported",
        ),
        (
            SimsoScenarioModel {
                uses_non_cpu_devices: true,
                ..SimsoScenarioModel::strict_cpu_only()
            },
            SimsoDiagnosticCategory::DeviceModel,
            "non_cpu_device_not_supported",
        ),
        (
            SimsoScenarioModel {
                uses_mixed_criticality: true,
                ..SimsoScenarioModel::strict_cpu_only()
            },
            SimsoDiagnosticCategory::CriticalityModel,
            "mixed_criticality_not_supported",
        ),
    ];

    for (scenario, expected_category, expected_code) in unsupported {
        let err = validate_simso_scenario(&scenario).expect_err("scenario should be unsupported");
        assert!(matches!(
            err,
            SimsoAdapterError::UnsupportedScenario {
                category,
                code,
                ..
            } if category == expected_category && code == expected_code
        ));
    }
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

#[test]
fn adapter_reports_non_zero_exit() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");
    let config = SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_nonzero.py"));
    let err =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect_err("non-zero adapter should fail");
    assert!(matches!(
        err,
        SimsoAdapterError::RunnerFailed { code: 9, .. }
    ));
}

#[test]
fn adapter_reports_malformed_json() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");
    let config = SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_malformed_json.py"));
    let err =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect_err("malformed JSON should fail");
    assert!(matches!(err, SimsoAdapterError::ParseOutput(_)));
}

#[test]
fn adapter_reports_missing_or_invalid_required_fields() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");

    let missing_config =
        SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_missing_field.py"));
    let missing_err = run_level3_simso_differential(
        &workload,
        CpuOnlySchedulerConfig::FixedPriority,
        &missing_config,
    )
    .expect_err("missing required field should fail");
    assert!(matches!(
        missing_err,
        SimsoAdapterError::MissingField {
            field: "deadline_misses"
        }
    ));

    let invalid_config =
        SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_invalid_field.py"));
    let invalid_err = run_level3_simso_differential(
        &workload,
        CpuOnlySchedulerConfig::FixedPriority,
        &invalid_config,
    )
    .expect_err("invalid field type should fail");
    assert!(matches!(
        invalid_err,
        SimsoAdapterError::InvalidField {
            field: "completion_count",
            ..
        }
    ));
}

#[test]
fn adapter_reports_missing_runner_path() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");
    let config = SimsoAdapterConfig::for_runner(fixture_runner("does_not_exist.py"));
    let err =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect_err("missing runner path should fail");
    assert!(matches!(err, SimsoAdapterError::RunnerMissing { .. }));
}

#[test]
fn adapter_reports_timeout() {
    let workload = selected_cpu_only_workloads()
        .into_iter()
        .next()
        .expect("selected workloads should not be empty");
    let config = SimsoAdapterConfig::for_runner(fixture_runner("simso_adapter_sleep.py"))
        .with_timeout(Duration::from_millis(30));
    let err =
        run_level3_simso_differential(&workload, CpuOnlySchedulerConfig::FixedPriority, &config)
            .expect_err("hung adapter should timeout");
    assert!(matches!(err, SimsoAdapterError::RunnerTimeout { .. }));
}
