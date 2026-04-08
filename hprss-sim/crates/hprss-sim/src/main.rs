use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use hprss_engine::engine::{SimConfig, SimEngine, SimResult};
use hprss_platform::PlatformConfig;
use hprss_scheduler::{
    EdfScheduler, EdfVdScheduler, FixedPriorityScheduler, HeftScheduler, LlfScheduler,
};
use hprss_types::Scheduler;
use hprss_workload::{WorkloadConfig, generate_taskset};
use rayon::prelude::*;
use tracing_subscriber::EnvFilter;

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

    /// Write simulation trace as JSON-lines
    #[arg(long, global = true)]
    trace_output: Option<PathBuf>,
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

    /// Comma-separated algorithms (fp,edf,edfvd,llf,heft)
    #[arg(long, default_value = "fp,edf,edfvd,llf,heft")]
    schedulers: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SchedulerKind {
    Fp,
    Edf,
    Edfvd,
    Llf,
    Heft,
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

fn parse_scheduler_list(s: &str) -> Result<Vec<SchedulerKind>, String> {
    let mut out = Vec::new();
    for token in s.split(',').map(|v| v.trim()).filter(|v| !v.is_empty()) {
        let kind = match token.to_ascii_lowercase().as_str() {
            "fp" => SchedulerKind::Fp,
            "edf" => SchedulerKind::Edf,
            "edfvd" => SchedulerKind::Edfvd,
            "llf" => SchedulerKind::Llf,
            "heft" => SchedulerKind::Heft,
            _ => return Err(format!("unknown scheduler: {token}")),
        };
        out.push(kind);
    }
    if out.is_empty() {
        return Err("scheduler list is empty".to_string());
    }
    Ok(out)
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
fn run_single(
    platform: &PlatformConfig,
    num_tasks: usize,
    utilization: f64,
    seed: u64,
    scheduler_kind: SchedulerKind,
    trace_output: Option<&std::path::Path>,
) -> anyhow::Result<(SimResult, u64)> {
    let devices = platform.build_devices()?;
    let interconnects = platform.build_interconnects(&devices)?;
    let buses = platform.build_buses()?;

    let workload_cfg = WorkloadConfig {
        num_tasks,
        total_utilization: utilization,
        seed,
        ..WorkloadConfig::default()
    };
    let tasks = generate_taskset(&workload_cfg, &devices);

    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: platform.duration_ns(),
            seed,
        },
        devices,
        interconnects,
        buses,
    );
    engine.register_tasks(tasks);
    engine.schedule_initial_arrivals();

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
    let (result, wall_us) = run_single(
        &platform,
        cli.tasks,
        cli.utilization,
        seed,
        cli.scheduler,
        cli.trace_output.as_deref(),
    )?;

    println!("HPRSS Simulation Summary");
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

    // Build cartesian product of (utilization, task_count, seed, scheduler)
    let mut configs: Vec<(f64, usize, u64, SchedulerKind)> = Vec::new();
    for &u in &utilizations {
        for &t in &task_counts {
            for &s in &seeds {
                for &scheduler in &schedulers {
                    configs.push((u, t, s, scheduler));
                }
            }
        }
    }

    let total = configs.len();
    eprintln!(
        "Sweep: {} experiments ({} utilizations × {} task counts × {} seeds)",
        total,
        utilizations.len(),
        task_counts.len(),
        seeds.len(),
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
        .filter_map(|&(utilization, task_count, seed, scheduler)| {
            let result = run_single(&platform, task_count, utilization, seed, scheduler, None);
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
        })
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

fn build_scheduler(kind: SchedulerKind) -> Box<dyn Scheduler> {
    match kind {
        SchedulerKind::Fp => Box::new(FixedPriorityScheduler),
        SchedulerKind::Edf => Box::new(EdfScheduler),
        SchedulerKind::Edfvd => Box::new(EdfVdScheduler::default()),
        SchedulerKind::Llf => Box::new(LlfScheduler::default()),
        SchedulerKind::Heft => Box::new(HeftScheduler::default()),
    }
}

fn scheduler_label(kind: SchedulerKind) -> &'static str {
    match kind {
        SchedulerKind::Fp => "FP-Het",
        SchedulerKind::Edf => "EDF-Het",
        SchedulerKind::Edfvd => "EDF-VD-Het",
        SchedulerKind::Llf => "LLF-Het",
        SchedulerKind::Heft => "HEFT",
    }
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
    use clap::Parser;

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
    fn parse_scheduler_list_supports_multiple_algorithms() {
        let parsed = parse_scheduler_list("fp,edf,edfvd,llf,heft").expect("parse should succeed");
        assert_eq!(
            parsed,
            vec![
                SchedulerKind::Fp,
                SchedulerKind::Edf,
                SchedulerKind::Edfvd,
                SchedulerKind::Llf,
                SchedulerKind::Heft
            ]
        );
    }

    #[test]
    fn parse_scheduler_list_rejects_unknown_name() {
        let err = parse_scheduler_list("fp,abc").expect_err("parse should fail");
        assert!(err.contains("unknown scheduler"));
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
}
