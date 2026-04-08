use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::FixedPriorityScheduler;
use hprss_types::{
    DeviceId,
    device::{DeviceConfig, PreemptionModel},
    task::DeviceType,
};
use hprss_workload::{WorkloadConfig, generate_taskset};

use super::heft_repro::{run_heft_fp_makespan_repro, selected_heft_repro_workloads};

/// Scope marker for the paper-facing, in-repo experiment summary artifact.
pub const PAPER_EXP_SCOPE: &str = "paper-facing HEFT+SHAPE baseline summary";

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PaperHeftMakespanRow {
    pub workload: String,
    pub fp_makespan: u64,
    pub heft_makespan: u64,
    pub heft_speedup: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PaperShapeCurvePoint {
    pub utilization: f64,
    pub schedulability_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PaperExperimentSummaryReport {
    pub scope: &'static str,
    pub heft_makespan_rows: Vec<PaperHeftMakespanRow>,
    pub shape_schedulability_curve: Vec<PaperShapeCurvePoint>,
}

impl PaperExperimentSummaryReport {
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn to_csv(&self) -> String {
        let mut lines = vec![
            "section,label,fp_makespan_ns,heft_makespan_ns,heft_speedup,utilization,schedulability_ratio".to_string(),
        ];

        lines.extend(self.heft_makespan_rows.iter().map(|row| {
            format!(
                "heft,{},{},{},{:.6},,",
                row.workload, row.fp_makespan, row.heft_makespan, row.heft_speedup
            )
        }));

        lines.extend(self.shape_schedulability_curve.iter().map(|point| {
            format!(
                "shape_curve,,,,,{:.3},{:.6}",
                point.utilization, point.schedulability_ratio
            )
        }));

        lines.join("\n")
    }

    pub fn to_table(&self) -> String {
        let mut out = String::from(
            "=== HEFT makespan baseline ===\nworkload | FP(ns) | HEFT(ns) | speedup\n",
        );
        for row in &self.heft_makespan_rows {
            out.push_str(&format!(
                "{} | {} | {} | {:.3}\n",
                row.workload, row.fp_makespan, row.heft_makespan, row.heft_speedup
            ));
        }

        out.push_str(
            "\n=== SHAPE-style schedulability baseline ===\nutilization | schedulability_ratio\n",
        );
        for point in &self.shape_schedulability_curve {
            out.push_str(&format!(
                "{:.3} | {:.3}\n",
                point.utilization, point.schedulability_ratio
            ));
        }
        out
    }
}

pub fn run_paper_experiment_summary() -> PaperExperimentSummaryReport {
    let heft_makespan_rows = selected_heft_repro_workloads()
        .into_iter()
        .map(|workload| {
            let report = run_heft_fp_makespan_repro(&workload);
            PaperHeftMakespanRow {
                workload: workload.name.to_string(),
                fp_makespan: report.fp_makespan,
                heft_makespan: report.heft_makespan,
                heft_speedup: report.heft_speedup,
            }
        })
        .collect();

    PaperExperimentSummaryReport {
        scope: PAPER_EXP_SCOPE,
        heft_makespan_rows,
        shape_schedulability_curve: shape_style_fp_baseline_curve(),
    }
}

fn shape_style_fp_baseline_curve() -> Vec<PaperShapeCurvePoint> {
    const UTILIZATION_POINTS: [f64; 5] = [0.4, 0.7, 1.0, 1.3, 1.6];
    const SEEDS: std::ops::RangeInclusive<u64> = 1..=8;

    UTILIZATION_POINTS
        .iter()
        .map(|&utilization| {
            let schedulable_runs = SEEDS
                .clone()
                .filter(|&seed| run_shape_baseline_single_seed(utilization, seed))
                .count();
            PaperShapeCurvePoint {
                utilization,
                schedulability_ratio: schedulable_runs as f64 / SEEDS.clone().count() as f64,
            }
        })
        .collect()
}

fn run_shape_baseline_single_seed(utilization: f64, seed: u64) -> bool {
    let devices = vec![shape_baseline_cpu_device()];
    let tasks = generate_taskset(
        &WorkloadConfig {
            num_tasks: 12,
            total_utilization: utilization,
            period_range_ms: (1, 4),
            seed,
        },
        &devices,
    );
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 40_000_000,
            seed,
        },
        devices,
        vec![],
        vec![],
    );

    engine.register_tasks(tasks);
    engine.schedule_initial_arrivals();
    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    let summary = engine.summary();
    summary.deadline_misses == 0 && summary.completed_jobs == summary.total_jobs
}

fn shape_baseline_cpu_device() -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(0),
        name: "cpu-0".to_string(),
        device_group: None,
        device_type: DeviceType::Cpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 1_000,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_summary_is_deterministic() {
        let first = run_paper_experiment_summary();
        let second = run_paper_experiment_summary();

        assert_eq!(first, second);
    }

    #[test]
    fn paper_summary_outputs_cover_both_baselines() {
        let report = run_paper_experiment_summary();
        assert!(!report.heft_makespan_rows.is_empty());
        assert!(!report.shape_schedulability_curve.is_empty());
        assert!(
            report
                .heft_makespan_rows
                .iter()
                .all(|row| row.heft_makespan <= row.fp_makespan)
        );

        let csv = report.to_csv();
        let json = report
            .to_json_pretty()
            .expect("paper summary should serialize to JSON");
        let table = report.to_table();

        assert!(csv.contains("section,label,fp_makespan_ns"));
        assert!(csv.contains("heft,"));
        assert!(csv.contains("shape_curve,"));
        assert!(json.contains("\"heft_makespan_rows\""));
        assert!(json.contains("\"shape_schedulability_curve\""));
        assert!(table.contains("HEFT makespan baseline"));
        assert!(table.contains("SHAPE-style schedulability baseline"));
    }
}
