# Profiling Baseline Report

**Date**: 2026-04-08
**Platform**: FT2000 (CPU+GPU+DSP+FPGA), 10s simulated

## Benchmark Results (Criterion)

| Tasks | Wall Time | Events | Growth |
|-------|-----------|--------|--------|
| 30    | 3.1ms     | ~10K   | 1x     |
| 100   | 10.9ms    | ~36K   | 3.5x   |
| 250   | 2.7s      | ~226K  | 870x   |
| 500   | 21.4s     | ~550K  | 6900x  |

**O(N²+) scaling detected**: 100→250 tasks (2.5x) causes 248x slowdown.

## Perf Profile (flat, 250 tasks)

| % CPU | Function |
|-------|----------|
| 87.0% | `Vec::from_iter` (heap allocations from `.collect()`) |
| 9.0%  | libc (malloc/free) |
| ~4%   | Everything else (BinaryHeap, get_job, schedule_event) |

## Root Cause

`DeviceManager::rebuild_view_data()` is called on **every scheduler invocation** (5 call sites in engine.rs). Each call:

1. Clears and re-allocates `view_running` and `view_queues` vectors
2. For each device, builds `Vec<QueuedJobInfo>` via `.collect()` — iterating all queued jobs
3. With 250 tasks at U=0.85, ready queues are large → thousands of QueuedJobInfo allocations per event

At ~226K events × 4 devices × N queued jobs = **billions of heap allocations**.

## Optimization Priority (data-driven)

1. **Eliminate rebuild_view_data allocations** — reuse buffers, avoid collect
2. **Slab job storage** — reduce Option<Job> holes, improve cache locality
3. **Indexed ready queues** — BTreeMap for O(log K) insert vs O(N) sorted insert
4. **Event queue** — BinaryHeap is fine (< 4% CPU)

## Key Insight

The superlinear scaling comes from queue sizes growing with task count:
- More tasks → larger ready queues
- Each event rebuilds ALL queues as Vecs
- Cost per event ∝ total_queued_jobs → O(events × queued_jobs) ≈ O(N²)
