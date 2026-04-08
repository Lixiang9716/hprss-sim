//! Scheduler trait and action types.
//!
//! The Scheduler trait is the core interface that both the simulator and
//! future real hardware scheduler will implement against. Schedulers observe
//! an immutable SchedulerView and return a list of Actions.

use crate::{
    CriticalityLevel, DeviceId, Job, JobId, Nanos, TaskId, device::DeviceConfig, task::Task,
};

/// Immutable snapshot of observable simulation state.
///
/// The scheduler sees device states, queued jobs, elapsed time —
/// but NOT actual_exec_time or remaining_ns (hidden, just like real hardware).
#[derive(Debug)]
pub struct SchedulerView<'a> {
    /// Current simulation time
    pub now: Nanos,
    /// All devices and their configurations
    pub devices: &'a [DeviceConfig],
    /// Jobs currently running on each device: device_id → Option<job_info>
    pub running_jobs: &'a [(DeviceId, Option<RunningJobInfo>)],
    /// Jobs in ready queues per device: device_id → [job_info]
    pub ready_queues: &'a [(DeviceId, Vec<QueuedJobInfo>)],
    /// Current mixed-criticality level
    pub criticality_level: CriticalityLevel,
}

/// Information about a running job visible to the scheduler.
/// Note: actual_exec_time and remaining time are NOT exposed.
#[derive(Debug, Clone)]
pub struct RunningJobInfo {
    pub job_id: JobId,
    pub task_id: TaskId,
    pub priority: u32,
    pub release_time: Nanos,
    pub absolute_deadline: Nanos,
    pub criticality: CriticalityLevel,
    /// How long this job has been executing (observable via timers)
    pub elapsed_ns: Nanos,
}

/// Information about a queued job visible to the scheduler
#[derive(Debug, Clone)]
pub struct QueuedJobInfo {
    pub job_id: JobId,
    pub task_id: TaskId,
    pub priority: u32,
    pub release_time: Nanos,
    pub absolute_deadline: Nanos,
    pub criticality: CriticalityLevel,
}

/// Scheduling action returned by the scheduler
#[derive(Debug, Clone)]
pub enum Action {
    /// Dispatch a job to a device for execution
    Dispatch { job_id: JobId, device_id: DeviceId },

    /// Preempt the victim job, replacing it with the new job
    Preempt {
        victim: JobId,
        by: JobId,
        device_id: DeviceId,
    },

    /// Migrate a job from one device to another (3-phase protocol)
    Migrate {
        job_id: JobId,
        from: DeviceId,
        to: DeviceId,
    },

    /// Enqueue a job into a device's ready queue
    Enqueue { job_id: JobId, device_id: DeviceId },

    /// Drop a job (MC mode switch: discard Lo-criticality tasks)
    DropJob { job_id: JobId },

    /// No action needed
    NoOp,
}

/// What causes a scheduler reevaluation callback to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReevaluationTrigger {
    /// Periodic scheduler tick.
    Periodic,
    /// Reevaluation requested after a task/job arrival-like event.
    TaskArrival,
    /// Reevaluation requested after a job completion.
    JobComplete,
    /// Reevaluation requested after a preemption-point callback.
    PreemptionPoint,
    /// Reevaluation requested after a transfer completion.
    TransferComplete,
    /// Reevaluation requested after a mixed-criticality mode switch.
    CriticalityChange,
}

/// Scheduler policy that controls reevaluation callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReevaluationPolicy {
    /// Reevaluation callbacks are disabled.
    Disabled,
    /// Fixed periodic reevaluation tick.
    Periodic { interval_ns: Nanos },
    /// Event-triggered reevaluation with debounce interval.
    Triggered { min_interval_ns: Nanos },
    /// Support both periodic and event-triggered reevaluation.
    ///
    /// Semantics:
    /// - periodic ticks are fixed-cadence (`interval_ns`, `2*interval_ns`, ... from t=0)
    /// - periodic ticks are NOT reset by triggered callbacks
    /// - triggered callbacks are debounced independently by `min_interval_ns`
    /// - periodic and triggered reevaluations can both fire in one simulation run
    Hybrid {
        interval_ns: Nanos,
        min_interval_ns: Nanos,
    },
}

impl ReevaluationPolicy {
    /// Returns periodic tick interval if this policy has periodic reevaluation enabled.
    ///
    /// Interval values of 0 are treated as disabled for safety.
    pub fn periodic_interval(self) -> Option<Nanos> {
        match self {
            Self::Periodic { interval_ns } | Self::Hybrid { interval_ns, .. }
                if interval_ns > 0 =>
            {
                Some(interval_ns)
            }
            _ => None,
        }
    }

    /// Returns triggered debounce interval if this policy has triggered reevaluation enabled.
    ///
    /// Interval values of 0 are treated as disabled for safety.
    pub fn triggered_min_interval(self) -> Option<Nanos> {
        match self {
            Self::Triggered { min_interval_ns }
            | Self::Hybrid {
                min_interval_ns, ..
            } if min_interval_ns > 0 => Some(min_interval_ns),
            _ => None,
        }
    }
}

/// The core scheduler interface.
///
/// Implementations must be deterministic given the same SchedulerView.
/// This trait is shared between simulator and future real-time hardware.
pub trait Scheduler: Send {
    /// Human-readable name of this scheduling algorithm
    fn name(&self) -> &str;

    /// A new job has been released (periodic timer / sporadic event / DAG predecessor done)
    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action>;

    /// A job completed execution on a device
    fn on_job_complete(
        &mut self,
        job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action>;

    /// A preemption checkpoint was reached (GPU kernel boundary / DSP DMA end)
    fn on_preemption_point(
        &mut self,
        device_id: DeviceId,
        running_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action>;

    /// Mixed-criticality mode change (Lo→Hi or Hi→Lo)
    fn on_criticality_change(
        &mut self,
        new_level: CriticalityLevel,
        trigger_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action>;

    /// A device is currently idle and can accept an eligible ready job.
    ///
    /// Used for deterministic redispatch after events such as mixed-criticality
    /// mode switches where jobs may be dropped from running state.
    fn on_device_idle(&mut self, _device_id: DeviceId, _view: &SchedulerView<'_>) -> Vec<Action> {
        vec![Action::NoOp]
    }

    /// Reevaluation policy used by the engine to safely schedule callbacks.
    ///
    /// Default is disabled for backward compatibility.
    fn reevaluation_policy(&self) -> ReevaluationPolicy {
        ReevaluationPolicy::Disabled
    }

    /// Callback for periodic/event-triggered reevaluation.
    ///
    /// Default is no-op for backward compatibility.
    fn on_reevaluation(
        &mut self,
        _trigger: ReevaluationTrigger,
        _view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        vec![Action::NoOp]
    }
}
