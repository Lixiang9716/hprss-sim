use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_platform::PlatformConfig;
use hprss_scheduler::FixedPriorityScheduler;
use hprss_workload::{WorkloadConfig, generate_taskset};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "hprss-sim", about = "Run HPRSS heterogeneous DES simulation")]
struct Args {
    /// Platform TOML config path
    #[arg(long)]
    platform: PathBuf,

    /// Number of tasks to generate
    #[arg(long, default_value_t = 10)]
    tasks: usize,

    /// Target total utilization
    #[arg(long, default_value_t = 0.6)]
    utilization: f64,

    /// Override random seed from platform config
    #[arg(long)]
    seed: Option<u64>,

    /// Enable verbose trace logs
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

fn init_tracing(verbose: bool) {
    let default_level = if verbose { "trace" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    init_tracing(args.verbose);

    let platform = PlatformConfig::load(&args.platform).with_context(|| {
        format!(
            "failed to load platform config: {}",
            args.platform.display()
        )
    })?;
    let devices = platform
        .build_devices()
        .context("failed to build devices")?;
    let interconnects = platform
        .build_interconnects(&devices)
        .context("failed to build interconnects")?;
    let buses = platform
        .build_buses()
        .context("failed to build shared buses")?;

    let seed = args.seed.unwrap_or(platform.simulation.seed);
    let workload_cfg = WorkloadConfig {
        num_tasks: args.tasks,
        total_utilization: args.utilization,
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

    let mut scheduler = FixedPriorityScheduler;
    engine.run(&mut scheduler);

    let metrics = engine.metrics();
    println!("HPRSS Simulation Summary");
    println!("total_jobs      : {}", metrics.total_jobs);
    println!("completed_jobs  : {}", metrics.completed_jobs);
    println!("deadline_misses : {}", metrics.deadline_misses);
    println!("miss_ratio      : {:.6}", metrics.miss_ratio());
    println!("schedulable     : {}", metrics.is_schedulable());
    println!("events_processed: {}", engine.events_processed());

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
