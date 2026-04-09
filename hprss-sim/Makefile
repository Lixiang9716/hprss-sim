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
PLOT_OUTPUT  ?= plots
PLOT_TRACE   ?=

.PHONY: help build release test check fmt clippy audit bench run sweep plot plot-test repro-suite repro-test clean doc coverage flamegraph

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

plot: ## Plot paper metrics from sweep CSV (Python + matplotlib)
	python3 scripts/plot_experiments.py --csv $(PLOT_INPUT) --output-dir $(PLOT_OUTPUT) $(if $(PLOT_TRACE),--trace-jsonl $(PLOT_TRACE),)

plot-test: ## Run plotting script sanity tests (stdlib unittest)
	python3 -m unittest scripts/test_plot_experiments.py scripts/test_plot_suite.py

repro-suite: ## Run deterministic algorithm paper reproduction suite
	python3 scripts/alg_paper_reproduction_suite.py

repro-test: ## Run reproduction suite script tests
	python3 -m unittest scripts/test_alg_paper_reproduction_suite.py

# ── Cleanup ──────────────────────────────────────────────────────────────
clean: ## Remove build artifacts
	cargo clean
	rm -f perf.data perf.data.old
