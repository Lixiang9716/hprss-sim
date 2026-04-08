use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptLevelDevice {
    pub isr_overhead_ns: Nanos,
    pub dma_non_preemptive_ns: Nanos,
}

impl DeviceBehavior for InterruptLevelDevice {
    fn evaluate_preemption(&self, input: PreemptionCheckInput) -> PreemptionDecision {
        if input.at_preemption_point {
            PreemptionDecision::AllowNow
        } else {
            PreemptionDecision::DeferUntilPreemptionPoint
        }
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        Some(self.dma_non_preemptive_ns)
    }
}
