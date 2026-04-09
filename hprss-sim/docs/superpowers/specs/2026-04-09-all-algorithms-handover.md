# 2026-04-09 All-Algorithms Final Handover

## 1) 15-paper algorithm coverage matrix

Legend: **implemented baseline** = executable path exists; **approximation-bound** = executable path with bounded-fidelity limits; **unsupported** = no path.

| Paper | Status | Notes |
|---|---|---|
| `SHAPE_ICCAD2022_Xu.pdf` | approximation-bound | SHAPE path exists, but fidelity remains approximation-bound until paper-native calibration bounds are formalized. |
| `XSched_OSDI25_Shen.pdf` | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `GCAPS_2024_Wang.pdf` | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RT_Conditional_DAG_TCAD2023_He.pdf` | implemented baseline | Analytic module exists and is integrated in the validation surface. |
| `RT_Heterogeneous_GenAI_2025_Karami.pdf` | implemented baseline | Karami adapter is integrated into CLI/reproduction and now appears in suite records. |
| `Preemptive_Priority_GPU_RT_2024_Wang.pdf` | approximation-bound | Implementation path exists, but paper-level fidelity claims remain bounded by approximation assumptions. |
| `Util_Vectors_RTSS2020_Griffin.pdf` | implemented baseline | Analytic module exists as executable baseline in validation layer. |
| `GPREEMPT_ATC25_Fan.pdf` | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RTA_Uniform_ECRTS2024_Sun.pdf` | implemented baseline | Uniform RTA module exists in validation analytics as executable baseline. |
| `Eval_SchedTests_WATERS2016_Davis.pdf` | implemented baseline | Baseline remains implemented and exact-reference friendly in current suite outputs. |
| `RTGPU_TPDS23_Zou.pdf` | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `SimSo_WATERS2014_Cheramy.pdf` | approximation-bound | Adapter is available; fidelity remains bounded by adapter-contract scope assumptions. |
| `MATCH_RTSS2025_Ni.pdf` | implemented baseline | Scheduler implementation (`match_sched`) and integration tests are present as baseline. |
| `WCRT_OpenMP_RTSS2021_Sun.pdf` | approximation-bound | OpenMP adapter scenarios are passing, but still rely on explicit approximation assumptions. |
| `Survey_RT_Heterogeneous_2025_Zou.pdf` | approximation-bound | Coverage map exists; this remains a taxonomy approximation layer, not a single executable algorithm. |

Roll-up: **implemented baseline 10 / approximation-bound 5 / unsupported 0**.

## 2) Current reproduction coverage

- Primary suite: `configs/repro/alg_paper_reproduction_suite.json`
- Runner: `python3 scripts/alg_paper_reproduction_suite.py`
- Records: **164 total**, **164 ok**, **0 failed**.
- Scenario types: **synthetic-sweep 160 / openmp-adapter 2 / karami-paper-profile 2**.

## 3) Open issues (fidelity, not missing paths)

1. Approximation-bound papers still need explicit numeric bias bounds for paper-native fidelity claims.
2. Remaining risk concentration: SHAPE, GPU preemptive-priority, SimSo scope, OpenMP WCRT adapter, survey taxonomy alignment.
3. Keep reproduction suite green while tightening approximation contracts.

## 4) Reference artifacts

- Catalog baseline: `docs/superpowers/specs/2026-04-09-all-paper-algorithm-catalog.{md,json}`
- Bias budget: `docs/superpowers/specs/2026-04-09-algorithm-bias-budget.{md,json}`
- Reproduction records: `artifacts/reproduction/alg-paper-reproduction-suite/suite_records.jsonl`
