//! Metrics collection and result output.
//!
//! Tracks: schedulability ratio, deadline miss rate, response time,
//! device utilization, end-to-end latency.

use hprss_types::{DeviceId, JobId, Nanos, TaskId, task::TaskChain};

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq)]
pub struct DeviceUtilization {
    pub device_id: DeviceId,
    pub busy_ns: Nanos,
    pub utilization: f64,
}

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq, Default)]
pub struct BlockingBreakdown {
    pub transfer_ns: Nanos,
    pub migration_ns: Nanos,
    pub bus_wait_ns: Nanos,
}

/// Collects simulation metrics
#[derive(Debug, Default)]
pub struct MetricsCollector {
    pub total_jobs: u64,
    pub completed_jobs: u64,
    pub deadline_misses: u64,
    pub completions: Vec<CompletionRecord>,
    pub misses: Vec<MissRecord>,
}

#[derive(Debug, Clone)]
pub struct CompletionRecord {
    pub job_id: JobId,
    pub task_id: TaskId,
    pub release_time: Nanos,
    pub completion_time: Nanos,
}

#[derive(Debug, Clone)]
pub struct MissRecord {
    pub job_id: JobId,
    pub task_id: TaskId,
    pub miss_time: Nanos,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TraceRecord {
    event: &'static str,
    time: Nanos,
    job_id: JobId,
    task_id: TaskId,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_job_release(&mut self) {
        self.total_jobs += 1;
    }

    pub fn record_completion(
        &mut self,
        job_id: JobId,
        task_id: TaskId,
        release_time: Nanos,
        time: Nanos,
    ) {
        self.completed_jobs += 1;
        self.completions.push(CompletionRecord {
            job_id,
            task_id,
            release_time,
            completion_time: time,
        });
    }

    pub fn record_deadline_miss(&mut self, job_id: JobId, task_id: TaskId, time: Nanos) {
        self.deadline_misses += 1;
        self.misses.push(MissRecord {
            job_id,
            task_id,
            miss_time: time,
        });
    }

    /// Deadline miss ratio (0.0 = perfect, 1.0 = all missed)
    pub fn miss_ratio(&self) -> f64 {
        if self.total_jobs == 0 {
            0.0
        } else {
            self.deadline_misses as f64 / self.total_jobs as f64
        }
    }

    /// Schedulability: true if zero deadline misses
    pub fn is_schedulable(&self) -> bool {
        self.deadline_misses == 0
    }

    /// Total elapsed time from earliest release to latest completion.
    pub fn makespan(&self) -> Option<Nanos> {
        let first_release = self.completions.iter().map(|c| c.release_time).min()?;
        let last_completion = self.completions.iter().map(|c| c.completion_time).max()?;
        Some(last_completion.saturating_sub(first_release))
    }

    /// Average response time across completed jobs, in nanoseconds.
    pub fn avg_response_time(&self) -> Option<f64> {
        if self.completions.is_empty() {
            return None;
        }
        let total_response: u128 = self
            .completions
            .iter()
            .map(|c| c.completion_time.saturating_sub(c.release_time) as u128)
            .sum();
        Some(total_response as f64 / self.completions.len() as f64)
    }

    /// Worst (maximum) response time across completed jobs, in nanoseconds.
    pub fn worst_response_time(&self) -> Option<Nanos> {
        self.completions
            .iter()
            .map(|c| c.completion_time.saturating_sub(c.release_time))
            .max()
    }

    /// Per-device utilization over the observed makespan window.
    pub fn per_device_utilization(
        &self,
        busy_by_device: &[(DeviceId, Nanos)],
    ) -> Vec<DeviceUtilization> {
        let makespan = self.makespan().unwrap_or(0);
        busy_by_device
            .iter()
            .map(|(device_id, busy_ns)| DeviceUtilization {
                device_id: *device_id,
                busy_ns: *busy_ns,
                utilization: if makespan == 0 {
                    0.0
                } else {
                    *busy_ns as f64 / makespan as f64
                },
            })
            .collect()
    }

    /// Serialize completion/miss timeline into JSON-lines text.
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut rows = Vec::new();
        for c in &self.completions {
            rows.push(TraceRecord {
                event: "job_complete",
                time: c.completion_time,
                job_id: c.job_id,
                task_id: c.task_id,
            });
        }
        for m in &self.misses {
            rows.push(TraceRecord {
                event: "deadline_miss",
                time: m.miss_time,
                job_id: m.job_id,
                task_id: m.task_id,
            });
        }
        rows.sort_by_key(|r| r.time);

        let mut out = String::new();
        for row in rows {
            out.push_str(&serde_json::to_string(&row)?);
            out.push('\n');
        }
        Ok(out)
    }

