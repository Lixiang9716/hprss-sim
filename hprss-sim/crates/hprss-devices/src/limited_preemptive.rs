use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision, PreemptionOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedPreemptiveDevice {
    pub granularity_ns: Nanos,
}

impl DeviceBehavior for LimitedPreemptiveDevice {
    fn evaluate_preemption(&self, input: PreemptionCheckInput) -> PreemptionOutcome {
        let decision = if input.at_preemption_point {
            PreemptionDecision::AllowNow
        } else {
            PreemptionDecision::DeferUntilPreemptionPoint
        };
        PreemptionOutcome {
            decision,
            penalty_ns: 0,
        }
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        Some(self.granularity_ns)
    }
}
