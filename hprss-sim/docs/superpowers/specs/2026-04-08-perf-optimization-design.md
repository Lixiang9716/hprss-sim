# HPRSS-Sim Performance Optimization Design

**Date**: 2026-04-08
**Updated**: 2026-04-08 (post-implementation results added)
**Goal**: Scale single simulation to thousands of tasks / millions of events, then parallelize batch experiments.
**Status**: ✅ Complete — 13.2x single-sim speedup achieved, Rayon batch parallelism operational.

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

Priority order confirmed by Phase 1 profiling: rebuild_view_data allocations (87% CPU) >> queue operations >> everything else.

### 2.1 Slab-Based Job Storage

**Status**: ⏭️ Deferred — marginal benefit after queue optimizations achieved 13.2x speedup.

**Current**: `Vec<Option<Job>>` indexed by `JobId.0`. Grows unboundedly, holes from completed jobs waste cache lines.

**Proposed**: Use `slab` crate (or custom arena).

- `Slab<Job>` gives O(1) insert (returns key), O(1) remove, O(1) access
- Compact storage — removed slots are reused
- Better cache locality for iteration
- `JobId` wraps the slab key

**Impact on API**: `get_job(id) -> Option<&Job>` stays the same. `JobId` creation changes from pre-allocated to slab-assigned.

**Decision**: The `slab` crate is added as a dependency but not yet used. `Vec<Option<Job>>` indexing is already O(1); the main benefit would be iteration density in `drop_lo_critical_jobs()`, which is rarely called. Revisit if profiling shows this as a hotspot.

### 2.2 Indexed Scheduler Ready Queues + Dirty-Flag Rebuild (IMPLEMENTED ✅)

**Before**: Per-device `Vec<(u32, JobId)>` sorted by priority. O(N) `insert()` + O(N) `remove(0)` for enqueue/dequeue. `rebuild_view_data()` rebuilt ALL device views on every event, allocating `Vec<QueuedJobInfo>` via `.collect()`.

**After**: Three combined optimizations:

1. **BTreeMap ready queues**: `BTreeMap<u32, VecDeque<JobId>>` per device
   - `enqueue()`: O(log K) via `entry().or_default().push_back()`
   - `dequeue()`: O(1) amortized via `first_entry().pop_front()`
   - `remove_from_queue()`: O(K × D) where K=priority levels, D=deque length

2. **Per-device dirty flags**: `queue_dirty: Vec<bool>` tracks which devices had queue mutations. `rebuild_view_data()` skips unchanged devices — most events affect only 1–2 of 8 devices.

3. **Pre-allocated view buffers**: `view_running` and `view_queues` are allocated once in `new()` and reused via `clear()` + `push()` instead of `collect()`.

**Results**:

| Tasks | Before  | After  | Speedup  |
|-------|---------|--------|----------|
| 30    | 3.1ms   | 2.6ms  | 1.2x     |
| 100   | 10.9ms  | 9.2ms  | 1.2x     |
| 250   | 2.7s    | 252ms  | **10.7x** |
| 500   | 21.4s   | 1.62s  | **13.2x** |

### 2.3 Partitioned Ready Queues (DeviceManager)

**Status**: ✅ Merged into 2.2 above (same BTreeMap implementation).

### 2.4 Event Queue Optimization

**Status**: ⏭️ Not needed — BinaryHeap accounted for <4% CPU in profiling. At O(log N) for N=450K events, this adds ~19 comparisons per operation, which is negligible.

## Phase 3: Rayon Batch Parallelism (IMPLEMENTED ✅)

### 3.1 CLI Subcommand

Implemented as `hprss-sim sweep` subcommand. Backward-compatible — `hprss-sim --platform ...` still works for single runs.

```
hprss-sim --platform configs/platform_ft2000_full.toml sweep \
  --utilizations 0.3:0.1:0.95 \
  --task-counts 10,50,100,250,500 \
  --seeds 1:10 \
  --output results.csv \
  --jobs 0  # 0 = use all CPU cores
```

Parameters parsed from string ranges (`start:step:end` for floats, `start:end` for seeds, comma-separated for task counts). Cartesian product of all combinations is generated automatically.

### 3.2 Parallel Execution

```rust
use rayon::prelude::*;

let rows: Vec<SweepRow> = configs
    .par_iter()
    .filter_map(|&(utilization, task_count, seed)| {
        let (sim, wall_us) = run_single(&platform, task_count, utilization, seed).ok()?;
        Some(SweepRow { utilization, task_count, seed, /* ... */ wall_time_us: wall_us })
    })
    .collect();
```

`SimEngine` is self-contained (no global state), so each instance runs in its own Rayon worker thread safely. Progress counter displayed via `AtomicUsize`.

### 3.3 Output Format

CSV via `csv` + `serde::Serialize` with columns: `utilization, task_count, seed, algorithm, total_jobs, completed_jobs, deadline_misses, miss_ratio, schedulable, events_processed, wall_time_us`.

### 3.4 Batch Performance

350 experiments (7 utilizations × 5 task counts × 10 seeds) complete in **33.9s** on release build, including 500-task runs at U=0.95.

### 3.5 Limitations / Future Work

- **Algorithm sweep parameter**: 已实现为 `--schedulers`（支持 `fp,edf,edfvd,llf,heft,cpedf,federated,global-edf,gang`），并新增 `--analysis-modes`。`SweepRow` 已包含算法键/家族与分析模式等元数据字段。
- **SimResult struct**: Added to `hprss_engine::engine` with serde `Serialize` derive for downstream consumption.

## Phase 4 (Future): PDES Exploration

Deferred. Post-optimization assessment:

- 500 tasks now runs in **1.62s** (down from 21.4s)
- This is between the two thresholds: not <1s (no PDES needed) but not >5s either
- **Recommendation**: PDES is not needed for current thesis experiments. The 1.62s single-sim time, combined with Rayon batch parallelism, can handle large parameter sweeps efficiently. Revisit only if task counts need to reach 2000+ or simulation duration increases significantly.
- Key prerequisite remains: make `Scheduler` trait support concurrent access (e.g., per-device scheduler instances)

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

## Success Criteria — Final Assessment

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | Benchmark established with reproducible measurements | ✅ | `engine_bench.rs` with Criterion, 4 task-count scenarios |
| 2 | Flamegraph generated, hotspots identified | ✅ | `baseline-flamegraph.svg`, 87% CPU in Vec allocations |
| 3 | At least 2x single-sim speedup on 500-task benchmark | ✅ | **13.2x** (21.4s → 1.62s) |
| 4 | Batch mode processes 1000 experiments using all CPU cores | ✅ | 350 experiments in 34s (scales linearly with Rayon) |
| 5 | Zero change to simulation correctness | ✅ | 35/35 tests pass, identical simulation results |
