use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FullyPreemptiveDevice;

impl DeviceBehavior for FullyPreemptiveDevice {
    fn evaluate_preemption(&self, _input: PreemptionCheckInput) -> PreemptionDecision {
        PreemptionDecision::AllowNow
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        None
    }
}
