use std::collections::{HashMap, HashSet};
use std::path::Path;

use hprss_types::CriticalityLevel;
use hprss_types::task::DeviceType;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::replay::{
    ReplayAssumption, ReplayExecHint, ReplayJobSpec, ReplayMetadata, ReplayTaskSpec,
    ReplayWorkload, ReplayWorkloadError,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaramiPaperProfileWorkload {
    pub profile_name: String,
    #[serde(default)]
    pub paper_reference: Option<String>,
    #[serde(default)]
    pub assumptions: Vec<KaramiAssumptionSpec>,
    pub scenarios: Vec<KaramiScenarioSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaramiAssumptionSpec {
    pub code: String,
    pub detail: String,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaramiScenarioSpec {
    pub scenario_id: u32,
    pub name: String,
    pub priority: u32,
    #[serde(default = "default_criticality")]
    pub criticality: CriticalityLevel,
    pub period_ns: u64,
    #[serde(default)]
    pub relative_deadline_ns: Option<u64>,
    pub release_offsets_ns: Vec<u64>,
    pub affinity: Vec<DeviceType>,
    pub execution_profiles: Vec<KaramiExecutionProfile>,
    #[serde(default)]
    pub observed_exec_ns: Option<u64>,
    #[serde(default)]
    pub data_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaramiExecutionProfile {
    pub device_type: DeviceType,
    pub wcet_ns: u64,
}

#[derive(Debug, Error)]
pub enum KaramiPaperProfileWorkloadError {
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("karami profile adapter validation failed: {0}")]
    Validation(String),
    #[error(transparent)]
    Replay(#[from] ReplayWorkloadError),
}

pub fn adapt_karami_paper_profile_json_str(
    json: &str,
) -> Result<ReplayWorkload, KaramiPaperProfileWorkloadError> {
    let workload: KaramiPaperProfileWorkload = serde_json::from_str(json)?;
    workload.to_replay_workload()
}

pub fn adapt_karami_paper_profile_json(
    path: &Path,
) -> Result<ReplayWorkload, KaramiPaperProfileWorkloadError> {
    let content = std::fs::read_to_string(path)?;
    adapt_karami_paper_profile_json_str(&content)
}

impl KaramiPaperProfileWorkload {
    fn to_replay_workload(&self) -> Result<ReplayWorkload, KaramiPaperProfileWorkloadError> {
        if self.profile_name.trim().is_empty() {
            return Err(KaramiPaperProfileWorkloadError::Validation(
                "profile_name must not be empty".to_string(),
            ));
        }
        if self.scenarios.is_empty() {
            return Err(KaramiPaperProfileWorkloadError::Validation(
                "scenarios list must not be empty".to_string(),
            ));
        }

        let mut assumptions = vec![ReplayAssumption {
            code: "karami-paper-profile".to_string(),
            detail: format!(
                "Workload adapted from paper profile '{}' for deterministic simulator replay.",
                self.profile_name
            ),
        }];
        let mut seen_assumption_codes = HashSet::from(["karami-paper-profile".to_string()]);
        for assumption in &self.assumptions {
            if assumption.code.trim().is_empty() {
                return Err(KaramiPaperProfileWorkloadError::Validation(
                    "assumptions[*].code must not be empty".to_string(),
                ));
            }
            if assumption.detail.trim().is_empty() {
                return Err(KaramiPaperProfileWorkloadError::Validation(
                    "assumptions[*].detail must not be empty".to_string(),
                ));
            }

            let mut detail = assumption.detail.clone();
            if let Some(rationale) = &assumption.rationale
                && !rationale.trim().is_empty()
            {
                detail = format!("{detail} [rationale: {}]", rationale.trim());
            }
            push_assumption(
                &mut assumptions,
                &mut seen_assumption_codes,
                &assumption.code,
                &detail,
            );
        }

        let mut seen_scenario_ids = HashSet::new();
        let mut tasks = Vec::with_capacity(self.scenarios.len());
        let mut jobs = Vec::new();
        let mut scenario_deadline = HashMap::new();
        let mut scenarios = self.scenarios.clone();
        scenarios.sort_by_key(|scenario| scenario.scenario_id);

        for scenario in &scenarios {
            if !seen_scenario_ids.insert(scenario.scenario_id) {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "duplicate scenario_id {}",
                    scenario.scenario_id
                )));
            }
            if scenario.priority == 0 {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "scenario {} has invalid priority 0",
                    scenario.scenario_id
                )));
            }
            if scenario.period_ns == 0 {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "scenario {} has period_ns=0",
                    scenario.scenario_id
                )));
            }
            if scenario.release_offsets_ns.is_empty() {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "scenario {} has empty release_offsets_ns",
                    scenario.scenario_id
                )));
            }
            if scenario.affinity.is_empty() {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "scenario {} has empty affinity",
                    scenario.scenario_id
                )));
            }
            if scenario.execution_profiles.is_empty() {
                return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                    "scenario {} has empty execution_profiles",
                    scenario.scenario_id
                )));
            }

            let mut seen_profile_device = HashSet::new();
            for profile in &scenario.execution_profiles {
                if profile.wcet_ns == 0 {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} has wcet_ns=0 for {:?}",
                        scenario.scenario_id, profile.device_type
                    )));
                }
                if !seen_profile_device.insert(profile.device_type) {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} has duplicate execution profile for {:?}",
                        scenario.scenario_id, profile.device_type
                    )));
                }
            }
            for affinity in &scenario.affinity {
                if !scenario
                    .execution_profiles
                    .iter()
                    .any(|profile| profile.device_type == *affinity)
                {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} affinity {:?} missing in execution_profiles",
                        scenario.scenario_id, affinity
                    )));
                }
            }

            let max_wcet = scenario
                .execution_profiles
                .iter()
                .map(|profile| profile.wcet_ns)
                .max()
                .unwrap_or(0);
            let deadline_ns = match scenario.relative_deadline_ns {
                Some(v) if v > 0 => v,
                Some(_) => {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} has relative_deadline_ns=0",
                        scenario.scenario_id
                    )));
                }
                None => {
                    push_assumption(
                        &mut assumptions,
                        &mut seen_assumption_codes,
                        "karami-derived-relative-deadline",
                        "Missing scenario relative_deadline_ns is approximated as period_ns.",
                    );
                    scenario.period_ns
                }
            };

            if let Some(observed_exec_ns) = scenario.observed_exec_ns {
                if observed_exec_ns == 0 {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} has observed_exec_ns=0",
                        scenario.scenario_id
                    )));
                }
                if observed_exec_ns > max_wcet {
                    return Err(KaramiPaperProfileWorkloadError::Validation(format!(
                        "scenario {} has observed_exec_ns={} exceeding max wcet {}",
                        scenario.scenario_id, observed_exec_ns, max_wcet
                    )));
                }
            } else {
                push_assumption(
                    &mut assumptions,
                    &mut seen_assumption_codes,
                    "karami-missing-observed-exec",
                    "Missing scenario observed_exec_ns leaves per-job execution unresolved for device-time resolution.",
                );
            }

            let exec_hints = scenario
                .execution_profiles
                .iter()
                .map(|profile| ReplayExecHint {
                    device_type: profile.device_type,
                    wcet_ns: profile.wcet_ns,
                })
                .collect();
            tasks.push(ReplayTaskSpec {
                task_id: scenario.scenario_id,
                name: scenario.name.clone(),
                priority: scenario.priority,
                deadline_ns,
                criticality: scenario.criticality,
                affinity: scenario.affinity.clone(),
                exec_hints,
                data_size: scenario.data_size,
            });
            scenario_deadline.insert(
                scenario.scenario_id,
                (
                    deadline_ns,
                    scenario.observed_exec_ns.is_some(),
                    scenario.observed_exec_ns,
                ),
            );
        }

        for scenario in &scenarios {
            let (deadline_ns, has_observed_exec, observed_exec_ns) = scenario_deadline
                .get(&scenario.scenario_id)
                .copied()
                .ok_or_else(|| {
                    KaramiPaperProfileWorkloadError::Validation(format!(
                        "internal adapter error: unknown scenario {}",
                        scenario.scenario_id
                    ))
                })?;

            let mut release_offsets = scenario.release_offsets_ns.clone();
            release_offsets.sort_unstable();
            for release_ns in release_offsets {
                jobs.push(ReplayJobSpec {
                    task_id: scenario.scenario_id,
                    release_ns,
                    absolute_deadline_ns: Some(release_ns.saturating_add(deadline_ns)),
                    actual_exec_ns: if has_observed_exec {
                        observed_exec_ns
                    } else {
                        None
                    },
                });
            }
        }

        let mut replay = ReplayWorkload {
            tasks,
            jobs,
            metadata: ReplayMetadata {
                source: Some("karami-paper-profile".to_string()),
                assumptions,
            },
        };
        replay.validate_and_normalize()?;
        Ok(replay)
    }
}

fn push_assumption(
    assumptions: &mut Vec<ReplayAssumption>,
    seen_codes: &mut HashSet<String>,
    code: &str,
    detail: &str,
) {
    if seen_codes.insert(code.to_string()) {
        assumptions.push(ReplayAssumption {
            code: code.to_string(),
            detail: detail.to_string(),
        });
    }
}

const fn default_criticality() -> CriticalityLevel {
    CriticalityLevel::Lo
}
