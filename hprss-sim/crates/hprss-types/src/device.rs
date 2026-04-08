//! Device model types.
//!
//! Models the four preemption levels:
//! - CPU (FT2000): FullyPreemptive
//! - GPU (GP201): LimitedPreemptive (kernel/stream boundary)
//! - DSP (FT6678): InterruptLevel (ISR + non-preemptive DMA)
//! - FPGA (Zynq7045): NonPreemptive (PR reconfiguration)

use serde::{Deserialize, Serialize};

use crate::{DeviceId, Nanos};

/// Preemption model — the core heterogeneity axis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PreemptionModel {
    /// CPU: can preempt at any instruction boundary
    FullyPreemptive,

    /// GPU: can only preempt at stream/kernel launch boundaries
    LimitedPreemptive {
        /// Minimum non-preemptible segment duration
        granularity_ns: Nanos,
    },

    /// DSP: interrupt-level preemption, but DMA regions are non-preemptible
    InterruptLevel {
        isr_overhead_ns: Nanos,
        /// Duration of non-preemptible DMA regions
        dma_non_preemptive_ns: Nanos,
    },

    /// FPGA: entire execution is non-preemptible; task switch = reconfiguration
    NonPreemptive {
        /// Partial reconfiguration time
        reconfig_time_ns: Nanos,
    },
}

impl PreemptionModel {
    /// Maximum blocking time this device can cause to a higher-priority task.
    /// This is the key parameter for Response Time Analysis (RTA).
    pub fn max_blocking(&self) -> Nanos {
        match self {
            Self::FullyPreemptive => 0,
            Self::LimitedPreemptive { granularity_ns } => *granularity_ns,
            Self::InterruptLevel {
                dma_non_preemptive_ns,
                ..
            } => *dma_non_preemptive_ns,
            Self::NonPreemptive { .. } => Nanos::MAX, // effectively infinite until complete
        }
    }

    /// Whether this device supports any form of preemption
    pub fn is_preemptible(&self) -> bool {
        !matches!(self, Self::NonPreemptive { .. })
    }
}

/// Multi-core CPU scheduling policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MultiCorePolicy {
    /// Tasks statically bound to cores (no migration)
    Partitioned,
    /// Global ready queue, tasks can run on any core
    Global,
    /// k cores per cluster, global within cluster
    Clustered { k: u32 },
}

/// Configuration for a single device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub id: DeviceId,
    pub name: String,
    pub device_type: crate::task::DeviceType,
    pub cores: u32,
    pub preemption: PreemptionModel,
    /// Context switch overhead (same-device task switch)
    pub context_switch_ns: Nanos,
    /// Speed factor relative to baseline CPU (for heterogeneous WCET scaling)
    pub speed_factor: f64,
    /// Multi-core policy (only meaningful for CPU with cores > 1)
    pub multicore_policy: Option<MultiCorePolicy>,
    /// Power consumption in watts (optional, for energy-aware scheduling)
    pub power_watts: Option<f64>,
}

/// Bus arbitration strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BusArbitration {
    /// Dedicated link, no contention
    Dedicated,
    /// Round-robin arbitration
    RoundRobin,
    /// Priority-based arbitration
    PriorityBased,
    /// Time-division multiple access (most deterministic)
    Tdma { slot_ns: Nanos },
}

/// Interconnect link between two devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterconnectConfig {
    pub from: DeviceId,
    pub to: DeviceId,
    pub latency_ns: Nanos,
    /// Bandwidth in bytes per nanosecond (e.g., 16 GB/s = 16 bytes/ns)
    pub bandwidth_bytes_per_ns: f64,
    /// Shared bus this link belongs to (None = dedicated)
    pub shared_bus: Option<crate::BusId>,
    pub arbitration: BusArbitration,
}

/// Shared bus definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedBusConfig {
    pub id: crate::BusId,
    pub name: String,
    pub total_bandwidth_bytes_per_ns: f64,
    pub arbitration: BusArbitration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preemption_blocking() {
        let cpu = PreemptionModel::FullyPreemptive;
        assert_eq!(cpu.max_blocking(), 0);
        assert!(cpu.is_preemptible());

        let gpu = PreemptionModel::LimitedPreemptive {
            granularity_ns: 50_000,
        };
        assert_eq!(gpu.max_blocking(), 50_000);
        assert!(gpu.is_preemptible());

        let fpga = PreemptionModel::NonPreemptive {
            reconfig_time_ns: 2_000_000,
        };
        assert_eq!(fpga.max_blocking(), Nanos::MAX);
        assert!(!fpga.is_preemptible());
    }
}
