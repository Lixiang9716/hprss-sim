use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::{Parser, Subcommand};
use hprss_engine::engine::{SimConfig, SimEngine, SimResult};
use hprss_platform::PlatformConfig;
use hprss_types::{EventKind, TaskId};
use hprss_workload::{
    ReplayWorkload, WorkloadConfig, adapt_karami_paper_profile_json, adapt_openmp_specialized_json,
    generate_taskset, load_replay_csv, load_replay_json,
};
use rayon::prelude::*;
use scheduler_catalog::{
    SchedulerKind, build_scheduler, parse_scheduler_list, scheduler_family, scheduler_key,
    scheduler_label,
};
use tracing_subscriber::EnvFilter;

mod scheduler_catalog;

#[derive(Debug, Parser)]
#[command(name = "hprss-sim", about = "HPRSS heterogeneous DES simulator")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    // ── Flat args for backward-compatible `hprss-sim --platform ...` usage ──
    /// Platform TOML config path
    #[arg(long, global = true)]
    platform: Option<PathBuf>,

    /// Number of tasks to generate
    #[arg(long, default_value_t = 10, global = true)]
    tasks: usize,

    /// Target total utilization
    #[arg(long, default_value_t = 0.6, global = true)]
    utilization: f64,

    /// Override random seed from platform config
    #[arg(long, global = true)]
    seed: Option<u64>,

    /// Enable verbose trace logs
    #[arg(long, default_value_t = false, global = true)]
    verbose: bool,

    /// Scheduling algorithm
    #[arg(long, value_enum, default_value_t = SchedulerKind::Fp, global = true)]
    scheduler: SchedulerKind,

    /// Optional analysis profile tag to include in run/sweep outputs
    #[arg(long, value_enum, default_value_t = AnalysisMode::None, global = true)]
    analysis_mode: AnalysisMode,

    /// Write simulation trace as JSON-lines
    #[arg(long, global = true)]
    trace_output: Option<PathBuf>,

    /// Replay workload JSON file (replaces synthetic generation)
    #[arg(
        long,
        global = true,
        conflicts_with_all = [
            "replay_csv_tasks",
            "replay_csv_jobs",
            "openmp_specialized_json",
            "karami_profile_json"
        ]
    )]
    replay_json: Option<PathBuf>,

    /// Replay workload task CSV file
    #[arg(
        long,
        global = true,
        requires = "replay_csv_jobs",
        conflicts_with_all = ["replay_json", "openmp_specialized_json", "karami_profile_json"]
    )]
    replay_csv_tasks: Option<PathBuf>,

    /// Replay workload job CSV file
    #[arg(
        long,
        global = true,
        requires = "replay_csv_tasks",
        conflicts_with_all = ["replay_json", "openmp_specialized_json", "karami_profile_json"]
    )]
    replay_csv_jobs: Option<PathBuf>,

    /// OpenMP-specialized workload JSON file (adapted into replay IR)
    #[arg(
        long,
        global = true,
        conflicts_with_all = [
            "replay_json",
            "replay_csv_tasks",
            "replay_csv_jobs",
            "karami_profile_json"
        ]
    )]
    openmp_specialized_json: Option<PathBuf>,

    /// Karami paper-profile workload JSON file (adapted into replay IR)
    #[arg(
        long,
        global = true,
        conflicts_with_all = [
            "replay_json",
            "replay_csv_tasks",
            "replay_csv_jobs",
            "openmp_specialized_json"
        ]
    )]
    karami_profile_json: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a single simulation (default when no subcommand given)
    Run,
    /// Sweep a parameter space in parallel and write results to CSV
    Sweep(SweepArgs),
}

#[derive(Debug, clap::Args)]
struct SweepArgs {
    /// Utilization range: start:step:end (e.g. 0.5:0.1:0.9)
    #[arg(long, default_value = "0.5:0.1:0.9")]
    utilizations: String,

    /// Comma-separated task counts (e.g. 10,50,100,250)
    #[arg(long, default_value = "10,50,100")]
    task_counts: String,

    /// Seed range: start:end (inclusive, e.g. 1:10)
    #[arg(long, default_value = "1:5")]
    seeds: String,

    /// Output CSV path
    #[arg(long, short, default_value = "sweep_results.csv")]
    output: PathBuf,

    /// Number of Rayon threads (0 = all available cores)
    #[arg(long, default_value_t = 0)]
    jobs: usize,

    /// Comma-separated algorithms (fp,edf,edfvd,llf,heft,cpedf,federated,global-edf,gang,xsched,gcaps,gpreempt,rtgpu,match,gpu-preemptive-priority)
    #[arg(
        long,
        default_value = "fp,edf,edfvd,llf,heft,cpedf,federated,global-edf,gang,xsched,gcaps,gpreempt,rtgpu,match,gpu-preemptive-priority"
    )]
    schedulers: String,

    /// Comma-separated analysis modes (none,rta-uniprocessor-fp,rta-uniform-global-fp-scaffold,util-vectors,uniform-rta-global-fp,conditional-dag,shape,openmp-wcrt,simso-scope-extension)
    #[arg(long, default_value = "none")]
    analysis_modes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum AnalysisMode {
    None,
    RtaUniprocessorFp,
    RtaUniformGlobalFpScaffold,
    UtilVectors,
    UniformRtaGlobalFp,
    ConditionalDag,
    Shape,
    OpenmpWcrt,
    SimsoScopeExtension,
}

impl AnalysisMode {
    fn key(self) -> &'static str {
        match self {
            AnalysisMode::None => "none",
            AnalysisMode::RtaUniprocessorFp => "rta-uniprocessor-fp",
            AnalysisMode::RtaUniformGlobalFpScaffold => "rta-uniform-global-fp-scaffold",
            AnalysisMode::UtilVectors => "util-vectors",
            AnalysisMode::UniformRtaGlobalFp => "uniform-rta-global-fp",
            AnalysisMode::ConditionalDag => "conditional-dag",
            AnalysisMode::Shape => "shape",
            AnalysisMode::OpenmpWcrt => "openmp-wcrt",
            AnalysisMode::SimsoScopeExtension => "simso-scope-extension",
        }
    }

    fn scope(self) -> &'static str {
        match self {
            AnalysisMode::None => "none",
            AnalysisMode::RtaUniprocessorFp => "uniprocessor fixed-priority response-time analysis",
            AnalysisMode::RtaUniformGlobalFpScaffold => {
                "uniform global fixed-priority response-time scaffold"
            }
            AnalysisMode::UtilVectors => "utilization vectors",
            AnalysisMode::UniformRtaGlobalFp => {
                "uniform global fixed-priority response-time analysis"
            }
            AnalysisMode::ConditionalDag => "conditional DAG response-time analysis",
            AnalysisMode::Shape => "shape-style schedulability curve analysis",
            AnalysisMode::OpenmpWcrt => "openmp fixed-priority wcrt analysis",
            AnalysisMode::SimsoScopeExtension => "simso differential scope extension",
        }
    }
}

