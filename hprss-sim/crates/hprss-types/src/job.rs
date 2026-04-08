//! Job runtime state and state machine.
//!
//! A Job is a single instance (release) of a Task. Each periodic release
//! creates a new Job with a unique JobId. The Job tracks execution progress,
//! state transitions, and carries a version number for event invalidation.

use serde::{Deserialize, Serialize};

use crate::{DeviceId, JobId, Nanos, TaskId};

/// Job state machine:
/// ```text
/// Released → Ready → Transferring → Running → Completed
///                  ↗               ↘       ↗
///            Suspended ←──────── (preempt)
///                  ↘
///              Migrating → Ready (on new device)
///
/// Any state → Dropped (MC mode switch, Lo-crit task)
/// Any state with deadline miss → DeadlineMissed
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    /// Just released, not yet in any device queue
    Released,
    /// In a device's ready queue, waiting to execute
    Ready,
    /// Data being transferred to target device
    Transferring,
    /// Currently executing on a device
    Running,
    /// Preempted, waiting to resume
    Suspended,
    /// Being migrated between devices (3-phase: preempt → transfer → dispatch)
    Migrating,
    /// Execution completed successfully
    Completed,
    /// Dropped during mixed-criticality mode switch
    Dropped,
    /// Missed its deadline
    DeadlineMissed,
}

impl JobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Dropped | Self::DeadlineMissed
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Transferring | Self::Running | Self::Suspended | Self::Migrating
        )
    }
}

/// Runtime job instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub task_id: TaskId,
    pub state: JobState,

    /// Monotonically increasing version. Incremented on every state change.
    /// Events carry (job_id, version) — stale events are discarded O(1).
    pub version: u64,

    /// Absolute release time
    pub release_time: Nanos,
    /// Absolute deadline
    pub absolute_deadline: Nanos,

    /// Actual execution time for this instance (sampled from distribution at release)
    /// Hidden from the scheduler — only the engine knows this.
    pub actual_exec_ns: Nanos,

    /// How much execution time has been consumed so far
    pub executed_ns: Nanos,

    /// Which device this job is assigned to (None if not yet mapped)
    pub assigned_device: Option<DeviceId>,

    /// Time when execution started on current device (for computing progress)
    pub exec_start_time: Option<Nanos>,

    /// Priority (may be dynamic, e.g., EDF uses absolute deadline)
    pub effective_priority: u32,
}

impl Job {
    pub fn new(
        id: JobId,
        task_id: TaskId,
        release_time: Nanos,
        absolute_deadline: Nanos,
        actual_exec_ns: Nanos,
        priority: u32,
    ) -> Self {
        Self {
            id,
            task_id,
            state: JobState::Released,
            version: 0,
            release_time,
            absolute_deadline,
            actual_exec_ns,
            executed_ns: 0,
            assigned_device: None,
            exec_start_time: None,
            effective_priority: priority,
        }
    }

    /// Remaining execution time (only the engine should use this)
    pub fn remaining_ns(&self) -> Nanos {
        self.actual_exec_ns.saturating_sub(self.executed_ns)
    }

    /// Transition to a new state, incrementing the version counter
    pub fn transition(&mut self, new_state: JobState) {
        self.state = new_state;
        self.version += 1;
    }

    /// Record execution progress: add elapsed time since last checkpoint
    pub fn record_progress(&mut self, elapsed_ns: Nanos) {
        self.executed_ns += elapsed_ns;
    }

    /// Check if this job has missed its deadline at the given time
    pub fn has_missed_deadline(&self, now: Nanos) -> bool {
        now > self.absolute_deadline && !self.state.is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lifecycle() {
        let mut job = Job::new(
            JobId(0),
            TaskId(0),
            0,           // release at t=0
            10_000_000,  // deadline at 10ms
            3_000_000,   // actual exec = 3ms
            1,
        );

        assert_eq!(job.state, JobState::Released);
        assert_eq!(job.version, 0);
        assert_eq!(job.remaining_ns(), 3_000_000);

        job.transition(JobState::Ready);
        assert_eq!(job.version, 1);

        job.transition(JobState::Running);
        assert_eq!(job.version, 2);

        job.record_progress(1_000_000); // ran for 1ms
        assert_eq!(job.executed_ns, 1_000_000);
        assert_eq!(job.remaining_ns(), 2_000_000);

        // Preempted
        job.transition(JobState::Suspended);
        assert_eq!(job.version, 3);

        // Resume
        job.transition(JobState::Running);
        job.record_progress(2_000_000); // ran for 2ms more
        assert_eq!(job.remaining_ns(), 0);

        job.transition(JobState::Completed);
        assert!(job.state.is_terminal());
    }

    #[test]
    fn deadline_miss_detection() {
        let job = Job::new(JobId(0), TaskId(0), 0, 10_000_000, 3_000_000, 1);
        assert!(!job.has_missed_deadline(5_000_000));
        assert!(job.has_missed_deadline(10_000_001));
    }
}
