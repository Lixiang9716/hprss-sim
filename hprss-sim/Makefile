# HPRSS-Sim Makefile
# Usage: make help

PLATFORM  ?= configs/platform_ft2000_full.toml
TASKS     ?= 10
UTIL      ?= 0.6
SEED      ?= 42

# Sweep defaults
SWEEP_UTILS  ?= 0.3:0.1:0.95
SWEEP_TASKS  ?= 10,50,100,250
SWEEP_SEEDS  ?= 1:10
SWEEP_OUTPUT ?= sweep_results.csv
SWEEP_JOBS   ?= 0
PLOT_INPUT   ?= $(SWEEP_OUTPUT)
REPRO_SWEEP_OUTPUT ?= artifacts/reproduction/alg-paper-reproduction-suite/sweep.csv
PLOT_OUTPUT  ?= plots
PLOT_TRACE   ?=
HW_ALIGN_SIM ?= sweep_results.csv
HW_ALIGN_HW  ?= artifacts/hardware/hw_results.csv
HW_ALIGN_OUT ?= artifacts/hw_alignment_report.json

.PHONY: help build release test check fmt clippy audit bench run sweep plot plot-test repro-suite repro-test taxonomy-check hw-align-report evidence-pack ci-gate clean doc coverage flamegraph

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

# ── Build ────────────────────────────────────────────────────────────────
build: ## Build (debug)
	cargo build --workspace

release: ## Build (release, optimized)
	cargo build --workspace --release

# ── Quality ──────────────────────────────────────────────────────────────
check: fmt clippy test audit ## Run all quality checks

fmt: ## Check formatting (use fmt-fix to auto-fix)
	cargo fmt --check

fmt-fix: ## Auto-fix formatting
	cargo fmt

clippy: ## Lint with clippy (-D warnings)
	cargo clippy --workspace -- -D warnings

audit: ## Security audit of dependencies
	cargo audit

test: ## Run tests with nextest
	cargo nextest run --workspace

coverage: ## Code coverage via tarpaulin
	cargo tarpaulin --workspace --out html --output-dir target/coverage
	@echo "Report: target/coverage/tarpaulin-report.html"

doc: ## Build documentation
	cargo doc --workspace --no-deps

# ── Benchmarks & Profiling ───────────────────────────────────────────────
bench: ## Run Criterion benchmarks
	cargo bench --bench engine_bench

flamegraph: release ## Generate CPU flamegraph (requires perf)
	@mkdir -p docs/profiling
	cargo flamegraph --bin hprss-sim --release \
		-o docs/profiling/flamegraph.svg \
		-- --platform $(PLATFORM) --tasks 250 --utilization 0.85

# ── Run ──────────────────────────────────────────────────────────────────
run: ## Single simulation (debug build)
	cargo run -p hprss-sim -- --platform $(PLATFORM) \
		--tasks $(TASKS) --utilization $(UTIL) --seed $(SEED)

run-release: ## Single simulation (release build)
	cargo run --release -p hprss-sim -- --platform $(PLATFORM) \
		--tasks $(TASKS) --utilization $(UTIL) --seed $(SEED)

sweep: ## Parameter sweep in parallel (release)
	cargo run --release -p hprss-sim -- --platform $(PLATFORM) \
		sweep \
		--utilizations $(SWEEP_UTILS) \
		--task-counts $(SWEEP_TASKS) \
		--seeds $(SWEEP_SEEDS) \
		--output $(SWEEP_OUTPUT) \
		--jobs $(SWEEP_JOBS)

plot: ## Plot paper metrics from sweep CSV (fallback to repro suite CSV if needed)
	@PLOT_CSV="$(PLOT_INPUT)"; \
	if [ ! -f "$$PLOT_CSV" ] && [ -f "$(REPRO_SWEEP_OUTPUT)" ]; then \
		echo "[plot] input '$$PLOT_CSV' missing; fallback to '$(REPRO_SWEEP_OUTPUT)'"; \
		PLOT_CSV="$(REPRO_SWEEP_OUTPUT)"; \
	fi; \
	if [ ! -f "$$PLOT_CSV" ]; then \
		echo "[plot] error: CSV not found. looked for '$$PLOT_CSV' and '$(REPRO_SWEEP_OUTPUT)'" >&2; \
		exit 2; \
	fi; \
	python3 scripts/plot_experiments.py --csv "$$PLOT_CSV" --output-dir "$(PLOT_OUTPUT)" $(if $(PLOT_TRACE),--trace-jsonl $(PLOT_TRACE),)

plot-test: ## Run plotting script sanity tests (stdlib unittest)
	python3 -m unittest scripts/test_plot_experiments.py scripts/test_plot_suite.py

repro-suite: ## Run deterministic algorithm paper reproduction suite
	python3 scripts/alg_paper_reproduction_suite.py

repro-test: ## Run reproduction suite script tests
	python3 -m unittest scripts/test_alg_paper_reproduction_suite.py

taxonomy-check: ## Validate survey taxonomy matrix consistency
	python3 scripts/check_survey_taxonomy_matrix.py

hw-align-report: ## Compare simulator vs hardware CSV; vars: HW_ALIGN_SIM, HW_ALIGN_HW, HW_ALIGN_OUT
	python3 scripts/compare_hw_alignment.py --sim-csv "$(HW_ALIGN_SIM)" --hw-csv "$(HW_ALIGN_HW)" --output "$(HW_ALIGN_OUT)"

evidence-pack: ## Build deterministic evidence bundle from reproduction artifacts
	python3 scripts/build_evidence_bundle.py --repo-root .

ci-gate: ## Run CI-equivalent gate sequence
	$(MAKE) check
	$(MAKE) repro-test
	$(MAKE) plot-test
	$(MAKE) taxonomy-check
	$(MAKE) repro-suite
	$(MAKE) plot

# ── Cleanup ──────────────────────────────────────────────────────────────
clean: ## Remove build artifacts
	cargo clean
	rm -f perf.data perf.data.old
