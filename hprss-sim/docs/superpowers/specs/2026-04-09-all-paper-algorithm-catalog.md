# 2026-04-09 All Paper Algorithm Catalog (15-paper coverage baseline)

## Scope and grounding

- Grounding sources: `crates/hprss-scheduler/src/`, `crates/hprss-validate/src/analytic/`, `crates/hprss-workload/src/karami_profile_adapter.rs`, `crates/hprss-sim/src/scheduler_catalog.rs`.
- Status labels: **implemented baseline** (executable path exists), **approximation-bound** (path exists but fidelity is bounded), **unsupported** (no path).
- This refresh removes stale unsupported-9 language: all 15 papers now map to implemented or approximation-bound paths.

## Per-paper algorithm coverage catalog

| Paper (thesis set) | Algorithm(s) | Family | Coverage status | Conservative implementation note |
|---|---|---|---|---|
| `SHAPE_ICCAD2022_Xu.pdf` | SHAPE | analytic test | approximation-bound | SHAPE path exists, but fidelity remains approximation-bound until paper-native calibration bounds are formalized. |
| `XSched_OSDI25_Shen.pdf` | XSched | online scheduler | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `GCAPS_2024_Wang.pdf` | GCAPS | online scheduler | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RT_Conditional_DAG_TCAD2023_He.pdf` | Conditional-DAG schedulability analysis | analytic test | implemented baseline | Analytic module exists and is integrated in the validation surface. |
| `RT_Heterogeneous_GenAI_2025_Karami.pdf` | Karami paper-profile adapter | external adapter | implemented baseline | Karami adapter is integrated into CLI/reproduction and now appears in suite records. |
| `Preemptive_Priority_GPU_RT_2024_Wang.pdf` | Preemptive-Priority GPU | online scheduler | approximation-bound | Implementation path exists, but paper-level fidelity claims remain bounded by approximation assumptions. |
| `Util_Vectors_RTSS2020_Griffin.pdf` | Utilization Vectors | analytic test | implemented baseline | Analytic module exists as executable baseline in validation layer. |
| `GPREEMPT_ATC25_Fan.pdf` | GPreempt | online scheduler | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RTA_Uniform_ECRTS2024_Sun.pdf` | Uniform multiprocessor RTA | analytic test | implemented baseline | Uniform RTA module exists in validation analytics as executable baseline. |
| `Eval_SchedTests_WATERS2016_Davis.pdf` | Liu-Layland/EDF/RTA/OPA sched tests | analytic test | implemented baseline | Baseline remains implemented and exact-reference friendly in current suite outputs. |
| `RTGPU_TPDS23_Zou.pdf` | RTGPU | online scheduler | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `SimSo_WATERS2014_Cheramy.pdf` | SimSo differential adapter | external adapter | approximation-bound | Adapter is available; fidelity remains bounded by adapter-contract scope assumptions. |
| `MATCH_RTSS2025_Ni.pdf` | MATCH | online scheduler | implemented baseline | Scheduler implementation (`match_sched`) and integration tests are present as baseline. |
| `WCRT_OpenMP_RTSS2021_Sun.pdf` | OpenMP WCRT analysis | analytic test | approximation-bound | OpenMP adapter scenarios are passing, but still rely on explicit approximation assumptions. |
| `Survey_RT_Heterogeneous_2025_Zou.pdf` | Survey taxonomy mapping | external adapter | approximation-bound | Coverage map exists; this remains a taxonomy approximation layer, not a single executable algorithm. |

## Coverage summary

- Implemented baseline: **10/15**
- Approximation-bound: **5/15**
- Unsupported: **0/15**

## Reproduction artifact summary (latest suite)

- Runner: `python3 scripts/alg_paper_reproduction_suite.py`
- Total records: **164** (all `ok`)
- Scenario types: **synthetic-sweep 160 / openmp-adapter 2 / karami-paper-profile 2**
