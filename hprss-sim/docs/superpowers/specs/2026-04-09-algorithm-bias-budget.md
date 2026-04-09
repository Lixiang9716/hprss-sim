# 2026-04-09 Algorithm Bias and Approximation Budget

## Evidence scope

- Artifacts: `artifacts/reproduction/alg-paper-reproduction-suite/{sweep.csv,suite_records.jsonl,manifest.json}`
- Scenario rows: **164 total** (**164 ok**, **0 non-ok**).
- Scenario types: **synthetic-sweep 160 / openmp-adapter 2 / karami-paper-profile 2**.

## Coverage-status totals (implementation paths)

| status | count |
|---|---:|
| implemented baseline | 15 |
| approximation-bound | 0 |
| unsupported | 0 |
| failed_scenario | 0 |

## By paper

| paper | family | status | bias class | notes |
|---|---|---|---|---|
| `SHAPE_ICCAD2022_Xu.pdf` | analytic test | implemented baseline | implemented baseline | SHAPE analytic path now includes deterministic paper-style numeric alignment tests and exact confidence-bound assertions. |
| `XSched_OSDI25_Shen.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `GCAPS_2024_Wang.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RT_Conditional_DAG_TCAD2023_He.pdf` | analytic test | implemented baseline | implemented baseline | Analytic module exists and is integrated in the validation surface. |
| `RT_Heterogeneous_GenAI_2025_Karami.pdf` | external adapter | implemented baseline | implemented baseline | Karami adapter is integrated into CLI/reproduction and now appears in suite records. |
| `Preemptive_Priority_GPU_RT_2024_Wang.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler preemption-point victim selection and priority semantics are covered by deterministic paper-intent tests. |
| `Util_Vectors_RTSS2020_Griffin.pdf` | analytic test | implemented baseline | implemented baseline | Analytic module exists as executable baseline in validation layer. |
| `GPREEMPT_ATC25_Fan.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `RTA_Uniform_ECRTS2024_Sun.pdf` | analytic test | implemented baseline | implemented baseline | Uniform RTA module exists in validation analytics as executable baseline. |
| `Eval_SchedTests_WATERS2016_Davis.pdf` | analytic test | implemented baseline | implemented baseline | Baseline remains implemented and exact-reference friendly in current suite outputs. |
| `RTGPU_TPDS23_Zou.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler module and CLI/catalog wiring are present as executable baseline. |
| `SimSo_WATERS2014_Cheramy.pdf` | external adapter | implemented baseline | implemented baseline | Adapter contract now includes structured mismatch diagnostics and paper-field alignment fixture coverage. |
| `MATCH_RTSS2025_Ni.pdf` | online scheduler | implemented baseline | implemented baseline | Scheduler implementation (`match_sched`) and integration tests are present as baseline. |
| `WCRT_OpenMP_RTSS2021_Sun.pdf` | analytic test | implemented baseline | implemented baseline | OpenMP WCRT estimator now uses paper-style fixed-point HP semantics with deterministic numeric alignment vectors. |
| `Survey_RT_Heterogeneous_2025_Zou.pdf` | external adapter | implemented baseline | implemented baseline | Taxonomy matrix now includes paper-traceable evidence paths plus machine-checkable consistency validation. |

## Open issues (fidelity-focused)

1. Paper-alignment assertions are now present for all prior approximation items; keep these fixtures stable across refactors.
2. Continue rerunning reproduction suite on each algorithm-surface change to prevent silent regression in paper-aligned metrics.
3. Keep taxonomy checker synced with scheduler/analysis inventory additions.