    pub fn write_jsonl(&self, path: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let text = self
            .to_jsonl()
            .map_err(|e| std::io::Error::other(format!("serialize trace jsonl: {e}")))?;
        std::fs::write(path, text)
    }
}

/// Compute max end-to-end reaction time for a task chain from completion records.
pub fn max_chain_reaction_time(
    chain: &TaskChain,
    completions: &[CompletionRecord],
) -> Option<Nanos> {
    if chain.tasks.is_empty() {
        return None;
    }
    let mut sorted = completions.to_vec();
    sorted.sort_by_key(|c| c.completion_time);

    let first_task = chain.tasks[0];
    let mut max_rt = None::<Nanos>;
    for start in sorted.iter().filter(|c| c.task_id == first_task) {
        let mut current_time = start.completion_time;
        let mut ok = true;
        for task_id in chain.tasks.iter().skip(1) {
            let next = sorted
                .iter()
                .find(|c| c.task_id == *task_id && c.completion_time >= current_time);
            let Some(next) = next else {
                ok = false;
                break;
            };
            current_time = next.completion_time;
        }
        if ok {
            let rt = current_time.saturating_sub(start.release_time);
            max_rt = Some(max_rt.map_or(rt, |prev| prev.max(rt)));
        }
    }
    max_rt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_basic() {
        let mut m = MetricsCollector::new();
        m.record_job_release();
        m.record_job_release();
        m.record_completion(JobId(0), TaskId(0), 0, 100);
        m.record_deadline_miss(JobId(1), TaskId(1), 200);

        assert_eq!(m.total_jobs, 2);
        assert_eq!(m.completed_jobs, 1);
        assert_eq!(m.deadline_misses, 1);
        assert!((m.miss_ratio() - 0.5).abs() < f64::EPSILON);
        assert!(!m.is_schedulable());
    }

    #[test]
    fn paper_metrics_are_computed() {
        let mut m = MetricsCollector::new();
        m.record_job_release();
        m.record_job_release();
        m.record_completion(JobId(0), TaskId(0), 10, 110);
        m.record_completion(JobId(1), TaskId(1), 20, 170);

        assert_eq!(m.makespan(), Some(160));
        assert_eq!(m.avg_response_time(), Some(125.0));
        assert_eq!(m.worst_response_time(), Some(150));
    }

    #[test]
    fn per_device_utilization_uses_makespan_window() {
        let mut m = MetricsCollector::new();
        m.record_job_release();
        m.record_job_release();
        m.record_completion(JobId(0), TaskId(0), 0, 100);
        m.record_completion(JobId(1), TaskId(1), 0, 200);

        let per_device = m.per_device_utilization(&[(DeviceId(0), 100), (DeviceId(1), 50)]);
        assert_eq!(per_device.len(), 2);
        assert!((per_device[0].utilization - 0.5).abs() < f64::EPSILON);
        assert!((per_device[1].utilization - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn trace_writer_outputs_jsonl() {
        let mut m = MetricsCollector::new();
        m.record_job_release();
        m.record_completion(JobId(2), TaskId(9), 10, 100);
        let out = m.to_jsonl().expect("jsonl should be created");
        assert!(out.contains("\"event\":\"job_complete\""));
        assert!(out.contains("\"task_id\":9"));
    }

    #[test]
    fn task_chain_reaction_time_is_computed() {
        let chain = TaskChain {
            id: hprss_types::ChainId(1),
            name: "pipeline".to_string(),
            tasks: vec![TaskId(0), TaskId(1), TaskId(2)],
            e2e_deadline: 1_000,
            metric: hprss_types::task::E2EMetric::ReactionTime,
        };
        let completions = vec![
            CompletionRecord {
                job_id: JobId(0),
                task_id: TaskId(0),
                release_time: 100,
                completion_time: 200,
            },
            CompletionRecord {
                job_id: JobId(1),
                task_id: TaskId(1),
                release_time: 120,
                completion_time: 260,
            },
            CompletionRecord {
                job_id: JobId(2),
                task_id: TaskId(2),
                release_time: 140,
                completion_time: 320,
            },
        ];
        let rt = max_chain_reaction_time(&chain, &completions).expect("reaction should exist");
        assert_eq!(rt, 220);
    }
}
