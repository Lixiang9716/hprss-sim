# HPRSS-Sim Performance Optimization Design

**Date**: 2026-04-08
**Goal**: Scale single simulation to thousands of tasks / millions of events, then parallelize batch experiments.

## Context

Current state: ~30 tasks, 10K events, <100ms. No bottleneck today, but architecture must scale to 1000+ tasks / 1M+ events for thesis experiments.

PDES (Approach A) was evaluated and deferred — the centralized `Scheduler` trait (`&mut self` on 5/7 event types) limits device-partitioned parallelism to ~1.5x. We pursue data-structure optimization first (Approach C), then revisit PDES if needed.

## Phase 1: Profiling Baseline

### 1.1 Large-Scale Benchmark Scenario

Create a dedicated benchmark binary/test that generates a stress workload:

- **500 tasks** across 4 device types (CPU-heavy mix)
- **Utilization**: 0.85 (high but schedulable)
- **Duration**: 100ms simulated = ~100+ job releases per task
- **Expected**: 50K+ jobs, 500K+ events

File: `crates/hprss-engine/benches/engine_bench.rs` using Criterion.

### 1.2 Flamegraph

Install `cargo-flamegraph`, run the benchmark scenario under `perf`, generate SVG flamegraph. This identifies actual CPU hotspots before we optimize.

### 1.3 Baseline Metrics

Record wall-clock time for the benchmark at key optimization points:
- Before any optimization (baseline)
- After each optimization phase

## Phase 2: Data Structure Optimizations

Priority order to be confirmed by Phase 1 profiling. Expected candidates:

### 2.1 Slab-Based Job Storage

**Current**: `Vec<Option<Job>>` indexed by `JobId.0`. Grows unboundedly, holes from completed jobs waste cache lines.

**Proposed**: Use `slab` crate (or custom arena).

- `Slab<Job>` gives O(1) insert (returns key), O(1) remove, O(1) access
- Compact storage — removed slots are reused
- Better cache locality for iteration
- `JobId` wraps the slab key

**Impact on API**: `get_job(id) -> Option<&Job>` stays the same. `JobId` creation changes from pre-allocated to slab-assigned.

### 2.2 Indexed Scheduler Ready Queues

**Current**: `FixedPriorityScheduler` likely does O(N) scan of ready queue per device to find highest-priority job.

**Proposed**: Per-device `BTreeMap<Reverse<u32>, VecDeque<JobId>>` — ordered by priority descending.

- `highest_priority_job()`: O(1) via `first_entry()`
- `insert()`: O(log K) where K = number of distinct priority levels (typically small, <20)
- `remove(job_id)`: O(K) worst case, but rare

Note: This requires modifying `hprss-scheduler`. The `Scheduler` trait itself stays unchanged — the optimization is inside `FixedPriorityScheduler`'s internal data structures.

### 2.3 Partitioned Ready Queues (DeviceManager)

**Current**: DeviceManager has per-device `Vec<JobId>` ready queues, sorted by priority on each insert.

**Proposed**: Replace with `BinaryHeap<(Reverse<u32>, JobId)>` per device for O(log N) insert instead of O(N) sorted insert. Or `BTreeMap` for O(log N) insert + O(1) peek.

### 2.4 Event Queue Optimization

**Current**: `BinaryHeap<Event>` — standard. For millions of events, consider:

- **Calendar Queue**: O(1) amortized insert/remove for events with bounded time spread. Classic DES optimization.
- **Decision**: Only if profiling shows event queue as hotspot. `BinaryHeap` at O(log N) for N=1M ≈ 20 comparisons per op is likely fine.

## Phase 3: Rayon Batch Parallelism

### 3.1 Experiment Configuration

New struct `ExperimentSweep`:

```rust
struct ExperimentSweep {
    platform: PathBuf,
    utilizations: Vec<f64>,     // e.g., [0.1, 0.2, ..., 1.0]
    task_counts: Vec<usize>,    // e.g., [10, 50, 100, 500]
    seeds: RangeInclusive<u64>, // e.g., 1..=100
    algorithms: Vec<String>,    // e.g., ["fp", "edf"]
}
```

Total experiments = |utilizations| × |task_counts| × |seeds| × |algorithms|.

### 3.2 CLI Extension

New subcommand or flag:

```
hprss-sim sweep \
  --platform configs/platform_ft2000_full.toml \
  --utilization 0.1:0.1:1.0 \
  --tasks 10,50,100,500 \
  --seeds 1:100 \
  --algorithms fp \
  --output results.csv \
  --jobs 0  # 0 = use all CPU cores
```

### 3.3 Parallel Execution

```rust
use rayon::prelude::*;

let results: Vec<SimResult> = experiment_configs
    .par_iter()
    .map(|cfg| {
        let mut engine = SimEngine::new(...);
        let mut scheduler = make_scheduler(&cfg.algorithm);
        engine.run(&mut scheduler);
        engine.summary()
    })
    .collect();
```

`SimEngine` is self-contained (no global state), so each instance runs in its own thread safely.

### 3.4 Output Format

CSV with columns: `utilization, task_count, seed, algorithm, total_jobs, completed_jobs, deadline_misses, miss_ratio, schedulable, events_processed, wall_time_us`.

## Phase 4 (Future): PDES Exploration

Deferred. After Phase 2-3, re-evaluate:

- If single-sim wall time for 1K tasks is <1s after optimization → PDES not needed
- If still >5s → consider Conservative PDES with device partitioning
- Key prerequisite: make `Scheduler` trait support concurrent access (e.g., per-device scheduler instances)

## Non-Goals

- Optimistic Time Warp PDES (too complex for this simulator)
- GPU-accelerated simulation
- Distributed simulation across machines
- Changing the `Scheduler` trait signature (it's shared with future hardware)

## Dependencies

New crates:
- `slab` (arena allocator)
- `rayon` (parallel iterators)
- `criterion` (benchmarking)
- `csv` (output)

## Success Criteria

1. Benchmark established with reproducible measurements
2. Flamegraph generated, hotspots identified
3. At least 2x single-sim speedup on 500-task benchmark after Phase 2
4. Batch mode processes 1000 experiments using all CPU cores
5. Zero change to simulation correctness (all existing tests pass)
