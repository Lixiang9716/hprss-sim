use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision, PreemptionOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptLevelDevice {
    pub isr_overhead_ns: Nanos,
    pub dma_non_preemptive_ns: Nanos,
}

impl DeviceBehavior for InterruptLevelDevice {
    fn evaluate_preemption(&self, input: PreemptionCheckInput) -> PreemptionOutcome {
        if input.at_preemption_point {
            PreemptionOutcome {
                decision: PreemptionDecision::AllowNow,
                penalty_ns: self.isr_overhead_ns,
            }
        } else {
            PreemptionOutcome {
                decision: PreemptionDecision::DeferUntilPreemptionPoint,
                penalty_ns: 0,
            }
        }
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        Some(self.dma_non_preemptive_ns)
    }
}
