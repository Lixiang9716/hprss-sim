use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonPreemptiveDevice {
    pub reconfig_time_ns: Nanos,
}

impl DeviceBehavior for NonPreemptiveDevice {
    fn evaluate_preemption(&self, _input: PreemptionCheckInput) -> PreemptionDecision {
        PreemptionDecision::Never
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        None
    }

    fn additional_dispatch_delay_ns(&self) -> Nanos {
        self.reconfig_time_ns
    }
}
