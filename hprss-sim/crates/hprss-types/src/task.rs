//! Task model definitions.
//!
//! Supports periodic, sporadic, aperiodic tasks with deterministic or
//! stochastic execution times, and DAG task structures.

use serde::{Deserialize, Serialize};

use crate::{DeviceId, Nanos, TaskId};

/// How a task arrives (release pattern)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArrivalModel {
    /// Strictly periodic: released every `period` nanoseconds
    Periodic { period: Nanos },

    /// Sporadic: minimum inter-arrival time, actual arrival may be later
    Sporadic { min_inter_arrival: Nanos },

    /// Aperiodic: arrives at externally specified times (event-driven)
    Aperiodic,
}

/// How execution time is modeled
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionTimeModel {
    /// Fixed WCET (classical RT analysis)
    Deterministic { wcet: Nanos },

    /// Execution time drawn from a distribution at each job release
    Stochastic {
        bcet: Nanos,
        wcet: Nanos,
        /// "uniform" | "normal" | "lognormal"
        distribution: String,
    },

    /// Mixed-criticality: two WCET bounds
    MixedCriticality { wcet_lo: Nanos, wcet_hi: Nanos },
}

impl ExecutionTimeModel {
    /// Get the worst-case value (used for schedulability analysis)
    pub fn wcet(&self) -> Nanos {
        match self {
            Self::Deterministic { wcet } => *wcet,
            Self::Stochastic { wcet, .. } => *wcet,
            Self::MixedCriticality { wcet_hi, .. } => *wcet_hi,
        }
    }

    /// Get the best-case value
    pub fn bcet(&self) -> Nanos {
        match self {
            Self::Deterministic { wcet } => *wcet,
            Self::Stochastic { bcet, .. } => *bcet,
            Self::MixedCriticality { wcet_lo, .. } => *wcet_lo,
        }
    }
}

/// Mixed-criticality level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CriticalityLevel {
    /// Low criticality (can be dropped during mode switch)
    Lo,
    /// High criticality (must always meet deadlines)
    Hi,
}

/// Device type for affinity specification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceType {
    Cpu,
    Gpu,
    Dsp,
    Fpga,
}

/// A periodic/sporadic real-time task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub name: String,
    pub priority: u32,
    pub arrival: ArrivalModel,
    /// Relative deadline (nanoseconds from release)
    pub deadline: Nanos,
    pub criticality: CriticalityLevel,
    /// Execution time model per device type
    /// Key: DeviceType → ExecutionTimeModel
    pub exec_times: Vec<(DeviceType, ExecutionTimeModel)>,
    /// Which device types this task can execute on
    pub affinity: Vec<DeviceType>,
    /// Input data size in bytes (for transfer overhead)
    pub data_size: u64,
}

impl Task {
    /// Get WCET on a specific device type, if supported
    pub fn wcet_on(&self, device: DeviceType) -> Option<Nanos> {
        self.exec_times
            .iter()
            .find(|(dt, _)| *dt == device)
            .map(|(_, model)| model.wcet())
    }

    /// Get the period (only valid for periodic tasks)
    pub fn period(&self) -> Option<Nanos> {
        match &self.arrival {
            ArrivalModel::Periodic { period } => Some(*period),
            ArrivalModel::Sporadic { min_inter_arrival } => Some(*min_inter_arrival),
            ArrivalModel::Aperiodic => None,
        }
    }
}

/// A sub-task within a DAG
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    pub index: usize,
    pub exec_times: Vec<(DeviceType, ExecutionTimeModel)>,
    pub affinity: Vec<DeviceType>,
    /// Data dependency: (predecessor index, transfer size in bytes)
    pub data_deps: Vec<(usize, u64)>,
}

/// A DAG (Directed Acyclic Graph) task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagTask {
    pub id: TaskId,
    pub name: String,
    pub arrival: ArrivalModel,
    /// End-to-end deadline for the entire DAG
    pub deadline: Nanos,
    pub criticality: CriticalityLevel,
    pub nodes: Vec<SubTask>,
    /// Edges: (predecessor_index, successor_index)
    pub edges: Vec<(usize, usize)>,
}

/// End-to-end task chain (cause-effect chain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskChain {
    pub id: crate::ChainId,
    pub name: String,
    /// Ordered task sequence: sensor → process → actuate
    pub tasks: Vec<TaskId>,
    /// End-to-end deadline
    pub e2e_deadline: Nanos,
    pub metric: E2EMetric,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum E2EMetric {
    ReactionTime,
    DataAge,
    Both,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_wcet_lookup() {
        let task = Task {
            id: TaskId(0),
            name: "test".into(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 10_000_000 },
            deadline: 10_000_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![
                (DeviceType::Cpu, ExecutionTimeModel::Deterministic { wcet: 1_000_000 }),
                (DeviceType::Gpu, ExecutionTimeModel::Deterministic { wcet: 200_000 }),
            ],
            affinity: vec![DeviceType::Cpu, DeviceType::Gpu],
            data_size: 4096,
        };

        assert_eq!(task.wcet_on(DeviceType::Cpu), Some(1_000_000));
        assert_eq!(task.wcet_on(DeviceType::Gpu), Some(200_000));
        assert_eq!(task.wcet_on(DeviceType::Dsp), None);
        assert_eq!(task.period(), Some(10_000_000));
    }
}
