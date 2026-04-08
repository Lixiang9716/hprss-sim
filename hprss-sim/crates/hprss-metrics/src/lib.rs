//! Metrics collection and result output.
//!
//! Tracks: schedulability ratio, deadline miss rate, response time,
//! device utilization, end-to-end latency.

use hprss_types::{JobId, Nanos};

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
    pub completion_time: Nanos,
}

#[derive(Debug, Clone)]
pub struct MissRecord {
    pub job_id: JobId,
    pub miss_time: Nanos,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_job_release(&mut self) {
        self.total_jobs += 1;
    }

    pub fn record_completion(&mut self, job_id: JobId, time: Nanos) {
        self.completed_jobs += 1;
        self.completions.push(CompletionRecord {
            job_id,
            completion_time: time,
        });
    }

    pub fn record_deadline_miss(&mut self, job_id: JobId, time: Nanos) {
        self.deadline_misses += 1;
        self.misses.push(MissRecord {
            job_id,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_basic() {
        let mut m = MetricsCollector::new();
        m.record_job_release();
        m.record_job_release();
        m.record_completion(JobId(0), 100);
        m.record_deadline_miss(JobId(1), 200);

        assert_eq!(m.total_jobs, 2);
        assert_eq!(m.completed_jobs, 1);
        assert_eq!(m.deadline_misses, 1);
        assert!((m.miss_ratio() - 0.5).abs() < f64::EPSILON);
        assert!(!m.is_schedulable());
    }
}
