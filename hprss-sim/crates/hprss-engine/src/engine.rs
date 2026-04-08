//! The DES simulation engine.

use std::collections::BinaryHeap;

use hprss_metrics::MetricsCollector;
use hprss_types::{
    Action, CriticalityLevel, DeviceId, Event, EventKind, Job, JobId, JobState, Nanos, Scheduler,
    TaskId,
};

/// Simulation engine configuration
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Total simulation duration in nanoseconds
    pub duration_ns: Nanos,
    /// Random seed for reproducibility
    pub seed: u64,
}

/// The core simulation engine
pub struct SimEngine {
    /// Configuration
    config: SimConfig,
    /// Current simulation time
    now: Nanos,
    /// Event queue (min-heap via reversed Ord)
    event_queue: BinaryHeap<Event>,
    /// Monotonic sequence counter for tie-breaking
    seq_counter: u64,
    /// All active jobs indexed by JobId
    jobs: Vec<Option<Job>>,
    /// Next job ID to allocate
    next_job_id: u64,
    /// Current mixed-criticality level
    criticality_level: CriticalityLevel,
    /// Metrics collector
    metrics: MetricsCollector,
    /// Events processed counter
    events_processed: u64,
}

impl SimEngine {
    pub fn new(config: SimConfig) -> Self {
        Self {
            config,
            now: 0,
            event_queue: BinaryHeap::new(),
            seq_counter: 0,
            jobs: Vec::new(),
            next_job_id: 0,
            criticality_level: CriticalityLevel::Lo,
            metrics: MetricsCollector::new(),
            events_processed: 0,
        }
    }

    /// Schedule a new event
    pub fn schedule_event(&mut self, time: Nanos, kind: EventKind) {
        let seq = self.seq_counter;
        self.seq_counter += 1;
        self.event_queue.push(Event {
            time,
            seq,
            kind,
        });
    }

    /// Allocate a new job
    pub fn create_job(
        &mut self,
        task_id: TaskId,
        release_time: Nanos,
        absolute_deadline: Nanos,
        actual_exec_ns: Nanos,
        priority: u32,
    ) -> JobId {
        let id = JobId(self.next_job_id);
        self.next_job_id += 1;

        let job = Job::new(id, task_id, release_time, absolute_deadline, actual_exec_ns, priority);

        // Grow vector if needed
        let idx = id.0 as usize;
        if idx >= self.jobs.len() {
            self.jobs.resize_with(idx + 1, || None);
        }
        self.jobs[idx] = Some(job);
        id
    }

    /// Look up a job by ID
    pub fn get_job(&self, id: JobId) -> Option<&Job> {
        self.jobs.get(id.0 as usize).and_then(|opt| opt.as_ref())
    }

