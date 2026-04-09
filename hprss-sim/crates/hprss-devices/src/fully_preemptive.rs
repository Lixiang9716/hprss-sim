use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision, PreemptionOutcome};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullyPreemptiveDevice;

impl DeviceBehavior for FullyPreemptiveDevice {
    fn evaluate_preemption(&self, _input: PreemptionCheckInput) -> PreemptionOutcome {
        PreemptionOutcome {
            decision: PreemptionDecision::AllowNow,
            penalty_ns: 0,
        }
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        None
    }
}
