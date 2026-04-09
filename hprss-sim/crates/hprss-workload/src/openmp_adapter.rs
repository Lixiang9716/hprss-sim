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
pub struct OpenMpSpecializedWorkload {
    pub regions: Vec<OpenMpRegionSpec>,
    pub instances: Vec<OpenMpRegionInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenMpRegionSpec {
    pub region_id: u32,
    pub name: String,
    pub priority: u32,
    #[serde(default = "default_criticality")]
    pub criticality: CriticalityLevel,
    pub requested_threads: u32,
    pub schedule_kind: String,
    #[serde(default)]
    pub chunk_size: Option<u32>,
    #[serde(default)]
    pub loop_iteration_count: Option<u64>,
    pub affinity: Vec<DeviceType>,
    pub device_profiles: Vec<OpenMpDeviceProfile>,
    #[serde(default)]
    pub relative_deadline_ns: Option<u64>,
    #[serde(default)]
    pub data_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenMpDeviceProfile {
    pub device_type: DeviceType,
    pub wcet_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenMpRegionInstance {
    pub region_id: u32,
    pub release_ns: u64,
    #[serde(default)]
    pub absolute_deadline_ns: Option<u64>,
    #[serde(default)]
    pub observed_exec_ns: Option<u64>,
}

#[derive(Debug, Error)]
pub enum OpenMpSpecializedWorkloadError {
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("openmp adapter validation failed: {0}")]
    Validation(String),
    #[error(transparent)]
    Replay(#[from] ReplayWorkloadError),
}

pub fn adapt_openmp_specialized_json_str(
    json: &str,
) -> Result<ReplayWorkload, OpenMpSpecializedWorkloadError> {
    let workload: OpenMpSpecializedWorkload = serde_json::from_str(json)?;
    workload.to_replay_workload()
}

pub fn adapt_openmp_specialized_json(
    path: &Path,
) -> Result<ReplayWorkload, OpenMpSpecializedWorkloadError> {
    let content = std::fs::read_to_string(path)?;
    adapt_openmp_specialized_json_str(&content)
}

impl OpenMpSpecializedWorkload {
    fn to_replay_workload(&self) -> Result<ReplayWorkload, OpenMpSpecializedWorkloadError> {
        if self.regions.is_empty() {
            return Err(OpenMpSpecializedWorkloadError::Validation(
                "regions list must not be empty".to_string(),
            ));
        }
        if self.instances.is_empty() {
            return Err(OpenMpSpecializedWorkloadError::Validation(
                "instances list must not be empty".to_string(),
            ));
        }

        let mut assumptions = vec![ReplayAssumption {
            code: "omp-region-collapsed-to-task".to_string(),
            detail: "Each OpenMP parallel region is collapsed into one simulator task.".to_string(),
        }];
        let mut seen_assumption_codes = HashSet::from(["omp-region-collapsed-to-task".to_string()]);

        let mut seen_region_ids = HashSet::new();
        let mut region_map = HashMap::new();
        let mut tasks = Vec::with_capacity(self.regions.len());

        for region in &self.regions {
            if !seen_region_ids.insert(region.region_id) {
                return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                    "duplicate region_id {}",
                    region.region_id
                )));
            }
            if region.priority == 0 {
                return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                    "region {} has invalid priority 0",
                    region.region_id
                )));
            }
            if region.requested_threads == 0 {
                return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                    "region {} requested_threads must be > 0",
                    region.region_id
                )));
            }
            if region.affinity.is_empty() {
                return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                    "region {} has empty affinity",
                    region.region_id
                )));
            }
            if region.device_profiles.is_empty() {
                return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                    "region {} has empty device_profiles",
                    region.region_id
                )));
            }

            let mut seen_profile_device = HashSet::new();
            for profile in &region.device_profiles {
                if profile.wcet_ns == 0 {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "region {} has wcet_ns=0 for {:?}",
                        region.region_id, profile.device_type
                    )));
                }
                if !seen_profile_device.insert(profile.device_type) {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "region {} has duplicate device profile for {:?}",
                        region.region_id, profile.device_type
                    )));
                }
            }

            for affinity in &region.affinity {
                if !region
                    .device_profiles
                    .iter()
                    .any(|profile| profile.device_type == *affinity)
                {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "region {} affinity {:?} missing in device_profiles",
                        region.region_id, affinity
                    )));
                }
            }

            let max_wcet = region
                .device_profiles
                .iter()
                .map(|profile| profile.wcet_ns)
                .max()
                .unwrap_or(0);

            let deadline_ns = match region.relative_deadline_ns {
                Some(v) if v > 0 => v,
                Some(_) => {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "region {} has relative_deadline_ns=0",
                        region.region_id
                    )));
                }
                None => {
                    push_assumption(
                        &mut assumptions,
                        &mut seen_assumption_codes,
                        "omp-derived-relative-deadline",
                        "Missing region relative_deadline_ns is approximated with max device WCET.",
                    );
                    max_wcet
                }
            };

            let exec_hints = region
                .device_profiles
                .iter()
                .map(|profile| ReplayExecHint {
                    device_type: profile.device_type,
                    wcet_ns: profile.wcet_ns,
                })
                .collect();

            region_map.insert(region.region_id, (deadline_ns, max_wcet));
            tasks.push(ReplayTaskSpec {
                task_id: region.region_id,
                name: region.name.clone(),
                priority: region.priority,
                deadline_ns,
                criticality: region.criticality,
                affinity: region.affinity.clone(),
                exec_hints,
                data_size: region.data_size,
            });
        }

        let mut jobs = Vec::with_capacity(self.instances.len());
        for instance in &self.instances {
            let (deadline_ns, max_wcet) =
                region_map
                    .get(&instance.region_id)
                    .copied()
                    .ok_or_else(|| {
                        OpenMpSpecializedWorkloadError::Validation(format!(
                            "instance references unknown region_id {}",
                            instance.region_id
                        ))
                    })?;

            if let Some(observed_exec_ns) = instance.observed_exec_ns {
                if observed_exec_ns == 0 {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "instance for region {} has observed_exec_ns=0",
                        instance.region_id
                    )));
                }
                if observed_exec_ns > max_wcet {
                    return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                        "instance for region {} has observed_exec_ns={} exceeding max wcet {}",
                        instance.region_id, observed_exec_ns, max_wcet
                    )));
                }
            } else {
                push_assumption(
                    &mut assumptions,
                    &mut seen_assumption_codes,
                    "omp-missing-observed-exec",
                    "Missing observed_exec_ns leaves per-job execution unresolved for device-time resolution.",
                );
            }

            let absolute_deadline_ns = match instance.absolute_deadline_ns {
                Some(v) => {
                    if v < instance.release_ns {
                        return Err(OpenMpSpecializedWorkloadError::Validation(format!(
                            "instance for region {} has absolute_deadline_ns < release_ns",
                            instance.region_id
                        )));
                    }
                    Some(v)
                }
                None => {
                    push_assumption(
                        &mut assumptions,
                        &mut seen_assumption_codes,
                        "omp-derived-absolute-deadline",
                        "Missing instance absolute_deadline_ns is approximated as release_ns + relative_deadline_ns.",
                    );
                    Some(instance.release_ns.saturating_add(deadline_ns))
                }
            };

            jobs.push(ReplayJobSpec {
                task_id: instance.region_id,
                release_ns: instance.release_ns,
                absolute_deadline_ns,
                actual_exec_ns: instance.observed_exec_ns,
            });
        }

        let mut replay = ReplayWorkload {
            tasks,
            jobs,
            metadata: ReplayMetadata {
                source: Some("openmp-specialized".to_string()),
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
