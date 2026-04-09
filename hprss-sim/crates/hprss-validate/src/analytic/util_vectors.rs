/// Scope marker for utilization-vector analytic checks.
pub const UTIL_VECTORS_SCOPE: &str =
    "paper-inspired utilization-vector checks for deterministic fixtures";

#[derive(Debug, Clone, PartialEq)]
pub struct UtilizationVectorTask {
    pub name: String,
    pub components: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilizationVectorConfig {
    pub epsilon: f64,
    pub dominant_share_limit: Option<f64>,
}

impl Default for UtilizationVectorConfig {
    fn default() -> Self {
        Self {
            epsilon: 1e-12,
            dominant_share_limit: Some(1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtilizationVectorViolationKind {
    InvalidCapacity,
    DimensionMismatch,
    NonFiniteComponent,
    NegativeComponent,
    TaskComponentExceeded,
    CapacityExceeded,
    DominantShareExceeded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UtilizationVectorViolation {
    pub kind: UtilizationVectorViolationKind,
    pub task_index: Option<usize>,
    pub task_name: Option<String>,
    pub dimension: Option<usize>,
    pub observed: f64,
    pub limit: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UtilizationVectorReport {
    pub scope: &'static str,
    pub task_count: usize,
    pub dimension_count: usize,
    pub aggregate_utilization: Vec<f64>,
    pub dominant_shares: Vec<f64>,
    pub dominant_share_sum: f64,
    pub passes: bool,
    pub violations: Vec<UtilizationVectorViolation>,
}

pub fn analyze_utilization_vectors(
    tasks: &[UtilizationVectorTask],
    capacities: &[f64],
    config: UtilizationVectorConfig,
) -> UtilizationVectorReport {
    let epsilon = if config.epsilon.is_finite() && config.epsilon > 0.0 {
        config.epsilon
    } else {
        1e-12
    };
    let dimension_count = capacities.len();
    let mut violations = Vec::<UtilizationVectorViolation>::new();
    let mut aggregate_utilization = vec![0.0; dimension_count];
    let mut dominant_shares = Vec::with_capacity(tasks.len());

    for (dimension, &capacity) in capacities.iter().enumerate() {
        if !capacity.is_finite() || capacity <= 0.0 {
            violations.push(UtilizationVectorViolation {
                kind: UtilizationVectorViolationKind::InvalidCapacity,
                task_index: None,
                task_name: None,
                dimension: Some(dimension),
                observed: quantize(capacity, epsilon),
                limit: 0.0,
            });
        }
    }

    for (task_index, task) in tasks.iter().enumerate() {
        if task.components.len() != dimension_count {
            violations.push(UtilizationVectorViolation {
                kind: UtilizationVectorViolationKind::DimensionMismatch,
                task_index: Some(task_index),
                task_name: Some(task.name.clone()),
                dimension: None,
                observed: quantize(task.components.len() as f64, epsilon),
                limit: quantize(dimension_count as f64, epsilon),
            });
            dominant_shares.push(0.0);
            continue;
        }

        let mut dominant_share = 0.0_f64;
        for (dimension, &component) in task.components.iter().enumerate() {
            if !component.is_finite() {
                violations.push(UtilizationVectorViolation {
                    kind: UtilizationVectorViolationKind::NonFiniteComponent,
                    task_index: Some(task_index),
                    task_name: Some(task.name.clone()),
                    dimension: Some(dimension),
                    observed: component,
                    limit: f64::INFINITY,
                });
                continue;
            }

            if component < -epsilon {
                violations.push(UtilizationVectorViolation {
                    kind: UtilizationVectorViolationKind::NegativeComponent,
                    task_index: Some(task_index),
                    task_name: Some(task.name.clone()),
                    dimension: Some(dimension),
                    observed: quantize(component, epsilon),
                    limit: 0.0,
                });
            }

            aggregate_utilization[dimension] += component;
            let capacity = capacities[dimension];
            if capacity.is_finite() && capacity > epsilon {
                if component > capacity + epsilon {
                    violations.push(UtilizationVectorViolation {
                        kind: UtilizationVectorViolationKind::TaskComponentExceeded,
                        task_index: Some(task_index),
                        task_name: Some(task.name.clone()),
                        dimension: Some(dimension),
                        observed: quantize(component, epsilon),
                        limit: quantize(capacity, epsilon),
                    });
                }
                dominant_share = dominant_share.max(component / capacity);
            }
        }
        dominant_shares.push(quantize(dominant_share, epsilon));
    }

    for (dimension, &aggregate) in aggregate_utilization.iter().enumerate() {
        let capacity = capacities.get(dimension).copied().unwrap_or(0.0);
        if capacity.is_finite() && aggregate > capacity + epsilon {
            violations.push(UtilizationVectorViolation {
                kind: UtilizationVectorViolationKind::CapacityExceeded,
                task_index: None,
                task_name: None,
                dimension: Some(dimension),
                observed: quantize(aggregate, epsilon),
                limit: quantize(capacity, epsilon),
            });
        }
    }

    let dominant_share_sum = quantize(dominant_shares.iter().sum::<f64>(), epsilon);
    if let Some(limit) = config.dominant_share_limit
        && dominant_share_sum > limit + epsilon
    {
        violations.push(UtilizationVectorViolation {
            kind: UtilizationVectorViolationKind::DominantShareExceeded,
            task_index: None,
            task_name: None,
            dimension: None,
            observed: dominant_share_sum,
            limit: quantize(limit, epsilon),
        });
    }

    for value in &mut aggregate_utilization {
        *value = quantize(*value, epsilon);
    }

    UtilizationVectorReport {
        scope: UTIL_VECTORS_SCOPE,
        task_count: tasks.len(),
        dimension_count,
        aggregate_utilization,
        dominant_shares,
        dominant_share_sum,
        passes: violations.is_empty(),
        violations,
    }
}

fn quantize(value: f64, epsilon: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    (value / epsilon).round() * epsilon
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str, components: &[f64]) -> UtilizationVectorTask {
        UtilizationVectorTask {
            name: name.to_string(),
            components: components.to_vec(),
        }
    }

    #[test]
    fn feasible_vectors_pass_all_checks() {
        let report = analyze_utilization_vectors(
            &[
                task("a", &[0.25, 0.20]),
                task("b", &[0.20, 0.25]),
                task("c", &[0.10, 0.10]),
            ],
            &[1.0, 1.0],
            UtilizationVectorConfig::default(),
        );
        assert!(report.passes);
        assert!(report.violations.is_empty());
        assert_eq!(report.aggregate_utilization, vec![0.55, 0.55]);
        assert_eq!(report.dominant_share_sum, 0.6);
    }

    #[test]
    fn overloaded_dimension_is_reported() {
        let report = analyze_utilization_vectors(
            &[task("a", &[0.60, 0.10]), task("b", &[0.45, 0.20])],
            &[1.0, 1.0],
            UtilizationVectorConfig {
                dominant_share_limit: None,
                ..UtilizationVectorConfig::default()
            },
        );
        assert!(!report.passes);
        assert!(report.violations.iter().any(|v| {
            v.kind == UtilizationVectorViolationKind::CapacityExceeded && v.dimension == Some(0)
        }));
    }
}
