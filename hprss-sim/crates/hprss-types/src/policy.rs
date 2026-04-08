//! Lightweight policy traits for device/runtime behavior.

use crate::{
    Nanos,
    device::PreemptionModel,
    task::{CriticalityLevel, ExecutionTimeModel},
};

/// Preemption behavior abstraction.
pub trait PreemptionPolicy {
    fn max_blocking_ns(&self) -> Nanos;
    fn is_preemptible(&self) -> bool;
}

impl PreemptionPolicy for PreemptionModel {
    fn max_blocking_ns(&self) -> Nanos {
        self.max_blocking()
    }

    fn is_preemptible(&self) -> bool {
        self.is_preemptible()
    }
}

/// Execution-time behavior abstraction.
pub trait ExecutionModel {
    fn resolve_wcet(&self, level: CriticalityLevel) -> Nanos;
    fn resolve_bcet(&self, level: CriticalityLevel) -> Nanos;
}

impl ExecutionModel for ExecutionTimeModel {
    fn resolve_wcet(&self, level: CriticalityLevel) -> Nanos {
        match (self, level) {
            (ExecutionTimeModel::MixedCriticality { wcet_lo, .. }, CriticalityLevel::Lo) => {
                *wcet_lo
            }
            (ExecutionTimeModel::MixedCriticality { wcet_hi, .. }, CriticalityLevel::Hi) => {
                *wcet_hi
            }
            _ => self.wcet(),
        }
    }

    fn resolve_bcet(&self, level: CriticalityLevel) -> Nanos {
        match (self, level) {
            (ExecutionTimeModel::MixedCriticality { wcet_lo, .. }, CriticalityLevel::Lo) => {
                *wcet_lo
            }
            (ExecutionTimeModel::MixedCriticality { wcet_hi, .. }, CriticalityLevel::Hi) => {
                *wcet_hi
            }
            _ => self.bcet(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_criticality_execution_model_resolves_per_level() {
        let model = ExecutionTimeModel::MixedCriticality {
            wcet_lo: 10_000,
            wcet_hi: 20_000,
        };
        assert_eq!(model.resolve_wcet(CriticalityLevel::Lo), 10_000);
        assert_eq!(model.resolve_wcet(CriticalityLevel::Hi), 20_000);
    }
}