fn parse_analysis_mode_list(s: &str) -> Result<Vec<AnalysisMode>, String> {
    let mut out = Vec::new();
    for token in s.split(',').map(|v| v.trim()).filter(|v| !v.is_empty()) {
        let mode = match token.to_ascii_lowercase().as_str() {
            "none" => AnalysisMode::None,
            "rta-uniprocessor-fp" | "rta_uniprocessor_fp" | "uniprocessor-fp" => {
                AnalysisMode::RtaUniprocessorFp
            }
            "rta-uniform-global-fp-scaffold"
            | "rta_uniform_global_fp_scaffold"
            | "uniform-global-fp"
            | "uniform-global-fp-scaffold" => AnalysisMode::RtaUniformGlobalFpScaffold,
            "util-vectors" | "util_vectors" | "utilization-vectors" => AnalysisMode::UtilVectors,
            "uniform-rta-global-fp" | "uniform_rta_global_fp" | "uniform-rta" => {
                AnalysisMode::UniformRtaGlobalFp
            }
            "conditional-dag" | "conditional_dag" => AnalysisMode::ConditionalDag,
            "shape" => AnalysisMode::Shape,
            "openmp-wcrt" | "openmp_wcrt" => AnalysisMode::OpenmpWcrt,
            "simso-scope-extension" | "simso_scope_extension" | "simso-extension" => {
                AnalysisMode::SimsoScopeExtension
            }
            _ => return Err(format!("unknown analysis mode: {token}")),
        };
        out.push(mode);
    }
    if out.is_empty() {
        return Err("analysis mode list is empty".to_string());
    }
    Ok(out)
}

/// Parse "start:step:end" into a Vec<f64>.
fn parse_range_f64(s: &str) -> Result<Vec<f64>, String> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        3 => {
            let start: f64 = parts[0].parse().map_err(|e| format!("{e}"))?;
            let step: f64 = parts[1].parse().map_err(|e| format!("{e}"))?;
            let end: f64 = parts[2].parse().map_err(|e| format!("{e}"))?;
            if step <= 0.0 {
                return Err("step must be positive".into());
            }
            let mut vals = Vec::new();
            let mut v = start;
            while v <= end + f64::EPSILON {
                vals.push((v * 1000.0).round() / 1000.0);
                v += step;
            }
            Ok(vals)
        }
        1 => {
            let v: f64 = parts[0].parse().map_err(|e| format!("{e}"))?;
            Ok(vec![v])
        }
        _ => Err("expected format: start:step:end or single value".into()),
    }
}

/// Parse "start:end" into a Vec<u64>.
fn parse_range_u64(s: &str) -> Result<Vec<u64>, String> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let start: u64 = parts[0].parse().map_err(|e| format!("{e}"))?;
            let end: u64 = parts[1].parse().map_err(|e| format!("{e}"))?;
            Ok((start..=end).collect())
        }
        1 => {
            let v: u64 = parts[0].parse().map_err(|e| format!("{e}"))?;
            Ok(vec![v])
        }
        _ => Err("expected format: start:end or single value".into()),
    }
}

