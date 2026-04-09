//! Event system with version-based invalidation.
//!
//! Events are ordered by (time, sequence_number) in a min-heap.
//! Each event targeting a specific job carries (job_id, expected_version).
//! When processing, if job.version != expected_version, the event is stale
//! and discarded in O(1) — no need to search/remove from the heap.

use std::cmp::Ordering;

use crate::{BusId, DeviceId, EdgeTransferId, JobId, Nanos, ReevaluationTrigger, TaskId};

/// Simulation event
#[derive(Debug, Clone)]
pub struct Event {
    /// When this event fires (nanosecond timestamp)
    pub time: Nanos,
    /// Tie-breaking sequence number (lower = earlier)
    pub seq: u64,
    /// Event payload
    pub kind: EventKind,
}

/// Implement ordering for min-heap (BinaryHeap is max-heap, so we reverse)
impl Eq for Event {}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time && self.seq == other.seq
    }
}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse for min-heap: smaller time = higher priority
        other
            .time
            .cmp(&self.time)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Event payload variants
#[derive(Debug, Clone)]
pub enum EventKind {
    /// A task releases a new job
    TaskArrival { task_id: TaskId, job_id: JobId },

    /// A job completes execution on a device
    JobComplete {
        job_id: JobId,
        /// Expected version — stale if job.version != this
        expected_version: u64,
        device_id: DeviceId,
    },

    /// A preemption check point is reached (GPU kernel boundary, DSP DMA end)
    PreemptionPoint {
        device_id: DeviceId,
        job_id: JobId,
        expected_version: u64,
    },

    /// Data transfer to device completed
    TransferComplete {
        job_id: JobId,
        expected_version: u64,
        device_id: DeviceId,
    },

    /// DAG edge data transfer completed.
    EdgeTransferComplete {
        job_id: JobId,
        expected_version: u64,
        device_id: DeviceId,
        edge_id: EdgeTransferId,
    },

    /// Shared bus arbitration tick
    BusArbitration { bus_id: BusId },

    /// Deadline check for a job
    DeadlineCheck {
        job_id: JobId,
        expected_version: u64,
    },

    /// Budget overrun detected (MC: job exceeded wcet_lo)
    /// Triggers atomic criticality switch
    BudgetOverrun {
        job_id: JobId,
        expected_version: u64,
    },

    /// Scheduler reevaluation callback (periodic or event-triggered).
    SchedulerReevaluation {
        generation: u64,
        trigger: ReevaluationTrigger,
    },

    /// End of simulation
    SimulationEnd,
}

impl EventKind {
    /// Check if this event is stale given the current job version.
    /// Returns true if the event should be discarded.
    pub fn is_stale(&self, current_version: u64) -> bool {
        match self {
            Self::JobComplete {
                expected_version, ..
            }
            | Self::PreemptionPoint {
                expected_version, ..
            }
            | Self::TransferComplete {
                expected_version, ..
            }
            | Self::EdgeTransferComplete {
                expected_version, ..
            }
            | Self::BudgetOverrun {
                expected_version, ..
            } => *expected_version != current_version,

            // DeadlineCheck is never stale — the absolute deadline doesn't change
            // when a job transitions between states.
            Self::DeadlineCheck { .. } => false,

            // These events are never stale
            Self::TaskArrival { .. }
            | Self::BusArbitration { .. }
            | Self::SchedulerReevaluation { .. }
            | Self::SimulationEnd => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ordering_min_heap() {
        let e1 = Event {
            time: 100,
            seq: 0,
            kind: EventKind::SimulationEnd,
        };
        let e2 = Event {
            time: 200,
            seq: 0,
            kind: EventKind::SimulationEnd,
        };
        // For min-heap: e1 (time=100) should come first, so e1 > e2 in Ord
        assert!(e1 > e2);
    }

    #[test]
    fn event_staleness() {
        let kind = EventKind::JobComplete {
            job_id: JobId(0),
            expected_version: 3,
            device_id: DeviceId(0),
        };
        assert!(!kind.is_stale(3)); // version matches
        assert!(kind.is_stale(4)); // version mismatch → stale
    }

    #[test]
    fn arrival_never_stale() {
        let kind = EventKind::TaskArrival {
            task_id: TaskId(0),
            job_id: JobId(0),
        };
        assert!(!kind.is_stale(999));
    }

    #[test]
    fn edge_transfer_event_uses_job_version_for_staleness() {
        let kind = EventKind::EdgeTransferComplete {
            job_id: JobId(9),
            expected_version: 2,
            device_id: DeviceId(1),
            edge_id: crate::EdgeTransferId {
                dag_instance_id: crate::DagInstanceId(7),
                from_node: crate::SubTaskIdx(1),
                to_node: crate::SubTaskIdx(3),
            },
        };
        assert!(!kind.is_stale(2));
        assert!(kind.is_stale(3));
    }
}

#[cfg(test)]
mod reevaluation_tests {
    use super::*;

    #[test]
    fn scheduler_reevaluation_never_stale() {
        let kind = EventKind::SchedulerReevaluation {
            generation: 1,
            trigger: crate::ReevaluationTrigger::Periodic,
        };
        assert!(!kind.is_stale(0));
        assert!(!kind.is_stale(99));
    }
}
