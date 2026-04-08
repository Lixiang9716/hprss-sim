# Copilot instructions for this repository

## Repository scope

- Primary implementation lives in `hprss-sim/` (Rust workspace, edition 2024).
- Top-level PDFs and `thesis/` are research artifacts; only edit them when the task explicitly asks.
- Run build/test/lint commands from `hprss-sim/` unless the task is repository-level documentation/config.

## Build, test, and lint commands

From `hprss-sim/`:

- Build: `cargo build --workspace` (or `make build`)
- Release build: `cargo build --workspace --release` (or `make release`)
- Format check: `cargo fmt --check` (or `make fmt`)
- Lint: `cargo clippy --workspace -- -D warnings` (or `make clippy`)
- Full tests: `cargo test --workspace` (or `cargo nextest run --workspace` / `make test`)
- Combined quality gate: `make check` (fmt + clippy + test + audit)

Single-test examples:

- Integration test file: `cargo test -p hprss-engine --test dag_flow`
- One test in an integration file: `cargo test -p hprss-engine --test closed_loop accelerator_task_transfer_and_completion`
- One unit test in CLI crate: `cargo test -p hprss-sim parse_scheduler_list_supports_multiple_algorithms`

Simulator execution:

- Single run: `cargo run -p hprss-sim -- --platform configs/platform_ft2000_full.toml --tasks 10 --utilization 0.6 --seed 42`
- Sweep: `cargo run --release -p hprss-sim -- --platform configs/platform_ft2000_full.toml sweep --utilizations 0.5:0.1:0.9 --task-counts 10,50,100 --seeds 1:5 --output sweep_results.csv`

## High-level architecture

Nine-crate workspace; data flows top-down, dependencies point inward:

1. **Model layer (`hprss-types`)**
   Canonical shared types: `Task`, `DagTask`, `Job`, `EventKind`, `DeviceConfig`, `Action`, the `Scheduler` trait, and newtype IDs (`TaskId(u32)`, `JobId(u64)`, `DeviceId(u32)`, `BusId(u32)`, `ChainId(u32)`).

2. **Input layer (`hprss-platform`, `hprss-workload`)**
   `hprss-platform` loads TOML platform configs and converts units (µs → ns, Gbps → bytes/ns).
   `hprss-workload` generates task sets (UUniFast-Discard) and DAG workloads (Erdős-Rényi, layered).

3. **Core DES layer (`hprss-engine`)**
   `SimEngine` — event-loop, job state machine, metrics updates.
   `DeviceManager` — per-device ready queues (`BTreeMap<u32, VecDeque<JobId>>`) + running job tracking.
   `TransferManager` — interconnect/shared-bus transfer timing and queueing.
   `DagTracker` — DAG node → proxy Task mapping, edge-token successor release.

4. **Scheduling layer (`hprss-scheduler`)**
   `FixedPriorityScheduler`, `EdfScheduler`, `HeftScheduler` implement `Scheduler` and return `Vec<Action>`; the engine executes those actions.

5. **Validation layer (`hprss-validate`)**
   Analytic RTA (response-time analysis), level 1–4 classical theory tests, differential validation (HEFT reproduction, paper experiment baselines).

6. **Output layer (`hprss-metrics`, CLI in `hprss-sim`)**
   `MetricsCollector` tracks releases/completions/deadline misses, emits JSONL trace.
   CLI supports single runs, parameter sweeps, and `--scheduler fp|edf|heft` selection.

`hprss-devices` is a stub crate reserved for future virtual device simulators.

## Scheduler–engine interaction

The `Scheduler` trait (defined in `hprss-types/src/scheduler.rs`) has four callbacks:

- `on_job_arrival(job, task, view)` — new job released
- `on_job_complete(job, device_id, view)` — execution finished
- `on_preemption_point(device_id, running_job, view)` — GPU kernel / DSP DMA boundary
- `on_criticality_change(new_level, trigger_job, view)` — mixed-criticality mode switch

Each returns `Vec<Action>`. The engine pops events from a `BinaryHeap<Event>` (min-heap via reversed `Ord`), rebuilds an immutable `SchedulerView` snapshot, calls the appropriate scheduler callback, then executes the returned actions (Dispatch, Preempt, Enqueue, Migrate, DropJob, NoOp).

`SchedulerView` intentionally hides remaining execution time and internal job state — schedulers see only what real hardware could observe.

## Key codebase conventions

- **Priority semantics:** lower numeric value = higher priority (1 is highest).
- **All times in nanoseconds** (`Nanos = u64`). Platform configs use µs/Gbps, converted at load time.
- **Event invalidation:** `Job.version` increments on state transitions; events carry `expected_version`; engine drops stale events O(1) — no heap search needed.
- **Execution-time resolution:** jobs may release with unresolved execution time (`actual_exec_ns: Option`) and resolve when dispatched to a concrete device.
- **Preemption models per device:** `FullyPreemptive` (CPU), `LimitedPreemptive` with granularity (GPU), `InterruptLevel` with ISR/DMA overhead (DSP), `NonPreemptive` with reconfig time (FPGA).
- **DAG execution:** DAG nodes become proxy `Task`s; successors release only when ALL incoming edge-tokens are satisfied (including transfer completions).
- **Transfer time:** `latency_ns + ceil(data_size / bandwidth_bytes_per_ns)`; shared buses queue and arbitrate.
- **Multi-core CPU:** each core is a separate `DeviceConfig`; related cores share `device_group`.
- **Workload generation:** UUniFast-Discard assigns utilizations, RM order by sorted periods, then rewrites `TaskId`/`priority` to match. All RNG uses `ChaCha8Rng` for reproducibility.
- **Performance-sensitive path:** preserve `DeviceManager` queue design and dirty-flag `SchedulerView` rebuilding when touching scheduler-view plumbing.
- **Adding a new scheduler:** update `SchedulerKind`, `parse_scheduler_list`, `build_scheduler`, and `scheduler_label` together in `crates/hprss-sim/src/main.rs`.
- **Error handling:** `thiserror` enums for domain errors, `anyhow` in CLI; no panics in simulation paths.

## Test organization

- **Unit tests:** `#[cfg(test)] mod tests` in-file (types, scheduler, metrics, workload).
- **Integration tests:** `crates/hprss-engine/tests/` — `closed_loop.rs` (E2E scenarios), `dag_flow.rs` (DAG scheduling).
- **Validation tests:** `crates/hprss-validate/` — analytic RTA, differential, and reproduction tests.
- **Pattern:** integration tests create a full `SimEngine`, register tasks/DAGs, run simulation, assert on `MetricsCollector` results.

## Platform config format

TOML files in `configs/`. Key sections: `[simulation]`, `[[device]]`, `[[interconnect]]`, `[[shared_bus]]`. Each device specifies `type` (cpu/gpu/dsp/fpga), `preemption` model, `speed_factor`, and optional `device_group`. Reference config: `configs/platform_ft2000_full.toml` (4-core CPU + GPU + DSP + FPGA with PCIe bus).