fn init_tracing(verbose: bool) {
    let default_level = if verbose { "trace" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// One row in the sweep CSV output.
#[derive(Debug, serde::Serialize)]
struct SweepRow {
    utilization: f64,
    task_count: usize,
    seed: u64,
    algorithm: String,
    algorithm_key: String,
    algorithm_family: String,
    analysis_mode: String,
    analysis_scope: String,
    analysis_assumptions: String,
    analysis_assumption_count: usize,
    workload_source: String,
    approximation_assumptions: String,
    approximation_assumption_count: usize,
    total_jobs: u64,
    completed_jobs: u64,
    deadline_misses: u64,
    miss_ratio: f64,
    schedulable: bool,
    makespan: u64,
    avg_response_time: f64,
    events_processed: u64,
    wall_time_us: u64,
    config_hash: String,
    git_commit: String,
    timestamp: String,
    per_device_utilization: String,
    transfer_overhead: u64,
    blocking_breakdown: String,
    worst_response_time: u64,
    preemption_count: u64,
    migration_count: u64,
    bus_contention_ratio: f64,
    energy_total_joules: f64,
}

fn config_hash(path: &Path) -> anyhow::Result<String> {
    let content = std::fs::read(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

fn git_commit() -> String {
    ProcessCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn run_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn serialize_csv_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// Run a single simulation and return the result with timing.
enum WorkloadInput {
    Synthetic { num_tasks: usize, utilization: f64 },
    Replay(ReplayWorkload),
}

#[derive(Debug, Clone)]
struct WorkloadMetadataSnapshot {
    source: String,
    assumptions_json: String,
    assumption_count: usize,
}

#[derive(Debug, serde::Serialize)]
struct AnalysisAssumption<'a> {
    code: &'a str,
    detail: &'a str,
}

#[derive(Debug, Clone)]
struct AnalysisMetadataSnapshot {
    scope: String,
    assumptions_json: String,
    assumption_count: usize,
}

fn analysis_metadata_snapshot(mode: AnalysisMode) -> AnalysisMetadataSnapshot {
    let assumptions: Vec<AnalysisAssumption<'_>> = match mode {
        AnalysisMode::None => Vec::new(),
        AnalysisMode::RtaUniprocessorFp => vec![AnalysisAssumption {
            code: "fp-uniprocessor-idealized",
            detail: "single-core FP approximation; no heterogeneous interference terms",
        }],
        AnalysisMode::RtaUniformGlobalFpScaffold => vec![AnalysisAssumption {
            code: "uniform-global-fp-scaffold",
            detail: "uniform global-FP scaffold model with conservative interference bounds",
        }],
        AnalysisMode::UtilVectors => vec![AnalysisAssumption {
            code: "util-vector-capacity",
            detail: "capacity-vector feasibility abstraction; does not model runtime jitter",
        }],
        AnalysisMode::UniformRtaGlobalFp => vec![AnalysisAssumption {
            code: "uniform-speed-abstraction",
            detail: "processors represented by normalized speed factors in uniform-RTA model",
        }],
        AnalysisMode::ConditionalDag => vec![AnalysisAssumption {
            code: "conditional-dag-boolean-branches",
            detail: "branch predicates are boolean and deterministic per scenario",
        }],
        AnalysisMode::Shape => vec![AnalysisAssumption {
            code: "shape-fixture-baseline",
            detail: "curve trend validated against deterministic fixture points and confidence bounds",
        }],
        AnalysisMode::OpenmpWcrt => vec![AnalysisAssumption {
            code: "openmp-fixed-pool",
            detail: "fixed-size OpenMP thread pool and fixed-point interference recurrence",
        }],
        AnalysisMode::SimsoScopeExtension => vec![AnalysisAssumption {
            code: "simso-adapter-scope",
            detail: "adapter-level scope expansion metadata; no external simso run in CLI mode",
        }],
    };
    AnalysisMetadataSnapshot {
        scope: mode.scope().to_string(),
        assumptions_json: serialize_csv_json(&assumptions),
        assumption_count: assumptions.len(),
    }
}

fn validate_analysis_mode_for_workload(
    mode: AnalysisMode,
    workload: &WorkloadInput,
) -> anyhow::Result<()> {
    if mode == AnalysisMode::OpenmpWcrt {
        match workload {
            WorkloadInput::Replay(replay)
                if replay
                    .metadata
                    .source
                    .as_deref()
                    .map(|src| src.contains("openmp"))
                    .unwrap_or(false) => {}
            _ => {
                anyhow::bail!(
                    "analysis mode '{}' requires --openmp-specialized-json workload input",
                    mode.key()
                );
            }
        }
    }
    Ok(())
}

fn workload_metadata_snapshot(workload: &WorkloadInput) -> WorkloadMetadataSnapshot {
    match workload {
        WorkloadInput::Synthetic { .. } => WorkloadMetadataSnapshot {
            source: "synthetic".to_string(),
            assumptions_json: "[]".to_string(),
            assumption_count: 0,
        },
        WorkloadInput::Replay(replay) => {
            let source = replay
                .metadata
                .source
                .clone()
                .unwrap_or_else(|| "replay".to_string());
            let assumptions_json = serialize_csv_json(&replay.metadata.assumptions);
            let assumption_count = replay.metadata.assumptions.len();
            WorkloadMetadataSnapshot {
                source,
                assumptions_json,
                assumption_count,
            }
        }
    }
}

fn run_single(
    platform: &PlatformConfig,
    workload: &WorkloadInput,
    seed: u64,
    scheduler_kind: SchedulerKind,
    trace_output: Option<&std::path::Path>,
) -> anyhow::Result<(SimResult, u64)> {
    let devices = platform.build_devices()?;
    let interconnects = platform.build_interconnects(&devices)?;
    let buses = platform.build_buses()?;
    let synthetic_tasks = match workload {
        WorkloadInput::Synthetic {
            num_tasks,
            utilization,
        } => {
            let workload_cfg = WorkloadConfig {
                num_tasks: *num_tasks,
                total_utilization: *utilization,
                seed,
                ..WorkloadConfig::default()
            };
            Some(generate_taskset(&workload_cfg, &devices))
        }
        WorkloadInput::Replay(_) => None,
    };

    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: platform.duration_ns(),
            seed,
        },
        devices,
        interconnects,
        buses,
    );
    match workload {
        WorkloadInput::Synthetic { .. } => {
            if let Some(tasks) = synthetic_tasks {
                engine.register_tasks(tasks);
            }
            engine.schedule_initial_arrivals();
        }
        WorkloadInput::Replay(replay) => {
            let replay_tasks = replay.to_tasks();
            let task_deadline: std::collections::HashMap<TaskId, u64> = replay_tasks
                .iter()
                .map(|task| (task.id, task.deadline))
                .collect();
            let task_priority: std::collections::HashMap<TaskId, u32> = replay_tasks
                .iter()
                .map(|task| (task.id, task.priority))
                .collect();

            engine.register_tasks(replay_tasks);

            for job in replay.jobs() {
                let task_id = TaskId(job.task_id);
                let priority = *task_priority
                    .get(&task_id)
                    .with_context(|| format!("replay job references unknown task {}", task_id.0))?;
                let relative_deadline = *task_deadline
                    .get(&task_id)
                    .with_context(|| format!("replay task {} missing deadline", task_id.0))?;
                let absolute_deadline = job
                    .absolute_deadline_ns
                    .unwrap_or_else(|| job.release_ns.saturating_add(relative_deadline));
                let job_id = engine.create_job(
                    task_id,
                    job.release_ns,
                    absolute_deadline,
                    job.actual_exec_ns,
                    priority,
                );
                let expected_version = engine.get_job(job_id).map_or(0, |job| job.version);
                engine.schedule_event(job.release_ns, EventKind::TaskArrival { task_id, job_id });
                let deadline_check_time = absolute_deadline.saturating_add(1);
                engine.schedule_event(
                    deadline_check_time,
                    EventKind::DeadlineCheck {
                        job_id,
                        expected_version,
                    },
                );
            }
        }
    }

    let mut scheduler = build_scheduler(scheduler_kind);
    let t0 = Instant::now();
    engine.run(scheduler.as_mut());
    let wall_us = t0.elapsed().as_micros() as u64;
    if let Some(path) = trace_output {
        engine
            .write_trace_jsonl(path)
            .with_context(|| format!("failed to write trace to {}", path.display()))?;
    }

    Ok((engine.summary(), wall_us))
}

fn cmd_run(cli: &Cli) -> anyhow::Result<()> {
    let platform_path = cli.platform.as_ref().context("--platform is required")?;
    let platform = PlatformConfig::load(platform_path).with_context(|| {
        format!(
            "failed to load platform config: {}",
            platform_path.display()
        )
    })?;

    let seed = cli.seed.unwrap_or(platform.simulation.seed);
    let workload = load_workload_input(cli)?;
    validate_analysis_mode_for_workload(cli.analysis_mode, &workload)?;
    let workload_meta = workload_metadata_snapshot(&workload);
    let analysis_meta = analysis_metadata_snapshot(cli.analysis_mode);
    if let WorkloadInput::Replay(replay) = &workload
        && !replay.metadata.assumptions.is_empty()
    {
        println!(
            "OpenMP approximation assumptions ({}):",
            replay.metadata.assumptions.len()
        );
        for assumption in &replay.metadata.assumptions {
            println!("  - {}: {}", assumption.code, assumption.detail);
        }
    }
    let (result, wall_us) = run_single(
        &platform,
        &workload,
        seed,
        cli.scheduler,
        cli.trace_output.as_deref(),
    )?;

    println!("HPRSS Simulation Summary");
    println!("algorithm      : {}", scheduler_label(cli.scheduler));
    println!("algorithm_key  : {}", scheduler_key(cli.scheduler));
    println!("algorithm_family: {}", scheduler_family(cli.scheduler));
    println!("analysis_mode  : {}", cli.analysis_mode.key());
    println!("analysis_scope : {}", analysis_meta.scope);
    println!(
        "analysis_assumption_count: {}",
        analysis_meta.assumption_count
    );
    if analysis_meta.assumption_count > 0 {
        println!("analysis_assumptions: {}", analysis_meta.assumptions_json);
    }
    println!("workload_source: {}", workload_meta.source);
    println!(
        "approximation_assumption_count: {}",
        workload_meta.assumption_count
    );
    if workload_meta.assumption_count > 0 {
        println!(
            "approximation_assumptions: {}",
            workload_meta.assumptions_json
        );
    }
    println!("total_jobs      : {}", result.total_jobs);
    println!("completed_jobs  : {}", result.completed_jobs);
    println!("deadline_misses : {}", result.deadline_misses);
    println!("miss_ratio      : {:.6}", result.miss_ratio);
    println!("schedulable     : {}", result.schedulable);
    println!("makespan_ns     : {}", result.makespan);
    println!("avg_response_ns : {:.3}", result.avg_response_time);
    println!("worst_response_ns: {}", result.worst_response_time);
    println!("transfer_overhead_ns: {}", result.transfer_overhead);
    println!("preemption_count : {}", result.preemption_count);
    println!("migration_count  : {}", result.migration_count);
    println!("bus_contention_ratio: {:.6}", result.bus_contention_ratio);
    println!("energy_total_joules: {:.9}", result.energy_total_joules);
    println!("events_processed: {}", result.events_processed);
    println!("wall_time_us    : {wall_us}");

    Ok(())
}

fn cmd_sweep(cli: &Cli, sweep: &SweepArgs) -> anyhow::Result<()> {
    if cli.replay_json.is_some() || cli.replay_csv_tasks.is_some() || cli.replay_csv_jobs.is_some()
    {
        anyhow::bail!("replay workload mode is only supported for single-run command");
    }

    let platform_path = cli.platform.as_ref().context("--platform is required")?;
    let platform = PlatformConfig::load(platform_path).with_context(|| {
        format!(
            "failed to load platform config: {}",
            platform_path.display()
        )
    })?;
    let run_config_hash = config_hash(platform_path)?;
    let run_git_commit = git_commit();
    let run_timestamp = run_timestamp();

    let utilizations = parse_range_f64(&sweep.utilizations)
        .map_err(|e| anyhow::anyhow!("bad --utilizations: {e}"))?;
    let task_counts: Vec<usize> = sweep
        .task_counts
        .split(',')
        .map(|s| s.trim().parse::<usize>())
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("bad --task-counts: {e}"))?;
    let seeds = parse_range_u64(&sweep.seeds).map_err(|e| anyhow::anyhow!("bad --seeds: {e}"))?;
    let schedulers = parse_scheduler_list(&sweep.schedulers)
        .map_err(|e| anyhow::anyhow!("bad --schedulers: {e}"))?;
    let analysis_modes = parse_analysis_mode_list(&sweep.analysis_modes)
        .map_err(|e| anyhow::anyhow!("bad --analysis-modes: {e}"))?;
    if analysis_modes.contains(&AnalysisMode::OpenmpWcrt) {
        anyhow::bail!(
            "analysis mode 'openmp-wcrt' is only supported for --openmp-specialized-json single-run input"
        );
    }

    // Build cartesian product of (utilization, task_count, seed, scheduler, analysis mode)
    let mut configs: Vec<(f64, usize, u64, SchedulerKind, AnalysisMode)> = Vec::new();
    for &u in &utilizations {
        for &t in &task_counts {
            for &s in &seeds {
                for &scheduler in &schedulers {
                    for &analysis_mode in &analysis_modes {
                        configs.push((u, t, s, scheduler, analysis_mode));
                    }
                }
            }
        }
    }

    let total = configs.len();
    eprintln!(
        "Sweep: {} experiments ({} utilizations × {} task counts × {} seeds × {} schedulers × {} analysis modes)",
        total,
        utilizations.len(),
        task_counts.len(),
        seeds.len(),
        schedulers.len(),
        analysis_modes.len(),
    );

    // Configure Rayon thread pool
    if sweep.jobs > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(sweep.jobs)
            .build_global()
            .ok();
    }

    let sweep_start = Instant::now();
    let completed = std::sync::atomic::AtomicUsize::new(0);

    let rows: Vec<SweepRow> = configs
        .par_iter()
        .filter_map(
            |&(utilization, task_count, seed, scheduler, analysis_mode)| {
                let workload = WorkloadInput::Synthetic {
                    num_tasks: task_count,
                    utilization,
                };
                let workload_meta = workload_metadata_snapshot(&workload);
                let analysis_meta = analysis_metadata_snapshot(analysis_mode);
                let result = run_single(&platform, &workload, seed, scheduler, None);
                let done = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if done.is_multiple_of(10) || done == total {
                    eprint!("\r  [{done}/{total}]");
                }
                match result {
                    Ok((sim, wall_us)) => Some(SweepRow {
                        utilization,
                        task_count,
                        seed,
                        algorithm: scheduler_label(scheduler).into(),
                        algorithm_key: scheduler_key(scheduler).into(),
                        algorithm_family: scheduler_family(scheduler).into(),
                        analysis_mode: analysis_mode.key().into(),
                        analysis_scope: analysis_meta.scope,
                        analysis_assumptions: analysis_meta.assumptions_json,
                        analysis_assumption_count: analysis_meta.assumption_count,
                        workload_source: workload_meta.source,
                        approximation_assumptions: workload_meta.assumptions_json,
                        approximation_assumption_count: workload_meta.assumption_count,
                        total_jobs: sim.total_jobs,
                        completed_jobs: sim.completed_jobs,
                        deadline_misses: sim.deadline_misses,
                        miss_ratio: sim.miss_ratio,
                        schedulable: sim.schedulable,
                        makespan: sim.makespan,
                        avg_response_time: sim.avg_response_time,
                        events_processed: sim.events_processed,
                        wall_time_us: wall_us,
                        config_hash: run_config_hash.clone(),
                        git_commit: run_git_commit.clone(),
                        timestamp: run_timestamp.clone(),
                        per_device_utilization: serialize_csv_json(&sim.per_device_utilization),
                        transfer_overhead: sim.transfer_overhead,
                        blocking_breakdown: serialize_csv_json(&sim.blocking_breakdown),
                        worst_response_time: sim.worst_response_time,
                        preemption_count: sim.preemption_count,
                        migration_count: sim.migration_count,
                        bus_contention_ratio: sim.bus_contention_ratio,
                        energy_total_joules: sim.energy_total_joules,
                    }),
                    Err(e) => {
                        eprintln!(
                            "\nWARN: u={utilization} tasks={task_count} seed={seed} failed: {e:#}"
                        );
                        None
                    }
                }
            },
        )
        .collect();

    eprintln!();
    let sweep_elapsed = sweep_start.elapsed();

    // Write CSV
    let mut wtr = csv::Writer::from_path(&sweep.output)
        .with_context(|| format!("failed to open {}", sweep.output.display()))?;
    for row in &rows {
        wtr.serialize(row)?;
    }
    wtr.flush()?;

    // Summary stats
    let schedulable_count = rows.iter().filter(|r| r.schedulable).count();
    let avg_miss: f64 = if rows.is_empty() {
        0.0
    } else {
        rows.iter().map(|r| r.miss_ratio).sum::<f64>() / rows.len() as f64
    };

    println!("Sweep complete");
    println!("  experiments : {}", rows.len());
    println!("  schedulable : {schedulable_count}/{}", rows.len());
    println!("  avg miss ratio: {avg_miss:.6}");
    println!("  wall time   : {:.2}s", sweep_elapsed.as_secs_f64());
    println!("  output      : {}", sweep.output.display());

    Ok(())
}

