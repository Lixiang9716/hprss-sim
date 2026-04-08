use hprss_types::Nanos;

use crate::{DeviceBehavior, PreemptionCheckInput, PreemptionDecision, PreemptionOutcome};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonPreemptiveDevice {
    pub reconfig_time_ns: Nanos,
}

impl DeviceBehavior for NonPreemptiveDevice {
    fn evaluate_preemption(&self, _input: PreemptionCheckInput) -> PreemptionOutcome {
        PreemptionOutcome {
            decision: PreemptionDecision::Never,
            penalty_ns: 0,
        }
    }

    fn preemption_point_interval_ns(&self) -> Option<Nanos> {
        None
    }

    fn additional_dispatch_delay_ns(&self) -> Nanos {
        self.reconfig_time_ns
    }
}
