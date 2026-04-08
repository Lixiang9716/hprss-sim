use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
    pub outputs_match: bool,
}

#[derive(Debug, Clone)]
pub struct SimsoAdapterConfig {
    pub runner: PathBuf,
    pub python_bin: String,
    pub miss_ratio_tolerance: f64,
    fixture_mode: Option<String>,
}

impl Default for SimsoAdapterConfig {
    fn default() -> Self {
        Self {
            runner: default_simso_adapter_runner(),
            python_bin: "python3".to_string(),
            miss_ratio_tolerance: DEFAULT_MISS_RATIO_TOLERANCE,
            fixture_mode: None,
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

    pub fn with_fixture_mode(mut self, mode: impl Into<String>) -> Self {
        self.fixture_mode = Some(mode.into());
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
    #[error("adapter runner exited with code {code}: {stderr}")]
    RunnerFailed { code: i32, stderr: String },
    #[error("adapter returned invalid JSON: {0}")]
    ParseOutput(serde_json::Error),
    #[error("adapter output missing required field `{field}`")]
    MissingField { field: &'static str },
    #[error("adapter output field `{field}` has invalid value: {value}")]
    InvalidField { field: &'static str, value: String },
}

#[derive(Debug, Clone, Serialize)]
struct SimsoAdapterInput {
    adapter_contract: &'static str,
    workload: String,
    horizon_ns: u64,
    scheduler: &'static str,
    tasks: Vec<SimsoAdapterTask>,
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
    let adapter_input = serde_json::to_string(&build_adapter_input(workload, scheduler))
        .map_err(SimsoAdapterError::SerializeInput)?;
    let adapter_output = execute_adapter(config, &adapter_input)?;
    let simso = normalize_simso_output(&adapter_output)?;
    let outputs_match = summaries_match(&hprss, &simso, config.miss_ratio_tolerance);

    Ok(Level3SimsoDifferentialReport {
        scope: LEVEL3_SCOPE,
        workload: workload.name.to_string(),
        scheduler,
        hprss,
        simso,
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
) -> SimsoAdapterInput {
    SimsoAdapterInput {
        adapter_contract: ADAPTER_CONTRACT,
        workload: workload.name.to_string(),
        horizon_ns: workload.horizon,
        scheduler: scheduler_label(scheduler),
        tasks: workload
            .tasks
            .iter()
            .enumerate()
            .map(|(task_id, task)| to_adapter_task(task_id as u32, task))
            .collect(),
    }
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
    if let Some(mode) = &config.fixture_mode {
        command.env("HPRSS_SIMSO_FIXTURE_MODE", mode);
    }

    let mut child = command.spawn().map_err(SimsoAdapterError::RunnerIo)?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| SimsoAdapterError::StdinWrite(std::io::Error::other("missing stdin")))?;
        stdin
            .write_all(input_json.as_bytes())
            .map_err(SimsoAdapterError::StdinWrite)?;
    }

    let output = child
        .wait_with_output()
        .map_err(SimsoAdapterError::RunnerIo)?;
    if !output.status.success() {
        return Err(SimsoAdapterError::RunnerFailed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn summaries_match(
    hprss: &CpuOnlyRunSummary,
    simso: &SimsoRunSummary,
    miss_ratio_tolerance: f64,
) -> bool {
    hprss.deadline_misses == simso.deadline_misses
        && hprss.completion_count == simso.completion_count
        && (hprss.miss_ratio - simso.miss_ratio).abs() <= miss_ratio_tolerance
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
}