fn load_workload_input(cli: &Cli) -> anyhow::Result<WorkloadInput> {
    if let Some(path) = &cli.karami_profile_json {
        let replay = adapt_karami_paper_profile_json(path).with_context(|| {
            format!(
                "failed to adapt karami paper profile json: {}",
                path.display()
            )
        })?;
        return Ok(WorkloadInput::Replay(reindex_replay_task_ids(replay)));
    }

    if let Some(path) = &cli.openmp_specialized_json {
        let replay = adapt_openmp_specialized_json(path).with_context(|| {
            format!(
                "failed to adapt openmp specialized json: {}",
                path.display()
            )
        })?;
        return Ok(WorkloadInput::Replay(replay));
    }

    if let Some(path) = &cli.replay_json {
        let replay = load_replay_json(path)
            .with_context(|| format!("failed to load replay json: {}", path.display()))?;
        return Ok(WorkloadInput::Replay(replay));
    }

    if let (Some(tasks_path), Some(jobs_path)) = (&cli.replay_csv_tasks, &cli.replay_csv_jobs) {
        let replay = load_replay_csv(tasks_path, jobs_path).with_context(|| {
            format!(
                "failed to load replay csv tasks={} jobs={}",
                tasks_path.display(),
                jobs_path.display()
            )
        })?;
        return Ok(WorkloadInput::Replay(replay));
    }

    Ok(WorkloadInput::Synthetic {
        num_tasks: cli.tasks,
        utilization: cli.utilization,
    })
}