    /// Look up a job mutably
    pub fn get_job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs
            .get_mut(id.0 as usize)
            .and_then(|opt| opt.as_mut())
    }

    /// Run the main simulation loop
    pub fn run(&mut self, scheduler: &mut dyn Scheduler) {
        // Schedule simulation end
        self.schedule_event(self.config.duration_ns, EventKind::SimulationEnd);

        tracing::info!(
            "Simulation starting: duration={}ns, seed={}",
            self.config.duration_ns,
            self.config.seed
        );

        while let Some(event) = self.event_queue.pop() {
            // Advance simulation clock
            debug_assert!(
                event.time >= self.now,
                "Time went backwards: {} < {}",
                event.time,
                self.now
            );
            self.now = event.time;

            // Check for stale events (version-based invalidation)
            if let Some(job_id) = self.event_job_id(&event.kind) {
                if let Some(job) = self.get_job(job_id) {
                    if event.kind.is_stale(job.version) {
                        tracing::trace!(
                            "Discarding stale event {:?} for job {:?}",
                            event.kind,
                            job_id
                        );
                        continue;
                    }
                }
            }

            self.events_processed += 1;

            match &event.kind {
                EventKind::SimulationEnd => {
                    tracing::info!(
                        "Simulation ended at t={}ns, events_processed={}",
                        self.now,
                        self.events_processed
                    );
                    break;
                }
                EventKind::TaskArrival { task_id, job_id } => {
                    tracing::trace!("t={}: TaskArrival task={:?} job={:?}", self.now, task_id, job_id);
                    // TODO: invoke scheduler.on_job_arrival() and process actions
                }
                EventKind::JobComplete {
                    job_id, device_id, ..
                } => {
                    tracing::trace!("t={}: JobComplete job={:?} dev={:?}", self.now, job_id, device_id);
                    if let Some(job) = self.get_job_mut(*job_id) {
                        job.transition(JobState::Completed);
                        self.metrics.record_completion(*job_id, self.now);
                    }
                    // TODO: invoke scheduler.on_job_complete() and process actions
                }
                EventKind::PreemptionPoint {
                    device_id, job_id, ..
                } => {
                    tracing::trace!(
                        "t={}: PreemptionPoint dev={:?} job={:?}",
                        self.now, device_id, job_id
                    );
                    // TODO: invoke scheduler.on_preemption_point()
                }
                EventKind::TransferComplete {
                    job_id, device_id, ..
                } => {
                    tracing::trace!(
                        "t={}: TransferComplete job={:?} dev={:?}",
                        self.now, job_id, device_id
                    );
                    // TODO: transition job to Running, schedule JobComplete
                }
                EventKind::BusArbitration { bus_id } => {
                    tracing::trace!("t={}: BusArbitration bus={:?}", self.now, bus_id);
                    // TODO: arbitrate pending transfers
                }
                EventKind::DeadlineCheck { job_id, .. } => {
                    if let Some(job) = self.get_job(*job_id) {
                        if job.has_missed_deadline(self.now) {
                            tracing::warn!(
                                "t={}: DEADLINE MISS job={:?} deadline={}",
                                self.now, job_id, job.absolute_deadline
                            );
                            self.metrics.record_deadline_miss(*job_id, self.now);
                        }
                    }
                }
                EventKind::BudgetOverrun { job_id, .. } => {
                    tracing::warn!("t={}: BudgetOverrun job={:?}", self.now, job_id);
                    // TODO: atomic MC mode switch
                }
            }
        }
    }

    /// Extract the job ID from an event kind (if applicable)
    fn event_job_id(&self, kind: &EventKind) -> Option<JobId> {
        match kind {
            EventKind::JobComplete { job_id, .. }
            | EventKind::PreemptionPoint { job_id, .. }
            | EventKind::TransferComplete { job_id, .. }
            | EventKind::DeadlineCheck { job_id, .. }
            | EventKind::BudgetOverrun { job_id, .. } => Some(*job_id),
            _ => None,
        }
    }

    /// Get current simulation time
    pub fn now(&self) -> Nanos {
        self.now
    }

    /// Get total events processed
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }

    /// Get metrics collector reference
    pub fn metrics(&self) -> &MetricsCollector {
        &self.metrics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_basic_lifecycle() {
        let config = SimConfig {
            duration_ns: 1_000_000, // 1ms
            seed: 42,
        };
        let mut engine = SimEngine::new(config);

        // Create a job
        let job_id = engine.create_job(TaskId(0), 0, 500_000, 100_000, 1);
        assert!(engine.get_job(job_id).is_some());
        assert_eq!(engine.get_job(job_id).unwrap().state, JobState::Released);
    }

    #[test]
    fn event_queue_ordering() {
        let config = SimConfig {
            duration_ns: 1_000_000,
            seed: 42,
        };
        let mut engine = SimEngine::new(config);

        // Schedule events out of order
        engine.schedule_event(
            300,
            EventKind::TaskArrival {
                task_id: TaskId(2),
                job_id: JobId(2),
            },
        );
        engine.schedule_event(
            100,
            EventKind::TaskArrival {
                task_id: TaskId(0),
                job_id: JobId(0),
            },
        );
        engine.schedule_event(
            200,
            EventKind::TaskArrival {
                task_id: TaskId(1),
                job_id: JobId(1),
            },
        );

        // Pop should return in time order
        let e1 = engine.event_queue.pop().unwrap();
        let e2 = engine.event_queue.pop().unwrap();
        let e3 = engine.event_queue.pop().unwrap();
        assert_eq!(e1.time, 100);
        assert_eq!(e2.time, 200);
        assert_eq!(e3.time, 300);
    }
}
