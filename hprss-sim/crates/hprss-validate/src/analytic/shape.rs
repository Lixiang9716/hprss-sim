use thiserror::Error;

/// Scope marker for SHAPE-style schedulability-curve analytics.
pub const SHAPE_SCOPE: &str =
    "SHAPE-style schedulability curve checks with explicit trend and confidence bounds";

/// Deterministic utilization points used by the in-repo paper baseline.
pub const SHAPE_BASELINE_UTILIZATION_POINTS: [f64; 5] = [0.4, 0.7, 1.0, 1.3, 1.6];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeModelAssumptions {
    /// Task sets are generated using a fixed deterministic fixture per utilization point.
    pub workload_fixture: &'static str,
    /// The schedulability ratio is interpreted as Bernoulli success frequency over finite seeds.
    pub ratio_interpretation: &'static str,
    /// Trend expectation for project baseline curves.
    pub trend_expectation: &'static str,
    /// Confidence envelope model for each point.
    pub confidence_model: &'static str,
}

impl Default for ShapeModelAssumptions {
    fn default() -> Self {
        Self {
            workload_fixture: "fixed utilization-grid fixture consistent with paper baseline points",
            ratio_interpretation: "schedulability ratio = schedulable_runs / total_runs",
            trend_expectation: "ratio is expected to be non-increasing as utilization increases",
            confidence_model: "Clopper-Pearson exact binomial two-sided confidence interval",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeCurveSample {
    pub utilization: f64,
    pub schedulable_runs: u32,
    pub total_runs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeAnalysisConfig {
    pub confidence: f64,
    pub trend_epsilon: f64,
    pub require_non_increasing_trend: bool,
}

impl Default for ShapeAnalysisConfig {
    fn default() -> Self {
        Self {
            confidence: 0.95,
            trend_epsilon: 1e-12,
            require_non_increasing_trend: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeCurvePoint {
    pub utilization: f64,
    pub schedulability_ratio: f64,
    pub lower_confidence_bound: f64,
    pub upper_confidence_bound: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeAnalysisReport {
    pub scope: &'static str,
    pub model_assumptions: ShapeModelAssumptions,
    pub confidence: f64,
    pub trend_ok: bool,
    pub points: Vec<ShapeCurvePoint>,
}

impl ShapeAnalysisReport {
    pub fn is_trend_consistent(&self) -> bool {
        self.trend_ok
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ShapeAnalysisError {
    #[error("SHAPE analysis requires at least one sample")]
    EmptySamples,
    #[error("config confidence must be finite and in (0, 1), got {confidence}")]
    InvalidConfidence { confidence: f64 },
    #[error("config trend_epsilon must be finite and non-negative, got {trend_epsilon}")]
    InvalidTrendEpsilon { trend_epsilon: f64 },
    #[error("sample {index} utilization must be finite and non-negative, got {utilization}")]
    InvalidUtilization { index: usize, utilization: f64 },
    #[error("sample {index} total_runs must be > 0")]
    ZeroTotalRuns { index: usize },
    #[error("sample {index} has schedulable_runs ({schedulable_runs}) > total_runs ({total_runs})")]
    InvalidRunCount {
        index: usize,
        schedulable_runs: u32,
        total_runs: u32,
    },
    #[error(
        "sample {index} utilization ({utilization}) is lower than previous sample utilization ({previous})"
    )]
    UtilizationOrderViolation {
        index: usize,
        previous: f64,
        utilization: f64,
    },
    #[error(
        "sample {index} ratio ({ratio}) violates non-increasing trend; previous ratio was {previous_ratio}"
    )]
    IncreasingTrend {
        index: usize,
        previous_ratio: f64,
        ratio: f64,
    },
}

pub fn analyze_shape_curve(
    samples: &[ShapeCurveSample],
    config: ShapeAnalysisConfig,
) -> Result<ShapeAnalysisReport, ShapeAnalysisError> {
    if samples.is_empty() {
        return Err(ShapeAnalysisError::EmptySamples);
    }
    if !config.confidence.is_finite() || config.confidence <= 0.0 || config.confidence >= 1.0 {
        return Err(ShapeAnalysisError::InvalidConfidence {
            confidence: config.confidence,
        });
    }
    if !config.trend_epsilon.is_finite() || config.trend_epsilon < 0.0 {
        return Err(ShapeAnalysisError::InvalidTrendEpsilon {
            trend_epsilon: config.trend_epsilon,
        });
    }

    let mut points = Vec::with_capacity(samples.len());
    let mut previous_utilization = None::<f64>;
    let mut previous_ratio = None::<f64>;

    for (index, sample) in samples.iter().copied().enumerate() {
        if !sample.utilization.is_finite() || sample.utilization < 0.0 {
            return Err(ShapeAnalysisError::InvalidUtilization {
                index,
                utilization: sample.utilization,
            });
        }
        if sample.total_runs == 0 {
            return Err(ShapeAnalysisError::ZeroTotalRuns { index });
        }
        if sample.schedulable_runs > sample.total_runs {
            return Err(ShapeAnalysisError::InvalidRunCount {
                index,
                schedulable_runs: sample.schedulable_runs,
                total_runs: sample.total_runs,
            });
        }

        if let Some(prev_u) = previous_utilization
            && sample.utilization + config.trend_epsilon < prev_u
        {
            return Err(ShapeAnalysisError::UtilizationOrderViolation {
                index,
                previous: prev_u,
                utilization: sample.utilization,
            });
        }

        let ratio = sample.schedulable_runs as f64 / sample.total_runs as f64;
        if let Some(prev_ratio) = previous_ratio
            && config.require_non_increasing_trend
            && ratio > prev_ratio + config.trend_epsilon
        {
            return Err(ShapeAnalysisError::IncreasingTrend {
                index,
                previous_ratio: prev_ratio,
                ratio,
            });
        }

        let (lower_confidence_bound, upper_confidence_bound) = clopper_pearson_bounds(
            sample.schedulable_runs,
            sample.total_runs,
            config.confidence,
        );

        points.push(ShapeCurvePoint {
            utilization: sample.utilization,
            schedulability_ratio: ratio,
            lower_confidence_bound,
            upper_confidence_bound,
        });

        previous_utilization = Some(sample.utilization);
        previous_ratio = Some(ratio);
    }

    Ok(ShapeAnalysisReport {
        scope: SHAPE_SCOPE,
        model_assumptions: ShapeModelAssumptions::default(),
        confidence: config.confidence,
        trend_ok: true,
        points,
    })
}

fn clopper_pearson_bounds(schedulable_runs: u32, total_runs: u32, confidence: f64) -> (f64, f64) {
    let alpha = 1.0 - confidence;
    let tail_probability = alpha / 2.0;

    let lower = if schedulable_runs == 0 {
        0.0
    } else {
        invert_binomial_tail_lower(schedulable_runs, total_runs, tail_probability)
    };

    let upper = if schedulable_runs == total_runs {
        1.0
    } else {
        invert_binomial_cdf_upper(schedulable_runs, total_runs, tail_probability)
    };

    (lower, upper)
}

fn invert_binomial_tail_lower(successes: u32, trials: u32, target: f64) -> f64 {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..100 {
        let mid = (low + high) / 2.0;
        if binomial_tail_ge(successes, trials, mid) < target {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

fn invert_binomial_cdf_upper(successes: u32, trials: u32, target: f64) -> f64 {
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..100 {
        let mid = (low + high) / 2.0;
        if binomial_cdf(successes, trials, mid) > target {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

fn binomial_tail_ge(successes: u32, trials: u32, p: f64) -> f64 {
    if successes == 0 {
        1.0
    } else {
        (1.0 - binomial_cdf(successes - 1, trials, p)).clamp(0.0, 1.0)
    }
}

fn binomial_cdf(successes: u32, trials: u32, p: f64) -> f64 {
    if successes >= trials {
        return 1.0;
    }
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return 0.0;
    }

    let q = 1.0 - p;
    let mut pmf = q.powf(trials as f64);
    let mut cumulative = pmf;

    for i in 0..successes {
        let numerator = (trials - i) as f64;
        let denominator = (i + 1) as f64;
        pmf *= numerator / denominator * p / q;
        cumulative += pmf;
    }

    cumulative.clamp(0.0, 1.0)
}

pub fn baseline_shape_fixture() -> Vec<ShapeCurveSample> {
    [
        ShapeCurveSample {
            utilization: 0.4,
            schedulable_runs: 5,
            total_runs: 8,
        },
        ShapeCurveSample {
            utilization: 0.7,
            schedulable_runs: 2,
            total_runs: 8,
        },
        ShapeCurveSample {
            utilization: 1.0,
            schedulable_runs: 0,
            total_runs: 8,
        },
        ShapeCurveSample {
            utilization: 1.3,
            schedulable_runs: 0,
            total_runs: 8,
        },
        ShapeCurveSample {
            utilization: 1.6,
            schedulable_runs: 0,
            total_runs: 8,
        },
    ]
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_fixture_matches_utilization_grid() {
        let fixture = baseline_shape_fixture();
        let utils: Vec<f64> = fixture.iter().map(|point| point.utilization).collect();
        assert_eq!(utils, SHAPE_BASELINE_UTILIZATION_POINTS.to_vec());
    }

    #[test]
    fn baseline_fixture_produces_monotonic_curve() {
        let report = analyze_shape_curve(&baseline_shape_fixture(), ShapeAnalysisConfig::default())
            .expect("baseline fixture should be valid");

        assert!(report.is_trend_consistent());
        assert_eq!(report.scope, SHAPE_SCOPE);
        for point in &report.points {
            assert!((0.0..=1.0).contains(&point.schedulability_ratio));
            assert!((0.0..=1.0).contains(&point.lower_confidence_bound));
            assert!((0.0..=1.0).contains(&point.upper_confidence_bound));
            assert!(point.lower_confidence_bound <= point.schedulability_ratio);
            assert!(point.schedulability_ratio <= point.upper_confidence_bound);
        }
    }
}
