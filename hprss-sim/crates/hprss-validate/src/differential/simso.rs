use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{Map, Value};

use super::cpu_only::{
    CpuOnlyRunSummary, CpuOnlySchedulerConfig, CpuOnlyTask, CpuOnlyWorkload,
    run_cpu_only_scheduler, selected_cpu_only_workloads,
};

/// Scope marker for strict Level 3 differential validation through an external SimSo adapter.
pub const LEVEL3_SCOPE: &str = "strict CPU-only SimSo differential validation";
const ADAPTER_CONTRACT: &str = "hprss-simso-v1";
const DEFAULT_MISS_RATIO_TOLERANCE: f64 = 1e-12;
const DEFAULT_ADAPTER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimsoRunSummary {
    pub scheduler: Option<String>,
    pub deadline_misses: u64,
    pub completion_count: u64,
    pub miss_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Level3SimsoDifferentialReport {
    pub scope: &'static str,
    pub workload: String,
    pub scheduler: CpuOnlySchedulerConfig,
    pub hprss: CpuOnlyRunSummary,
    pub simso: SimsoRunSummary,
    pub mismatches: Vec<SimsoSummaryMismatch>,
    pub outputs_match: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimsoMismatchCategory {
    Scheduler,
    DeadlineMisses,
    CompletionCount,
    MissRatio,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimsoSummaryMismatch {
    pub category: SimsoMismatchCategory,
    pub field: &'static str,
    pub expected: String,
    pub observed: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimsoScenarioDomain {
    CpuOnly,
    CpuMultiprocessor,
    Heterogeneous,
    Dag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimsoTaskModel {
    Periodic,
    Sporadic,
    Dag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimsoScenarioModel {
    pub domain: SimsoScenarioDomain,
    pub core_count: u32,
    pub task_model: SimsoTaskModel,
    pub uses_non_cpu_devices: bool,
    pub uses_mixed_criticality: bool,
}

impl SimsoScenarioModel {
    pub fn strict_cpu_only() -> Self {
        Self {
            domain: SimsoScenarioDomain::CpuOnly,
            core_count: 1,
            task_model: SimsoTaskModel::Periodic,
            uses_non_cpu_devices: false,
            uses_mixed_criticality: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SimsoDiagnosticCategory {
    Domain,
    ResourceTopology,
    TaskModel,
    DeviceModel,
    CriticalityModel,
}

#[derive(Debug, Clone)]
pub struct SimsoAdapterConfig {
    pub runner: PathBuf,
    pub python_bin: String,
    pub miss_ratio_tolerance: f64,
    pub adapter_timeout: Duration,
}

impl Default for SimsoAdapterConfig {
    fn default() -> Self {
        Self {
            runner: default_simso_adapter_runner(),
            python_bin: "python3".to_string(),
            miss_ratio_tolerance: DEFAULT_MISS_RATIO_TOLERANCE,
            adapter_timeout: DEFAULT_ADAPTER_TIMEOUT,
        }
    }
}

impl SimsoAdapterConfig {
    pub fn for_runner(runner: impl Into<PathBuf>) -> Self {
        Self {
            runner: runner.into(),
            ..Self::default()
        }
    }

    pub fn with_python_bin(mut self, python_bin: impl Into<String>) -> Self {
        self.python_bin = python_bin.into();
        self
    }

    pub fn with_tolerance(mut self, tolerance: f64) -> Self {
        self.miss_ratio_tolerance = tolerance;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.adapter_timeout = timeout;
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SimsoAdapterError {
    #[error("failed to serialize adapter input: {0}")]
    SerializeInput(serde_json::Error),
    #[error("adapter runner path does not exist: {path}")]
    RunnerMissing { path: PathBuf },
    #[error("failed to execute adapter runner: {0}")]
    RunnerIo(std::io::Error),
    #[error("failed to write adapter input to stdin: {0}")]
    StdinWrite(std::io::Error),
    #[error("adapter runner timed out after {timeout_ms} ms")]
    RunnerTimeout { timeout_ms: u64 },
    #[error("adapter runner exited with code {code}: {stderr}")]
    RunnerFailed { code: i32, stderr: String },
    #[error("adapter returned invalid JSON: {0}")]
    ParseOutput(serde_json::Error),
    #[error("adapter output missing required field `{field}`")]
    MissingField { field: &'static str },
    #[error("adapter output field `{field}` has invalid value: {value}")]
    InvalidField { field: &'static str, value: String },
    #[error("unsupported scenario ({category:?}/{code}): {detail}")]
    UnsupportedScenario {
        category: SimsoDiagnosticCategory,
        code: &'static str,
        detail: String,
    },
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterInput {
    adapter_contract: &'static str,
    strict_mode: bool,
    workload: String,
    horizon_ns: u64,
    scheduler: &'static str,
    scenario: SimsoAdapterScenario,
    algorithm: SimsoAdapterAlgorithm,
    model: SimsoAdapterModel,
    tasks: Vec<SimsoAdapterTask>,
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterScenario {
    domain: SimsoScenarioDomain,
    core_count: u32,
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterAlgorithm {
    requested: &'static str,
    canonical: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterModel {
    time_unit: &'static str,
    task_model: SimsoTaskModel,
    mixed_criticality: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterTask {
    task_id: u32,
    period_ns: u64,
    deadline_ns: u64,
    wcet_ns: u64,
    priority: u32,
}

pub fn default_simso_adapter_runner() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/simso_adapter_runner.py")
        .canonicalize()
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/simso_adapter_runner.py")
        })
}

pub fn run_level3_simso_selected(
    config: &SimsoAdapterConfig,
) -> Result<Vec<Level3SimsoDifferentialReport>, SimsoAdapterError> {
    let mut reports = Vec::new();
    for workload in selected_cpu_only_workloads() {
        for scheduler in [
            CpuOnlySchedulerConfig::FixedPriority,
            CpuOnlySchedulerConfig::Edf,
        ] {
            reports.push(run_level3_simso_differential(&workload, scheduler, config)?);
        }
    }
    Ok(reports)
}

pub fn run_level3_simso_differential(
    workload: &CpuOnlyWorkload,
    scheduler: CpuOnlySchedulerConfig,
    config: &SimsoAdapterConfig,
) -> Result<Level3SimsoDifferentialReport, SimsoAdapterError> {
    let hprss = run_cpu_only_scheduler(workload, scheduler);
    let adapter_input = serde_json::to_string(&build_adapter_input(workload, scheduler)?)
        .map_err(SimsoAdapterError::SerializeInput)?;
    let adapter_output = execute_adapter(config, &adapter_input)?;
    let simso = normalize_simso_output(&adapter_output)?;
    let mismatches = collect_summary_mismatches(
        &hprss,
        &simso,
        scheduler_label(scheduler),
        config.miss_ratio_tolerance,
    );
    let outputs_match = mismatches.is_empty();

    Ok(Level3SimsoDifferentialReport {
        scope: LEVEL3_SCOPE,
        workload: workload.name.to_string(),
        scheduler,
        hprss,
        simso,
        mismatches,
        outputs_match,
    })
}

pub fn normalize_simso_output(output_json: &str) -> Result<SimsoRunSummary, SimsoAdapterError> {
    let parsed: Value =
        serde_json::from_str(output_json).map_err(SimsoAdapterError::ParseOutput)?;
    let obj = parsed.as_object().ok_or(SimsoAdapterError::InvalidField {
        field: "root",
        value: "expected JSON object".to_string(),
    })?;

    let deadline_misses = extract_u64(obj, &["deadline_misses", "misses"], "deadline_misses")?;
    let completion_count = extract_u64(
        obj,
        &["completion_count", "completions"],
        "completion_count",
    )?;
    let miss_ratio = extract_f64(obj, &["miss_ratio"], "miss_ratio")?;
    let scheduler = find_value(obj, &["scheduler", "scheduler_name"])
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(SimsoRunSummary {
        scheduler,
        deadline_misses,
        completion_count,
        miss_ratio,
    })
}

fn build_adapter_input(
    workload: &CpuOnlyWorkload,
    scheduler: CpuOnlySchedulerConfig,
) -> Result<SimsoAdapterInput, SimsoAdapterError> {
    build_adapter_input_with_scenario(workload, scheduler, &SimsoScenarioModel::strict_cpu_only())
}

fn build_adapter_input_with_scenario(
    workload: &CpuOnlyWorkload,
    scheduler: CpuOnlySchedulerConfig,
    scenario: &SimsoScenarioModel,
) -> Result<SimsoAdapterInput, SimsoAdapterError> {
    validate_simso_scenario(scenario)?;
    let scheduler = scheduler_label(scheduler);
    Ok(SimsoAdapterInput {
        adapter_contract: ADAPTER_CONTRACT,
        strict_mode: true,
        workload: workload.name.to_string(),
        horizon_ns: workload.horizon,
        scheduler,
        scenario: SimsoAdapterScenario {
            domain: scenario.domain,
            core_count: scenario.core_count,
        },
        algorithm: SimsoAdapterAlgorithm {
            requested: scheduler,
            canonical: scheduler,
        },
        model: SimsoAdapterModel {
            time_unit: "ns",
            task_model: scenario.task_model,
            mixed_criticality: scenario.uses_mixed_criticality,
        },
        tasks: workload
            .tasks
            .iter()
            .enumerate()
            .map(|(task_id, task)| to_adapter_task(task_id as u32, task))
            .collect(),
    })
}

pub fn validate_simso_scenario(scenario: &SimsoScenarioModel) -> Result<(), SimsoAdapterError> {
    if scenario.domain != SimsoScenarioDomain::CpuOnly {
        return Err(SimsoAdapterError::UnsupportedScenario {
            category: SimsoDiagnosticCategory::Domain,
            code: "domain_not_supported",
            detail: format!(
                "domain {:?} is outside strict CPU-only SimSo differential scope",
                scenario.domain
            ),
        });
    }
    if scenario.core_count != 1 {
        return Err(SimsoAdapterError::UnsupportedScenario {
            category: SimsoDiagnosticCategory::ResourceTopology,
            code: "core_count_not_supported",
            detail: format!(
                "core_count={} is unsupported; strict path requires exactly 1 core",
                scenario.core_count
            ),
        });
    }
    if scenario.task_model != SimsoTaskModel::Periodic {
        return Err(SimsoAdapterError::UnsupportedScenario {
            category: SimsoDiagnosticCategory::TaskModel,
            code: "task_model_not_supported",
            detail: format!(
                "task_model {:?} is unsupported; strict path expects periodic tasks",
                scenario.task_model
            ),
        });
    }
    if scenario.uses_non_cpu_devices {
        return Err(SimsoAdapterError::UnsupportedScenario {
            category: SimsoDiagnosticCategory::DeviceModel,
            code: "non_cpu_device_not_supported",
            detail: "heterogeneous/non-CPU devices are not supported in strict differential mode"
                .to_string(),
        });
    }
    if scenario.uses_mixed_criticality {
        return Err(SimsoAdapterError::UnsupportedScenario {
            category: SimsoDiagnosticCategory::CriticalityModel,
            code: "mixed_criticality_not_supported",
            detail: "mixed criticality scenarios are not yet supported in SimSo differential"
                .to_string(),
        });
    }
    Ok(())
}

fn execute_adapter(
    config: &SimsoAdapterConfig,
    input_json: &str,
) -> Result<String, SimsoAdapterError> {
    if !config.runner.exists() {
        return Err(SimsoAdapterError::RunnerMissing {
            path: config.runner.clone(),
        });
    }

    let mut command = Command::new(&config.python_bin);
    command
        .arg(&config.runner)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(SimsoAdapterError::RunnerIo)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| SimsoAdapterError::StdinWrite(std::io::Error::other("missing stdin")))?;
    stdin
        .write_all(input_json.as_bytes())
        .map_err(SimsoAdapterError::StdinWrite)?;
    drop(stdin);

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(SimsoAdapterError::RunnerIo)? {
            break status;
        }
        if start.elapsed() > config.adapter_timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SimsoAdapterError::RunnerTimeout {
                timeout_ms: config.adapter_timeout.as_millis() as u64,
            });
        }
        thread::sleep(Duration::from_millis(10));
    };

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)
            .map_err(SimsoAdapterError::RunnerIo)?;
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)
            .map_err(SimsoAdapterError::RunnerIo)?;
    }

    if !status.success() {
        return Err(SimsoAdapterError::RunnerFailed {
            code: status.code().unwrap_or(-1),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(stdout.trim().to_string())
}

fn to_adapter_task(task_id: u32, task: &CpuOnlyTask) -> SimsoAdapterTask {
    SimsoAdapterTask {
        task_id,
        period_ns: task.period,
        deadline_ns: task.deadline,
        wcet_ns: task.wcet,
        priority: task.priority,
    }
}

fn collect_summary_mismatches(
    hprss: &CpuOnlyRunSummary,
    simso: &SimsoRunSummary,
    requested_scheduler: &'static str,
    miss_ratio_tolerance: f64,
) -> Vec<SimsoSummaryMismatch> {
    let mut mismatches = Vec::new();
    match simso.scheduler.as_deref() {
        Some(observed_scheduler) if observed_scheduler != requested_scheduler => {
            mismatches.push(SimsoSummaryMismatch {
                category: SimsoMismatchCategory::Scheduler,
                field: "scheduler",
                expected: requested_scheduler.to_string(),
                observed: observed_scheduler.to_string(),
                detail: "adapter scheduler does not match requested scheduler".to_string(),
            });
        }
        Some(_) => {}
        None => {
            mismatches.push(SimsoSummaryMismatch {
                category: SimsoMismatchCategory::Scheduler,
                field: "scheduler",
                expected: requested_scheduler.to_string(),
                observed: "<missing>".to_string(),
                detail: "adapter output missing required scheduler field".to_string(),
            });
        }
    }
    if hprss.deadline_misses != simso.deadline_misses {
        mismatches.push(SimsoSummaryMismatch {
            category: SimsoMismatchCategory::DeadlineMisses,
            field: "deadline_misses",
            expected: hprss.deadline_misses.to_string(),
            observed: simso.deadline_misses.to_string(),
            detail: "deadline misses differ between hprss and SimSo".to_string(),
        });
    }
    if hprss.completion_count != simso.completion_count {
        mismatches.push(SimsoSummaryMismatch {
            category: SimsoMismatchCategory::CompletionCount,
            field: "completion_count",
            expected: hprss.completion_count.to_string(),
            observed: simso.completion_count.to_string(),
            detail: "completion count differs between hprss and SimSo".to_string(),
        });
    }
    let mut miss_ratio_notes = Vec::new();
    let miss_ratio_delta = (hprss.miss_ratio - simso.miss_ratio).abs();
    if miss_ratio_delta > miss_ratio_tolerance {
        miss_ratio_notes.push(format!(
            "absolute delta to hprss ({miss_ratio_delta}) exceeds tolerance {miss_ratio_tolerance}"
        ));
    }
    let simso_total_jobs = simso.deadline_misses.saturating_add(simso.completion_count);
    let derived_simso_miss_ratio = if simso_total_jobs == 0 {
        0.0
    } else {
        simso.deadline_misses as f64 / simso_total_jobs as f64
    };
    let simso_self_delta = (simso.miss_ratio - derived_simso_miss_ratio).abs();
    if simso_self_delta > miss_ratio_tolerance {
        miss_ratio_notes.push(format!(
            "adapter miss_ratio inconsistent with deadline_misses/(deadline_misses+completion_count) (expected {derived_simso_miss_ratio}, observed {})",
            simso.miss_ratio
        ));
    }
    if !miss_ratio_notes.is_empty() {
        mismatches.push(SimsoSummaryMismatch {
            category: SimsoMismatchCategory::MissRatio,
            field: "miss_ratio",
            expected: hprss.miss_ratio.to_string(),
            observed: simso.miss_ratio.to_string(),
            detail: miss_ratio_notes.join("; "),
        });
    }
    mismatches
}

fn scheduler_label(scheduler: CpuOnlySchedulerConfig) -> &'static str {
    match scheduler {
        CpuOnlySchedulerConfig::FixedPriority => "fp",
        CpuOnlySchedulerConfig::Edf => "edf",
    }
}

fn find_value<'a>(obj: &'a Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| obj.get(*name))
}

fn extract_u64(
    obj: &Map<String, Value>,
    names: &[&str],
    canonical: &'static str,
) -> Result<u64, SimsoAdapterError> {
    let value =
        find_value(obj, names).ok_or(SimsoAdapterError::MissingField { field: canonical })?;
    match value {
        Value::Number(number) => {
            if let Some(v) = number.as_u64() {
                Ok(v)
            } else if let Some(v) = number.as_i64() {
                if v >= 0 {
                    Ok(v as u64)
                } else {
                    Err(SimsoAdapterError::InvalidField {
                        field: canonical,
                        value: value.to_string(),
                    })
                }
            } else if let Some(v) = number.as_f64() {
                if v.is_finite() && v >= 0.0 && v.fract().abs() <= f64::EPSILON {
                    Ok(v as u64)
                } else {
                    Err(SimsoAdapterError::InvalidField {
                        field: canonical,
                        value: value.to_string(),
                    })
                }
            } else {
                Err(SimsoAdapterError::InvalidField {
                    field: canonical,
                    value: value.to_string(),
                })
            }
        }
        _ => Err(SimsoAdapterError::InvalidField {
            field: canonical,
            value: value.to_string(),
        }),
    }
}

fn extract_f64(
    obj: &Map<String, Value>,
    names: &[&str],
    canonical: &'static str,
) -> Result<f64, SimsoAdapterError> {
    let value =
        find_value(obj, names).ok_or(SimsoAdapterError::MissingField { field: canonical })?;
    match value {
        Value::Number(number) => number
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| SimsoAdapterError::InvalidField {
                field: canonical,
                value: value.to_string(),
            }),
        _ => Err(SimsoAdapterError::InvalidField {
            field: canonical,
            value: value.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_rejects_non_object_payload() {
        let err = normalize_simso_output("[]").expect_err("root must be object");
        assert!(matches!(
            err,
            SimsoAdapterError::InvalidField { field: "root", .. }
        ));
    }

    #[test]
    fn normalization_accepts_canonical_payload() {
        let normalized = normalize_simso_output(
            r#"{"scheduler":"edf","deadline_misses":0,"completion_count":4,"miss_ratio":0.0}"#,
        )
        .expect("canonical format should parse");
        assert_eq!(normalized.deadline_misses, 0);
        assert_eq!(normalized.completion_count, 4);
        assert_eq!(normalized.scheduler.as_deref(), Some("edf"));
    }

    #[test]
    fn summary_mismatch_diagnostics_include_categories() {
        let mismatches = collect_summary_mismatches(
            &CpuOnlyRunSummary {
                scheduler: CpuOnlySchedulerConfig::FixedPriority,
                deadline_misses: 0,
                completion_count: 10,
                miss_ratio: 0.0,
            },
            &SimsoRunSummary {
                scheduler: Some("edf".to_string()),
                deadline_misses: 2,
                completion_count: 7,
                miss_ratio: 0.2857142857142857,
            },
            "fp",
            1e-12,
        );
        assert_eq!(mismatches.len(), 4);
        assert!(
            mismatches
                .iter()
                .any(|m| m.category == SimsoMismatchCategory::Scheduler)
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m.category == SimsoMismatchCategory::DeadlineMisses)
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m.category == SimsoMismatchCategory::CompletionCount)
        );
        assert!(
            mismatches
                .iter()
                .any(|m| m.category == SimsoMismatchCategory::MissRatio)
        );
    }

    #[test]
    fn summary_mismatch_diagnostics_flag_inconsistent_simso_ratio() {
        let mismatches = collect_summary_mismatches(
            &CpuOnlyRunSummary {
                scheduler: CpuOnlySchedulerConfig::FixedPriority,
                deadline_misses: 7,
                completion_count: 777,
                miss_ratio: 7.0 / 777.0,
            },
            &SimsoRunSummary {
                scheduler: Some("fp".to_string()),
                deadline_misses: 7,
                completion_count: 777,
                miss_ratio: 0.123456,
            },
            "fp",
            1e-12,
        );
        let ratio = mismatches
            .into_iter()
            .find(|m| m.category == SimsoMismatchCategory::MissRatio)
            .expect("miss_ratio mismatch should be reported");
        assert!(
            ratio
                .detail
                .contains("deadline_misses/(deadline_misses+completion_count)")
        );
    }

    #[test]
    fn summary_mismatch_diagnostics_require_scheduler_field() {
        let mismatches = collect_summary_mismatches(
            &CpuOnlyRunSummary {
                scheduler: CpuOnlySchedulerConfig::FixedPriority,
                deadline_misses: 0,
                completion_count: 10,
                miss_ratio: 0.0,
            },
            &SimsoRunSummary {
                scheduler: None,
                deadline_misses: 0,
                completion_count: 10,
                miss_ratio: 0.0,
            },
            "fp",
            1e-12,
        );
        let scheduler = mismatches
            .iter()
            .find(|m| m.category == SimsoMismatchCategory::Scheduler)
            .expect("missing scheduler must be reported");
        assert_eq!(scheduler.observed, "<missing>");
    }

    #[test]
    fn strict_cpu_only_scenario_validation_passes() {
        let scenario = SimsoScenarioModel::strict_cpu_only();
        assert!(validate_simso_scenario(&scenario).is_ok());
    }

    #[test]
    fn scenario_validation_categorizes_unsupported_reasons() {
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
                    core_count: 4,
                    ..SimsoScenarioModel::strict_cpu_only()
                },
                SimsoDiagnosticCategory::ResourceTopology,
                "core_count_not_supported",
            ),
            (
                SimsoScenarioModel {
                    task_model: SimsoTaskModel::Dag,
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
            let err = validate_simso_scenario(&scenario).expect_err("scenario should be rejected");
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
}