fn reindex_replay_task_ids(mut replay: ReplayWorkload) -> ReplayWorkload {
    let mut task_id_map = std::collections::HashMap::with_capacity(replay.tasks.len());
    for (idx, task) in replay.tasks.iter_mut().enumerate() {
        let new_id = idx as u32;
        task_id_map.insert(task.task_id, new_id);
        task.task_id = new_id;
    }
    for job in &mut replay.jobs {
        if let Some(new_id) = task_id_map.get(&job.task_id) {
            job.task_id = *new_id;
        }
    }
    replay
}

fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let result = match &cli.command {
        Some(Command::Sweep(sweep)) => cmd_sweep(&cli, sweep),
        Some(Command::Run) | None => cmd_run(&cli),
    };

    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Parser, ValueEnum};
    use std::collections::HashSet;

    #[derive(Debug, serde::Deserialize)]
    struct SurveyTaxonomyMatrix {
        classes: Vec<SurveyTaxonomyClass>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct SurveyTaxonomyClass {
        class_id: String,
        class_status: String,
        scheduler_mappings: Vec<SurveyTaxonomyMapping>,
        analysis_mappings: Vec<SurveyTaxonomyMapping>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct SurveyTaxonomyMapping {
        key: String,
        label: String,
        implementation_status: String,
    }

    #[test]
    fn cli_accepts_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "edf",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Edf);
    }

    #[test]
    fn cli_scheduler_defaults_to_fp() {
        let cli = Cli::try_parse_from(["hprss-sim", "--platform", "p.toml"])
            .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Fp);
    }

    #[test]
    fn cli_accepts_llf_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "llf",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Llf);
    }

    #[test]
    fn cli_accepts_edfvd_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "edfvd",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Edfvd);
    }

    #[test]
    fn cli_accepts_cpedf_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "cpedf",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Cpedf);
    }

    #[test]
    fn cli_accepts_federated_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "federated",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Federated);
    }

    #[test]
    fn cli_accepts_global_edf_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "global-edf",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::GlobalEdf);
    }

    #[test]
    fn cli_accepts_gang_scheduler_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--scheduler",
            "gang",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.scheduler, SchedulerKind::Gang);
    }

    #[test]
    fn cli_accepts_new_scheduler_switches() {
        for (flag, expected) in [
            ("xsched", SchedulerKind::Xsched),
            ("gcaps", SchedulerKind::Gcaps),
            ("gpreempt", SchedulerKind::Gpreempt),
            ("rtgpu", SchedulerKind::Rtgpu),
            ("match", SchedulerKind::Match),
            (
                "gpu-preemptive-priority",
                SchedulerKind::GpuPreemptivePriority,
            ),
        ] {
            let cli = Cli::try_parse_from([
                "hprss-sim",
                "--platform",
                "configs/platform_ft2000_full.toml",
                "--scheduler",
                flag,
            ])
            .expect("cli parsing should succeed");
            assert_eq!(cli.scheduler, expected);
        }
    }

    #[test]
    fn parse_scheduler_list_supports_multiple_algorithms() {
        let parsed = parse_scheduler_list(
            "fp,edf,edfvd,llf,heft,cpedf,federated,global-edf,gang,xsched,gcaps,gpreempt,rtgpu,match,gpu-preemptive-priority",
        )
            .expect("parse should succeed");
        assert_eq!(
            parsed,
            vec![
                SchedulerKind::Fp,
                SchedulerKind::Edf,
                SchedulerKind::Edfvd,
                SchedulerKind::Llf,
                SchedulerKind::Heft,
                SchedulerKind::Cpedf,
                SchedulerKind::Federated,
                SchedulerKind::GlobalEdf,
                SchedulerKind::Gang,
                SchedulerKind::Xsched,
                SchedulerKind::Gcaps,
                SchedulerKind::Gpreempt,
                SchedulerKind::Rtgpu,
                SchedulerKind::Match,
                SchedulerKind::GpuPreemptivePriority
            ]
        );
    }

    #[test]
    fn parse_scheduler_list_rejects_unknown_name() {
        let err = parse_scheduler_list("fp,abc").expect_err("parse should fail");
        assert!(err.contains("unknown scheduler"));
    }

    #[test]
    fn cli_accepts_analysis_mode_switch() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--analysis-mode",
            "rta-uniform-global-fp-scaffold",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(cli.analysis_mode, AnalysisMode::RtaUniformGlobalFpScaffold);
    }

    #[test]
    fn parse_analysis_mode_list_supports_aliases() {
        let parsed = parse_analysis_mode_list(
            "none,rta-uniprocessor-fp,uniform-global-fp-scaffold,rta_uniform_global_fp_scaffold,util-vectors,uniform-rta,conditional-dag,shape,openmp_wcrt,simso-extension",
        )
        .expect("parse should succeed");
        assert_eq!(
            parsed,
            vec![
                AnalysisMode::None,
                AnalysisMode::RtaUniprocessorFp,
                AnalysisMode::RtaUniformGlobalFpScaffold,
                AnalysisMode::RtaUniformGlobalFpScaffold,
                AnalysisMode::UtilVectors,
                AnalysisMode::UniformRtaGlobalFp,
                AnalysisMode::ConditionalDag,
                AnalysisMode::Shape,
                AnalysisMode::OpenmpWcrt,
                AnalysisMode::SimsoScopeExtension
            ]
        );
    }

    #[test]
    fn cli_accepts_trace_output_path() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--trace-output",
            "/tmp/trace.jsonl",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(
            cli.trace_output.as_deref(),
            Some(std::path::Path::new("/tmp/trace.jsonl"))
        );
    }

    #[test]
    fn sweep_row_includes_paper_metrics_and_repro_metadata() {
        let row = SweepRow {
            utilization: 0.7,
            task_count: 16,
            seed: 11,
            algorithm: "FP-Het".to_string(),
            algorithm_key: "fp".to_string(),
            algorithm_family: "fixed-priority".to_string(),
            analysis_mode: "none".to_string(),
            analysis_scope: "none".to_string(),
            analysis_assumptions: "[]".to_string(),
            analysis_assumption_count: 0,
            workload_source: "synthetic".to_string(),
            approximation_assumptions: "[]".to_string(),
            approximation_assumption_count: 0,
            total_jobs: 100,
            completed_jobs: 95,
            deadline_misses: 5,
            miss_ratio: 0.05,
            schedulable: false,
            makespan: 9_000,
            avg_response_time: 450.0,
            events_processed: 123,
            wall_time_us: 77,
            config_hash: "abc123".to_string(),
            git_commit: "deadbeef".to_string(),
            timestamp: "2026-04-08T10:15:00Z".to_string(),
            per_device_utilization: "[{\"device_id\":0,\"busy_ns\":1000,\"utilization\":0.5}]"
                .to_string(),
            transfer_overhead: 2048,
            blocking_breakdown: "{\"transfer_ns\":2048,\"migration_ns\":512,\"bus_wait_ns\":64}"
                .to_string(),
            worst_response_time: 1000,
            preemption_count: 3,
            migration_count: 2,
            bus_contention_ratio: 0.25,
            energy_total_joules: 0.123,
        };

        let mut writer = csv::Writer::from_writer(vec![]);
        writer.serialize(&row).expect("row should serialize");
        let output = String::from_utf8(
            writer
                .into_inner()
                .expect("csv writer should provide underlying buffer"),
        )
        .expect("csv output should be utf8");

        assert!(output.contains("makespan"));
        assert!(output.contains("avg_response_time"));
        assert!(output.contains("algorithm_key"));
        assert!(output.contains("algorithm_family"));
        assert!(output.contains("analysis_mode"));
        assert!(output.contains("analysis_scope"));
        assert!(output.contains("analysis_assumptions"));
        assert!(output.contains("analysis_assumption_count"));
        assert!(output.contains("workload_source"));
        assert!(output.contains("approximation_assumptions"));
        assert!(output.contains("approximation_assumption_count"));
        assert!(output.contains("config_hash"));
        assert!(output.contains("git_commit"));
        assert!(output.contains("timestamp"));
        assert!(output.contains("per_device_utilization"));
        assert!(output.contains("transfer_overhead"));
        assert!(output.contains("blocking_breakdown"));
        assert!(output.contains("worst_response_time"));
        assert!(output.contains("preemption_count"));
        assert!(output.contains("migration_count"));
        assert!(output.contains("bus_contention_ratio"));
        assert!(output.contains("energy_total_joules"));
    }

    #[test]
    fn reproducibility_metadata_helpers_produce_values() {
        let cargo_toml = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let hash = config_hash(&cargo_toml).expect("config hash should be generated");
        assert!(!hash.is_empty());
        assert!(!git_commit().is_empty());
        assert!(!run_timestamp().is_empty());
    }

    #[test]
    fn metric_json_columns_are_round_trip_json_with_precision() {
        #[derive(serde::Serialize)]
        struct UtilRow {
            device_id: u32,
            busy_ns: u64,
            utilization: f64,
        }
        #[derive(serde::Serialize)]
        struct BlockingRow {
            transfer_ns: u64,
            migration_ns: u64,
            bus_wait_ns: u64,
        }

        let per_device = serialize_csv_json(&[UtilRow {
            device_id: 7,
            busy_ns: 500,
            utilization: 1.0 / 3.0,
        }]);
        let parsed_per_device: serde_json::Value =
            serde_json::from_str(&per_device).expect("must be valid json");
        let utilization = parsed_per_device[0]["utilization"]
            .as_f64()
            .expect("utilization should be numeric");
        assert!(
            (utilization - (1.0 / 3.0)).abs() < 1e-12,
            "serialization should preserve floating precision"
        );

        let blocking = serialize_csv_json(&BlockingRow {
            transfer_ns: 120,
            migration_ns: 30,
            bus_wait_ns: 10,
        });
        let parsed_blocking: serde_json::Value =
            serde_json::from_str(&blocking).expect("must be valid json");
        assert_eq!(parsed_blocking["transfer_ns"].as_u64(), Some(120));
        assert_eq!(parsed_blocking["migration_ns"].as_u64(), Some(30));
        assert_eq!(parsed_blocking["bus_wait_ns"].as_u64(), Some(10));
    }

    #[test]
    fn cli_accepts_openmp_specialized_json_mode() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--openmp-specialized-json",
            "openmp.json",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(
            cli.openmp_specialized_json.as_deref(),
            Some(std::path::Path::new("openmp.json"))
        );
        assert!(cli.replay_json.is_none());
        assert!(cli.replay_csv_tasks.is_none());
        assert!(cli.replay_csv_jobs.is_none());
        assert!(cli.karami_profile_json.is_none());
    }

    #[test]
    fn cli_accepts_karami_profile_json_mode() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--karami-profile-json",
            "karami.json",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(
            cli.karami_profile_json.as_deref(),
            Some(std::path::Path::new("karami.json"))
        );
        assert!(cli.replay_json.is_none());
        assert!(cli.replay_csv_tasks.is_none());
        assert!(cli.replay_csv_jobs.is_none());
        assert!(cli.openmp_specialized_json.is_none());
    }

    #[test]
    fn cli_accepts_replay_json_mode() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--replay-json",
            "replay.json",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(
            cli.replay_json.as_deref(),
            Some(std::path::Path::new("replay.json"))
        );
        assert!(cli.replay_csv_tasks.is_none());
        assert!(cli.replay_csv_jobs.is_none());
        assert!(cli.openmp_specialized_json.is_none());
        assert!(cli.karami_profile_json.is_none());
    }

    #[test]
    fn cli_accepts_replay_csv_mode() {
        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--replay-csv-tasks",
            "tasks.csv",
            "--replay-csv-jobs",
            "jobs.csv",
        ])
        .expect("cli parsing should succeed");
        assert_eq!(
            cli.replay_csv_tasks.as_deref(),
            Some(std::path::Path::new("tasks.csv"))
        );
        assert_eq!(
            cli.replay_csv_jobs.as_deref(),
            Some(std::path::Path::new("jobs.csv"))
        );
        assert!(cli.replay_json.is_none());
        assert!(cli.openmp_specialized_json.is_none());
        assert!(cli.karami_profile_json.is_none());
    }

    #[test]
    fn cli_rejects_karami_combined_with_openmp_input() {
        let err = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--karami-profile-json",
            "karami.json",
            "--openmp-specialized-json",
            "openmp.json",
        ])
        .expect_err("cli parsing should fail");
        let msg = err.to_string();
        assert!(msg.contains("--karami-profile-json"));
    }

    #[test]
    fn cli_rejects_karami_combined_with_replay_input() {
        let err = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--karami-profile-json",
            "karami.json",
            "--replay-json",
            "replay.json",
        ])
        .expect_err("cli parsing should fail");
        let msg = err.to_string();
        assert!(msg.contains("--karami-profile-json"));
    }

    #[test]
    fn openmp_specialized_cli_path_loads_workload_metadata() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let openmp_json = workspace_root
            .join("crates/hprss-workload/tests/fixtures/openmp_specialized_sample.json");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--openmp-specialized-json",
            openmp_json.to_str().expect("utf8 path"),
        ])
        .expect("cli parsing should succeed");

        let workload = load_workload_input(&cli).expect("openmp workload should load");
        match workload {
            WorkloadInput::Replay(replay) => {
                assert_eq!(replay.tasks.len(), 1);
                assert_eq!(replay.jobs().len(), 2);
                assert!(
                    replay
                        .metadata
                        .assumptions
                        .iter()
                        .any(|entry| entry.code == "omp-missing-observed-exec")
                );
            }
            WorkloadInput::Synthetic { .. } => panic!("expected replay workload"),
        }
    }

    #[test]
    fn openmp_specialized_cli_path_runs_with_openmp_schedulers() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let openmp_json = workspace_root
            .join("crates/hprss-workload/tests/fixtures/openmp_specialized_sample.json");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--openmp-specialized-json",
            openmp_json.to_str().expect("utf8 path"),
        ])
        .expect("cli parsing should succeed");
        let workload = load_workload_input(&cli).expect("openmp workload should load");
        let platform = PlatformConfig::load(&platform_path)
            .expect("platform fixture should load for openmp replay test");
        let seed = cli.seed.unwrap_or(platform.simulation.seed);

        for scheduler in [SchedulerKind::GlobalEdf, SchedulerKind::Gang] {
            let (result, _) = run_single(&platform, &workload, seed, scheduler, None)
                .expect("openmp replay run should succeed");
            assert_eq!(result.total_jobs, 2);
            assert_eq!(
                result.completed_jobs + result.deadline_misses,
                result.total_jobs,
                "openmp replay jobs should complete or miss deadline without panicking"
            );
        }
    }

    #[test]
    fn karami_profile_cli_path_loads_workload_metadata() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let karami_json =
            workspace_root.join("crates/hprss-workload/tests/fixtures/karami_profile_sample.json");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--karami-profile-json",
            karami_json.to_str().expect("utf8 path"),
        ])
        .expect("cli parsing should succeed");

        let workload = load_workload_input(&cli).expect("karami workload should load");
        match workload {
            WorkloadInput::Replay(replay) => {
                assert_eq!(replay.tasks.len(), 2);
                assert_eq!(replay.jobs().len(), 5);
                let task_ids: Vec<u32> = replay.tasks.iter().map(|task| task.task_id).collect();
                assert_eq!(task_ids, vec![0, 1]);
                assert!(replay.jobs().iter().all(|job| job.task_id <= 1));
                assert!(
                    replay
                        .metadata
                        .source
                        .as_deref()
                        .is_some_and(|src| src.contains("karami-paper-profile"))
                );
                assert!(
                    replay
                        .metadata
                        .assumptions
                        .iter()
                        .any(|entry| entry.code == "karami-paper-profile")
                );
            }
            WorkloadInput::Synthetic { .. } => panic!("expected replay workload"),
        }
    }

    #[test]
    fn karami_profile_cli_path_runs_with_stable_schedulers() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let karami_json =
            workspace_root.join("crates/hprss-workload/tests/fixtures/karami_profile_sample.json");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--karami-profile-json",
            karami_json.to_str().expect("utf8 path"),
        ])
        .expect("cli parsing should succeed");
        let workload = load_workload_input(&cli).expect("karami workload should load");
        let platform = PlatformConfig::load(&platform_path)
            .expect("platform fixture should load for karami replay test");
        let seed = cli.seed.unwrap_or(platform.simulation.seed);

        for scheduler in [SchedulerKind::GlobalEdf, SchedulerKind::Gang] {
            let (result, _) = run_single(&platform, &workload, seed, scheduler, None)
                .expect("karami replay run should succeed");
            assert_eq!(result.total_jobs, 5);
            assert_eq!(
                result.completed_jobs + result.deadline_misses,
                result.total_jobs,
                "karami replay jobs should complete or miss deadline without panicking"
            );
        }
    }

    #[test]
    fn cli_rejects_partial_replay_csv_mode() {
        let err = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            "configs/platform_ft2000_full.toml",
            "--replay-csv-tasks",
            "tasks.csv",
        ])
        .expect_err("cli parsing should fail");
        let msg = err.to_string();
        assert!(msg.contains("--replay-csv-jobs"));
    }

    #[test]
    fn replay_csv_cli_path_loads_and_runs_deterministically() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let replay_tasks =
            workspace_root.join("crates/hprss-workload/tests/fixtures/replay_tasks.csv");
        let replay_jobs =
            workspace_root.join("crates/hprss-workload/tests/fixtures/replay_jobs.csv");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--replay-csv-tasks",
            replay_tasks.to_str().expect("utf8 path"),
            "--replay-csv-jobs",
            replay_jobs.to_str().expect("utf8 path"),
            "--scheduler",
            "fp",
        ])
        .expect("cli parsing should succeed");

        let workload = load_workload_input(&cli).expect("replay workload should load");
        let platform = PlatformConfig::load(&platform_path)
            .expect("platform fixture should load for replay test");
        let seed = cli.seed.unwrap_or(platform.simulation.seed);

        let (first, _) = run_single(&platform, &workload, seed, cli.scheduler, None)
            .expect("first replay run should succeed");
        let (second, _) = run_single(&platform, &workload, seed, cli.scheduler, None)
            .expect("second replay run should succeed");

        assert_eq!(first.total_jobs, 2);
        assert_eq!(
            first.completed_jobs + first.deadline_misses,
            first.total_jobs,
            "replay jobs should resolve as completed or deadline-missed"
        );
        assert_eq!(first.total_jobs, second.total_jobs);
        assert_eq!(first.completed_jobs, second.completed_jobs);
        assert_eq!(first.deadline_misses, second.deadline_misses);
        assert_eq!(first.makespan, second.makespan);
        assert_eq!(first.transfer_overhead, second.transfer_overhead);
    }

    #[test]
    fn replay_json_cli_path_accounts_for_deadline_miss() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let platform_path = workspace_root.join("configs/platform_ft2000_full.toml");
        let replay_json =
            workspace_root.join("crates/hprss-workload/tests/fixtures/replay_deadline_miss.json");

        let cli = Cli::try_parse_from([
            "hprss-sim",
            "--platform",
            platform_path.to_str().expect("utf8 path"),
            "--replay-json",
            replay_json.to_str().expect("utf8 path"),
            "--scheduler",
            "fp",
        ])
        .expect("cli parsing should succeed");

        let workload = load_workload_input(&cli).expect("replay workload should load");
        let platform = PlatformConfig::load(&platform_path)
            .expect("platform fixture should load for replay test");
        let seed = cli.seed.unwrap_or(platform.simulation.seed);

        let (result, _) = run_single(&platform, &workload, seed, cli.scheduler, None)
            .expect("run should succeed");

        assert_eq!(result.total_jobs, 1);
        assert!(
            result.deadline_misses > 0,
            "replay job should be counted as deadline miss"
        );
    }

    #[test]
    fn survey_taxonomy_matrix_stays_in_sync_with_exports() {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root should resolve");
        let matrix_path = workspace_root.join("docs/superpowers/specs/survey-taxonomy-matrix.json");
        let raw = std::fs::read_to_string(&matrix_path).unwrap_or_else(|err| {
            panic!(
                "matrix file missing or unreadable at {}: {err}",
                matrix_path.display()
            )
        });
        let matrix: SurveyTaxonomyMatrix =
            serde_json::from_str(&raw).expect("survey taxonomy matrix must be valid JSON");

        let allowed_status = ["implemented", "partial", "missing", "unsupported"];

        let mut class_ids = HashSet::new();
        let mut mapped_scheduler_keys = HashSet::new();
        let mut mapped_analysis_keys = HashSet::new();

        let exported_scheduler_variants = SchedulerKind::value_variants();
        let exported_scheduler_keys: HashSet<&'static str> = exported_scheduler_variants
            .iter()
            .map(|kind| scheduler_key(*kind))
            .collect();
        let exported_analysis_keys: HashSet<&'static str> = AnalysisMode::value_variants()
            .iter()
            .map(|mode| mode.key())
            .collect();

        for class in &matrix.classes {
            assert!(
                class_ids.insert(class.class_id.as_str()),
                "duplicate class_id in matrix: {}",
                class.class_id
            );
            assert!(
                allowed_status.contains(&class.class_status.as_str()),
                "invalid class_status '{}' in class '{}'",
                class.class_status,
                class.class_id
            );

            for scheduler in &class.scheduler_mappings {
                assert!(
                    allowed_status.contains(&scheduler.implementation_status.as_str()),
                    "invalid implementation_status '{}' for scheduler '{}' in class '{}'",
                    scheduler.implementation_status,
                    scheduler.key,
                    class.class_id
                );
                assert!(
                    exported_scheduler_keys.contains(scheduler.key.as_str()),
                    "unknown scheduler key '{}' in class '{}'",
                    scheduler.key,
                    class.class_id
                );
                assert!(
                    mapped_scheduler_keys.insert(scheduler.key.as_str()),
                    "scheduler key '{}' appears more than once in matrix",
                    scheduler.key
                );
            }

            for analysis in &class.analysis_mappings {
                assert!(
                    allowed_status.contains(&analysis.implementation_status.as_str()),
                    "invalid implementation_status '{}' for analysis '{}' in class '{}'",
                    analysis.implementation_status,
                    analysis.key,
                    class.class_id
                );
                assert!(
                    exported_analysis_keys.contains(analysis.key.as_str()),
                    "unknown analysis key '{}' in class '{}'",
                    analysis.key,
                    class.class_id
                );
                assert!(
                    mapped_analysis_keys.insert(analysis.key.as_str()),
                    "analysis key '{}' appears more than once in matrix",
                    analysis.key
                );
            }
        }

        for kind in exported_scheduler_variants {
            let key = scheduler_key(*kind);
            let label = scheduler_label(*kind);
            assert!(
                matrix
                    .classes
                    .iter()
                    .flat_map(|c| &c.scheduler_mappings)
                    .any(|m| { m.key == key && m.label == label }),
                "matrix missing scheduler mapping for key '{key}' with label '{label}'"
            );
        }

        assert_eq!(
            mapped_scheduler_keys, exported_scheduler_keys,
            "scheduler mappings must exactly match scheduler exports"
        );
        assert_eq!(
            mapped_analysis_keys, exported_analysis_keys,
            "analysis mappings must exactly match analysis mode exports"
        );
    }
}
