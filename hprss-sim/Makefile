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

.PHONY: help build release test check fmt clippy audit bench run sweep clean doc coverage flamegraph

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

# ── Cleanup ──────────────────────────────────────────────────────────────
clean: ## Remove build artifacts
	cargo clean
	rm -f perf.data perf.data.old
