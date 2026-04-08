//! Validation tests against classical RT theory.
//!
//! Level 1: Liu-Layland bound, EDF optimality, Joseph-Pandya RTA
//! Level 2: Small-scale exhaustive enumeration
//! Level 3: CPU-only differential baseline (in-repo, pre-adapter phase)
//! Level 4: Heterogeneous semantics validation

pub mod analytic;
pub mod differential;

pub use analytic::level1::{
    CpuTask, LEVEL1_SCOPE, Level1SimulationSummary, audsley_opa, dm_priority_assignment,
    edf_exact_bound, fp_tasks_with_priorities, hyperperiod, liu_layland_rm_bound,
    non_preemptive_exact_theory_supported, rm_priority_assignment, simulate_edf, simulate_fp,
    total_utilization,
};
pub use analytic::level2::{
    LEVEL2_SCOPE, TinyCpuScheduleSummary, TinyCpuTask, TinyDagEdge, TinyDagNode,
    TinyDagReferenceSummary, TinyHeteroScheduleSummary, TinyHeteroTask, exact_tiny_dag_reference,
    exact_tiny_fp_hetero, exact_tiny_fp_uniprocessor,
};
pub use analytic::level4::{
    FpgaSwitchObservation, HeteroPreemptionObservation, LEVEL4_SCOPE, TransferGatingObservation,
    observe_dag_transfer_gating, observe_dsp_dma_blocking, observe_fpga_non_preemptive_switch,
    observe_gpu_limited_preemption_boundary,
};
pub use analytic::rta::{
    FpTask, RtaConfig, RtaReport, TaskRtaResult, TaskSchedulability, UnschedulableReason,
    analyze_uniprocessor_fp,
};
pub use differential::cpu_only::{
    CpuOnlyDifferentialReport, CpuOnlyRunSummary, CpuOnlySchedulerConfig, CpuOnlyTask,
    CpuOnlyWorkload, LEVEL3_SCOPE, run_cpu_only_differential, run_cpu_only_scheduler,
    selected_cpu_only_workloads,
};
pub use differential::heft_repro::{
    HEFT_REPRO_SCOPE, HeftFpMakespanReproReport, HeftReproWorkload, run_heft_fp_makespan_repro,
    selected_heft_repro_workloads,
};
pub use differential::paper_exp::{
    PAPER_EXP_SCOPE, PaperExperimentSummaryReport, PaperHeftMakespanRow, PaperShapeCurvePoint,
    run_paper_experiment_summary,
};
