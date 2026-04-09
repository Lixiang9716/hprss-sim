use std::collections::{HashMap, HashSet};
use std::path::Path;

use hprss_types::{
    CriticalityLevel, Task, TaskId,
    task::{ArrivalModel, DeviceType, ExecutionTimeModel},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Trace replay workload schema.
///
/// JSON shape:
/// - `tasks`: array of [`ReplayTaskSpec`]
/// - `jobs`: array of [`ReplayJobSpec`]
///
/// CSV shape:
/// - `tasks.csv` columns: `task_id,name,priority,deadline_ns,criticality,affinity,exec_hints,data_size`
///   where `affinity` and `exec_hints` are JSON-encoded arrays.
/// - `jobs.csv` columns: `task_id,release_ns,absolute_deadline_ns,actual_exec_ns`
///
/// Replay tasks are converted into aperiodic engine tasks, and replay jobs provide explicit
/// release timestamps plus optional absolute deadlines and execution-time hints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayTaskSpec {
    pub task_id: u32,
    pub name: String,
    pub priority: u32,
    pub deadline_ns: u64,
    #[serde(default = "default_criticality")]
    pub criticality: CriticalityLevel,
    pub affinity: Vec<DeviceType>,
    pub exec_hints: Vec<ReplayExecHint>,
    #[serde(default)]
    pub data_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayExecHint {
    pub device_type: DeviceType,
    pub wcet_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayJobSpec {
    pub task_id: u32,
    pub release_ns: u64,
    #[serde(default)]
    pub absolute_deadline_ns: Option<u64>,
    #[serde(default)]
    pub actual_exec_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayWorkload {
    pub tasks: Vec<ReplayTaskSpec>,
    pub jobs: Vec<ReplayJobSpec>,
}

#[derive(Debug, Error)]
pub enum ReplayWorkloadError {
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse failure: {0}")]
    Json(#[from] serde_json::Error),
    #[error("CSV parse failure: {0}")]
    Csv(#[from] csv::Error),
    #[error("replay validation failed: {0}")]
    Validation(String),
}

impl ReplayWorkload {
    /// Parse replay workload from JSON and apply deterministic normalization:
    /// - validates schema-level/runtime constraints
    /// - sorts jobs by `(release_ns, task_id, absolute_deadline_ns)`
    pub fn from_json_str(json: &str) -> Result<Self, ReplayWorkloadError> {
        let mut workload: ReplayWorkload = serde_json::from_str(json)?;
        workload.validate_and_normalize()?;
        Ok(workload)
    }

    /// Parse replay workload from CSV task/job payloads with the same normalization
    /// guarantees as [`Self::from_json_str`].
    pub fn from_csv_str(tasks_csv: &str, jobs_csv: &str) -> Result<Self, ReplayWorkloadError> {
        let mut task_reader = csv::Reader::from_reader(tasks_csv.as_bytes());
        let mut tasks = Vec::new();
        for row in task_reader.deserialize::<ReplayTaskCsvRow>() {
            let row = row?;
            tasks.push(row.into_task_spec()?);
        }

        let mut job_reader = csv::Reader::from_reader(jobs_csv.as_bytes());
        let mut jobs = Vec::new();
        for row in job_reader.deserialize::<ReplayJobSpec>() {
            jobs.push(row?);
        }

        let mut workload = ReplayWorkload { tasks, jobs };
        workload.validate_and_normalize()?;
        Ok(workload)
    }

    /// Convert replay task specs into engine task descriptors.
    ///
    /// All generated tasks are aperiodic; replay jobs provide release/deadline timing.
    pub fn to_tasks(&self) -> Vec<Task> {
        self.tasks
            .iter()
            .map(|spec| Task {
                id: TaskId(spec.task_id),
                name: spec.name.clone(),
                priority: spec.priority,
                arrival: ArrivalModel::Aperiodic,
                deadline: spec.deadline_ns,
                criticality: spec.criticality,
                exec_times: spec
                    .exec_hints
                    .iter()
                    .map(|hint| {
                        (
                            hint.device_type,
                            ExecutionTimeModel::Deterministic { wcet: hint.wcet_ns },
                        )
                    })
                    .collect(),
                affinity: spec.affinity.clone(),
                data_size: spec.data_size,
            })
            .collect()
    }

    /// Access normalized replay jobs in deterministic dispatch order.
    pub fn jobs(&self) -> &[ReplayJobSpec] {
        &self.jobs
    }

    fn validate_and_normalize(&mut self) -> Result<(), ReplayWorkloadError> {
        if self.tasks.is_empty() {
            return Err(ReplayWorkloadError::Validation(
                "tasks list must not be empty".to_string(),
            ));
        }
        if self.jobs.is_empty() {
            return Err(ReplayWorkloadError::Validation(
                "jobs list must not be empty".to_string(),
            ));
        }

        let mut task_ids = HashSet::new();
        for task in &self.tasks {
            if !task_ids.insert(task.task_id) {
                return Err(ReplayWorkloadError::Validation(format!(
                    "duplicate task_id {}",
                    task.task_id
                )));
            }
            if task.priority == 0 {
                return Err(ReplayWorkloadError::Validation(format!(
                    "task {} has invalid priority 0",
                    task.task_id
                )));
            }
            if task.deadline_ns == 0 {
                return Err(ReplayWorkloadError::Validation(format!(
                    "task {} has deadline_ns=0",
                    task.task_id
                )));
            }
            if task.affinity.is_empty() {
                return Err(ReplayWorkloadError::Validation(format!(
                    "task {} has empty affinity",
                    task.task_id
                )));
            }
            if task.exec_hints.is_empty() {
                return Err(ReplayWorkloadError::Validation(format!(
                    "task {} has empty exec_hints",
                    task.task_id
                )));
            }
            let mut seen_hint_device = HashSet::new();
            for hint in &task.exec_hints {
                if hint.wcet_ns == 0 {
                    return Err(ReplayWorkloadError::Validation(format!(
                        "task {} has wcet_ns=0 for {:?}",
                        task.task_id, hint.device_type
                    )));
                }
                if !seen_hint_device.insert(hint.device_type) {
                    return Err(ReplayWorkloadError::Validation(format!(
                        "task {} has duplicate exec_hints for {:?}",
                        task.task_id, hint.device_type
                    )));
                }
            }
            for affinity in &task.affinity {
                if !task
                    .exec_hints
                    .iter()
                    .any(|hint| hint.device_type == *affinity)
                {
                    return Err(ReplayWorkloadError::Validation(format!(
                        "task {} affinity {:?} missing in exec_hints",
                        task.task_id, affinity
                    )));
                }
            }
        }

        let by_task: HashMap<u32, &ReplayTaskSpec> =
            self.tasks.iter().map(|task| (task.task_id, task)).collect();
        for job in &self.jobs {
            let Some(task) = by_task.get(&job.task_id) else {
                return Err(ReplayWorkloadError::Validation(format!(
                    "job references unknown task_id {}",
                    job.task_id
                )));
            };

            if let Some(actual_exec_ns) = job.actual_exec_ns {
                if actual_exec_ns == 0 {
                    return Err(ReplayWorkloadError::Validation(format!(
                        "job for task {} has actual_exec_ns=0",
                        job.task_id
                    )));
                }
                let max_wcet = task
                    .exec_hints
                    .iter()
                    .map(|hint| hint.wcet_ns)
                    .max()
                    .unwrap_or(0);
                if actual_exec_ns > max_wcet {
                    return Err(ReplayWorkloadError::Validation(format!(
                        "job for task {} has actual_exec_ns={} exceeding max wcet {}",
                        job.task_id, actual_exec_ns, max_wcet
                    )));
                }
            }

            if let Some(absolute_deadline_ns) = job.absolute_deadline_ns
                && absolute_deadline_ns < job.release_ns
            {
                return Err(ReplayWorkloadError::Validation(format!(
                    "job for task {} has absolute_deadline_ns < release_ns",
                    job.task_id
                )));
            }
        }

        self.jobs
            .sort_by_key(|job| (job.release_ns, job.task_id, job.absolute_deadline_ns));

        Ok(())
    }
}

pub fn load_replay_json(path: &Path) -> Result<ReplayWorkload, ReplayWorkloadError> {
    let content = std::fs::read_to_string(path)?;
    ReplayWorkload::from_json_str(&content)
}

pub fn load_replay_csv(
    tasks_path: &Path,
    jobs_path: &Path,
) -> Result<ReplayWorkload, ReplayWorkloadError> {
    let tasks = std::fs::read_to_string(tasks_path)?;
    let jobs = std::fs::read_to_string(jobs_path)?;
    ReplayWorkload::from_csv_str(&tasks, &jobs)
}

#[derive(Debug, Deserialize)]
struct ReplayTaskCsvRow {
    task_id: u32,
    name: String,
    priority: u32,
    deadline_ns: u64,
    #[serde(default = "default_criticality")]
    criticality: CriticalityLevel,
    affinity: String,
    exec_hints: String,
    #[serde(default)]
    data_size: u64,
}

impl ReplayTaskCsvRow {
    fn into_task_spec(self) -> Result<ReplayTaskSpec, ReplayWorkloadError> {
        let affinity = serde_json::from_str(&self.affinity).map_err(|e| {
            ReplayWorkloadError::Validation(format!(
                "task {} has invalid affinity JSON: {e}",
                self.task_id
            ))
        })?;
        let exec_hints = serde_json::from_str(&self.exec_hints).map_err(|e| {
            ReplayWorkloadError::Validation(format!(
                "task {} has invalid exec_hints JSON: {e}",
                self.task_id
            ))
        })?;

        Ok(ReplayTaskSpec {
            task_id: self.task_id,
            name: self.name,
            priority: self.priority,
            deadline_ns: self.deadline_ns,
            criticality: self.criticality,
            affinity,
            exec_hints,
            data_size: self.data_size,
        })
    }
}

const fn default_criticality() -> CriticalityLevel {
    CriticalityLevel::Lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_replay_workload_and_sort_jobs() {
        let input = r#"
        {
          "tasks": [
            {
              "task_id": 1,
              "name": "gpu-task",
              "priority": 2,
              "deadline_ns": 40000,
              "criticality": "Lo",
              "affinity": ["Gpu"],
              "exec_hints": [{ "device_type": "Gpu", "wcet_ns": 20000 }],
              "data_size": 1024
            },
            {
              "task_id": 0,
              "name": "cpu-task",
              "priority": 1,
              "deadline_ns": 50000,
              "criticality": "Lo",
              "affinity": ["Cpu"],
              "exec_hints": [{ "device_type": "Cpu", "wcet_ns": 30000 }],
              "data_size": 0
            }
          ],
          "jobs": [
            { "task_id": 1, "release_ns": 100000, "actual_exec_ns": 15000 },
            { "task_id": 0, "release_ns": 0, "actual_exec_ns": 20000 }
          ]
        }
        "#;

        let parsed = ReplayWorkload::from_json_str(input).expect("json replay should parse");
        let tasks = parsed.to_tasks();
        assert_eq!(tasks.len(), 2);
        assert!(
            tasks
                .iter()
                .all(|task| matches!(task.arrival, ArrivalModel::Aperiodic))
        );
        assert_eq!(parsed.jobs()[0].task_id, 0);
        assert_eq!(parsed.jobs()[1].task_id, 1);
    }

    #[test]
    fn parse_csv_replay_workload() {
        let tasks_csv = "task_id,name,priority,deadline_ns,criticality,affinity,exec_hints,data_size\n\
                         0,cpu-task,1,50000,Lo,\"[\"\"Cpu\"\"]\",\"[{\"\"device_type\"\":\"\"Cpu\"\",\"\"wcet_ns\"\":30000}]\",0\n\
                         1,gpu-task,2,40000,Lo,\"[\"\"Gpu\"\"]\",\"[{\"\"device_type\"\":\"\"Gpu\"\",\"\"wcet_ns\"\":20000}]\",1024\n";
        let jobs_csv = "task_id,release_ns,absolute_deadline_ns,actual_exec_ns\n\
                        1,100000,,15000\n\
                        0,0,,20000\n";

        let parsed = ReplayWorkload::from_csv_str(tasks_csv, jobs_csv).expect("csv replay parses");
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.jobs()[0].task_id, 0);
        assert_eq!(parsed.jobs()[1].task_id, 1);
    }

    #[test]
    fn reject_replay_with_unknown_job_task() {
        let input = r#"
        {
          "tasks": [
            {
              "task_id": 0,
              "name": "cpu-task",
              "priority": 1,
              "deadline_ns": 50000,
              "affinity": ["Cpu"],
              "exec_hints": [{ "device_type": "Cpu", "wcet_ns": 30000 }]
            }
          ],
          "jobs": [
            { "task_id": 3, "release_ns": 0, "actual_exec_ns": 10000 }
          ]
        }
        "#;

        let err = ReplayWorkload::from_json_str(input).expect_err("must fail");
        assert!(err.to_string().contains("unknown task_id 3"));
    }

    #[test]
    fn load_replay_from_json_file_fixture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/replay_sample.json");
        let replay = load_replay_json(&path).expect("fixture json should load");
        assert_eq!(replay.tasks.len(), 2);
        assert_eq!(replay.jobs()[0].task_id, 0);
    }

    #[test]
    fn load_replay_from_csv_files_fixture() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let replay = load_replay_csv(
            &base.join("replay_tasks.csv"),
            &base.join("replay_jobs.csv"),
        )
        .expect("fixture csv should load");
        assert_eq!(replay.tasks.len(), 2);
        assert_eq!(replay.jobs()[0].task_id, 0);
        assert_eq!(replay.jobs()[1].task_id, 1);
    }
}
