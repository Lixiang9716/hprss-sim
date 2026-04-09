//! Virtual device behavior models aligned to simulator preemption semantics.

mod fully_preemptive;
mod interrupt_level;
mod limited_preemptive;
mod non_preemptive;

pub use fully_preemptive::FullyPreemptiveDevice;
pub use interrupt_level::InterruptLevelDevice;
pub use limited_preemptive::LimitedPreemptiveDevice;
pub use non_preemptive::NonPreemptiveDevice;

use hprss_types::{
    Nanos,
    device::{DeviceConfig, PreemptionModel},
};

trait DeviceBehavior {
    fn evaluate_preemption(&self, input: PreemptionCheckInput) -> PreemptionOutcome;
    fn preemption_point_interval_ns(&self) -> Option<Nanos>;
    fn additional_dispatch_delay_ns(&self) -> Nanos {
        0
    }
}

/// Input to evaluate timing of a dispatch decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchTimingInput {
    pub now_ns: Nanos,
    pub context_switch_ns: Nanos,
    pub pre_dispatch_penalty_ns: Nanos,
    pub remaining_exec_wall_ns: Nanos,
}

/// Timing behavior observed after dispatching a job to a device model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchTiming {
    pub completion_time_ns: Nanos,
    pub next_preemption_point_ns: Option<Nanos>,
    /// Additional model-specific delay (e.g. FPGA reconfiguration) that is
    /// included when computing `completion_time_ns` / `next_preemption_point_ns`.
    pub additional_dispatch_delay_ns: Nanos,
}

/// Input to evaluate if a requested preemption can be applied at this instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreemptionCheckInput {
    pub at_preemption_point: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreemptionOutcome {
    pub decision: PreemptionDecision,
    pub penalty_ns: Nanos,
}

impl PreemptionOutcome {
    pub fn allows_preemption_now(self) -> bool {
        self.decision.allows_preemption_now()
    }
}

/// Decision returned by virtual devices for preemption requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreemptionDecision {
    AllowNow,
    DeferUntilPreemptionPoint,
    Never,
}

impl PreemptionDecision {
    pub fn allows_preemption_now(self) -> bool {
        matches!(self, Self::AllowNow)
    }
}

/// Concrete virtual device behavior model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualDeviceModel {
    FullyPreemptive(FullyPreemptiveDevice),
    LimitedPreemptive(LimitedPreemptiveDevice),
    InterruptLevel(InterruptLevelDevice),
    NonPreemptive(NonPreemptiveDevice),
}

impl VirtualDeviceModel {
    pub fn from_device(device: &DeviceConfig) -> Self {
        Self::from_preemption(device.preemption.clone())
    }

    pub fn from_preemption(preemption: PreemptionModel) -> Self {
        match preemption {
            PreemptionModel::FullyPreemptive => Self::FullyPreemptive(FullyPreemptiveDevice),
            PreemptionModel::LimitedPreemptive { granularity_ns } => {
                Self::LimitedPreemptive(LimitedPreemptiveDevice { granularity_ns })
            }
            PreemptionModel::InterruptLevel {
                isr_overhead_ns,
                dma_non_preemptive_ns,
            } => Self::InterruptLevel(InterruptLevelDevice {
                isr_overhead_ns,
                dma_non_preemptive_ns,
            }),
            PreemptionModel::NonPreemptive { reconfig_time_ns } => {
                Self::NonPreemptive(NonPreemptiveDevice { reconfig_time_ns })
            }
        }
    }

    pub fn evaluate_dispatch_timing(&self, input: DispatchTimingInput) -> DispatchTiming {
        let additional_dispatch_delay_ns = self.behavior().additional_dispatch_delay_ns();
        let execution_start_ns = input
            .now_ns
            .saturating_add(input.context_switch_ns)
            .saturating_add(input.pre_dispatch_penalty_ns)
            .saturating_add(additional_dispatch_delay_ns);
        let completion_time_ns = execution_start_ns.saturating_add(input.remaining_exec_wall_ns);
        let next_preemption_point_ns = self
            .behavior()
            .preemption_point_interval_ns()
            .map(|interval| execution_start_ns.saturating_add(interval));
        DispatchTiming {
            completion_time_ns,
            next_preemption_point_ns,
            additional_dispatch_delay_ns,
        }
    }

    pub fn evaluate_preemption(&self, input: PreemptionCheckInput) -> PreemptionOutcome {
        self.behavior().evaluate_preemption(input)
    }

    pub fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        self.behavior().preemption_point_interval_ns()
    }

    fn behavior(&self) -> &dyn DeviceBehavior {
        match self {
            Self::FullyPreemptive(model) => model,
            Self::LimitedPreemptive(model) => model,
            Self::InterruptLevel(model) => model,
            Self::NonPreemptive(model) => model,
        }
    }
}
