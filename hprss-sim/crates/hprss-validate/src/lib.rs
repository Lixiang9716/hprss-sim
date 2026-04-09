//! Validation tests against classical RT theory.
//!
//! Level 1: Liu-Layland bound, EDF optimality, Joseph-Pandya RTA
//! Level 2: Small-scale exhaustive enumeration
//! Level 3: Strict CPU-only differential validation via external SimSo adapter
//! Level 4: Heterogeneous semantics validation
//!
//! ## Running Level 3 (SimSo differential)
//! - Install Python dependency: `pip install simso`
//! - Use the default adapter runner: `scripts/simso_adapter_runner.py`
//! - Programmatic entry points:
//!   - `run_level3_simso_differential` for one workload/scheduler pair
//!   - `run_level3_simso_selected` for curated CPU-only fixtures

pub mod analytic;
pub mod differential;

pub use analytic::conditional_dag::{
    CONDITIONAL_DAG_SCOPE, ConditionAssignment, ConditionLiteral, ConditionalDagAnalysisConfig,
    ConditionalDagAnalysisError, ConditionalDagAnalysisReport, ConditionalDagEdge,
    ConditionalDagModelAssumptions, ConditionalDagNode, ConditionalDagScenarioReport,
    analyze_conditional_dag,
};
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
pub use analytic::openmp_wcrt::{
    OPENMP_WCRT_SCOPE, OpenMpWcrtConfig, OpenMpWcrtError, OpenMpWcrtModelAssumptions,
    OpenMpWcrtReport, OpenMpWcrtStatus, OpenMpWcrtTask, OpenMpWcrtTaskResult,
    OpenMpWcrtUnschedulableReason, analyze_openmp_wcrt,
};
pub use analytic::rta::{
    AnalysisAlgorithm, AnalysisConfig, AnalysisOutcome, AnalysisReport, AnalysisTaskResult, FpTask,
    InconclusiveReason, RtaConfig, RtaReport, TaskRtaResult, TaskSchedulability,
    UnschedulableReason, analyze_fp_family, analyze_uniprocessor_fp,
};
pub use analytic::shape::{
    SHAPE_BASELINE_UTILIZATION_POINTS, SHAPE_SCOPE, ShapeAnalysisConfig, ShapeAnalysisError,
    ShapeAnalysisReport, ShapeCurvePoint, ShapeCurveSample, ShapeModelAssumptions,
    analyze_shape_curve, baseline_shape_fixture,
};
pub use analytic::uniform_rta::{
    UNIFORM_RTA_SCOPE, UniformRtaConfig, UniformRtaReport, UniformTaskResult, UniformTaskStatus,
    analyze_uniform_global_fp,
};
pub use analytic::util_vectors::{
    UTIL_VECTORS_SCOPE, UtilizationVectorConfig, UtilizationVectorReport, UtilizationVectorTask,
    UtilizationVectorViolation, UtilizationVectorViolationKind, analyze_utilization_vectors,
};
pub use differential::cpu_only::{
    CpuOnlyDifferentialReport, CpuOnlyRunSummary, CpuOnlySchedulerConfig, CpuOnlyTask,
    CpuOnlyWorkload, run_cpu_only_differential, run_cpu_only_scheduler,
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
pub use differential::simso::{
    LEVEL3_SCOPE, Level3SimsoDifferentialReport, SimsoAdapterConfig, SimsoAdapterError,
    SimsoDiagnosticCategory, SimsoMismatchCategory, SimsoRunSummary, SimsoScenarioDomain,
    SimsoScenarioModel, SimsoSummaryMismatch, SimsoTaskModel, default_simso_adapter_runner,
    normalize_simso_output, run_level3_simso_differential, run_level3_simso_selected,
    validate_simso_scenario,
};
