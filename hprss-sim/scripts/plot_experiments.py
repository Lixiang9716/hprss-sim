#!/usr/bin/env python3
"""Generate publication-friendly plots from `hprss-sim sweep` CSV output.

Dependencies (minimal):
  - Python 3.9+
  - matplotlib (install via `pip install -r scripts/requirements-plot.txt`)

The script intentionally uses stdlib CSV/JSON parsing; pandas is not required.

Examples:
  python3 scripts/plot_experiments.py \
      --csv sweep_results.csv \
      --output-dir plots

  python3 scripts/plot_experiments.py \
      --csv sweep_results.csv \
      --trace-jsonl run_trace.jsonl \
      --output-dir plots --format pdf
"""

from __future__ import annotations

import argparse
import csv
import json
import sys
from collections import defaultdict
from pathlib import Path
from statistics import fmean, pstdev
from typing import Any

REQUIRED_COLUMNS = {
    "utilization",
    "algorithm",
    "miss_ratio",
    "schedulable",
    "makespan",
    "avg_response_time",
    "wall_time_us",
}

COLOR_CYCLE = ["#1f77b4", "#ff7f0e", "#2ca02c", "#d62728", "#9467bd"]


def parse_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes"}:
        return True
    if normalized in {"0", "false", "no"}:
        return False
    raise ValueError(f"invalid bool: {value!r}")


def load_sweep_rows(csv_path: Path) -> list[dict[str, Any]]:
    with csv_path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        if not reader.fieldnames:
            raise ValueError(f"{csv_path} has no header")
        missing = REQUIRED_COLUMNS.difference(reader.fieldnames)
        if missing:
            missing_csv = ", ".join(sorted(missing))
            raise ValueError(f"{csv_path} missing required columns: {missing_csv}")

        rows: list[dict[str, Any]] = []
        for line_no, raw in enumerate(reader, start=2):
            try:
                rows.append(
                    {
                        "utilization": float(raw["utilization"]),
                        "algorithm": raw["algorithm"].strip(),
                        "miss_ratio": float(raw["miss_ratio"]),
                        "schedulable": parse_bool(raw["schedulable"]),
                        "makespan": float(raw["makespan"]),
                        "avg_response_time": float(raw["avg_response_time"]),
                        "wall_time_us": float(raw["wall_time_us"]),
                    }
                )
            except Exception as exc:  # noqa: BLE001 - keep error message with line number
                raise ValueError(f"failed to parse {csv_path}:{line_no}: {exc}") from exc

    if not rows:
        raise ValueError(f"{csv_path} contains no data rows")
    return rows


def aggregate_sched_miss_by_algorithm(
    rows: list[dict[str, Any]],
) -> dict[str, list[tuple[float, float, float]]]:
    grouped: dict[tuple[str, float], list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        grouped[(row["algorithm"], row["utilization"])].append(row)

    curves: dict[str, list[tuple[float, float, float]]] = defaultdict(list)
    for (algorithm, utilization), bucket in grouped.items():
        sched_ratio = sum(1 for item in bucket if item["schedulable"]) / len(bucket)
        miss_ratio = fmean(float(item["miss_ratio"]) for item in bucket)
        curves[algorithm].append((utilization, sched_ratio, miss_ratio))

    for algorithm in curves:
        curves[algorithm].sort(key=lambda point: point[0])

    return dict(curves)


def aggregate_metric_by_algorithm(
    rows: list[dict[str, Any]], metric: str
) -> list[tuple[str, float, float]]:
    grouped: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        grouped[row["algorithm"]].append(float(row[metric]))

    stats: list[tuple[str, float, float]] = []
    for algorithm in sorted(grouped):
        samples = grouped[algorithm]
        mean = fmean(samples)
        std = pstdev(samples) if len(samples) > 1 else 0.0
        stats.append((algorithm, mean, std))
    return stats


def load_trace_jsonl(trace_path: Path) -> dict[str, list[float]]:
    events: dict[str, list[float]] = defaultdict(list)
    with trace_path.open("r", encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"invalid JSON at {trace_path}:{line_no}: {exc}") from exc

            event = str(data.get("event", "unknown"))
            timestamp = data.get("time", data.get("t"))
            if timestamp is None:
                raise ValueError(f"missing time/t at {trace_path}:{line_no}")
            events[event].append(float(timestamp))

    return dict(events)


def get_plot_backend():
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError as exc:
        raise RuntimeError(
            "matplotlib is required. Install with: pip install -r scripts/requirements-plot.txt"
        ) from exc
    return plt


def apply_publication_style(plt) -> None:
    plt.rcParams.update(
        {
            "figure.dpi": 300,
            "savefig.dpi": 300,
            "font.size": 11,
            "axes.titlesize": 12,
            "axes.labelsize": 11,
            "legend.fontsize": 10,
            "lines.linewidth": 2.0,
            "lines.markersize": 5,
            "axes.grid": True,
            "grid.linestyle": "--",
            "grid.alpha": 0.35,
        }
    )


def plot_sched_miss_vs_utilization(
    plt,
    curves: dict[str, list[tuple[float, float, float]]],
    output_path: Path,
) -> None:
    fig, axes = plt.subplots(1, 2, figsize=(11.0, 4.0), constrained_layout=True)

    for idx, (algorithm, points) in enumerate(sorted(curves.items())):
        color = COLOR_CYCLE[idx % len(COLOR_CYCLE)]
        xs = [point[0] for point in points]
        sched_ys = [point[1] for point in points]
        miss_ys = [point[2] for point in points]

        axes[0].plot(xs, sched_ys, marker="o", color=color, label=algorithm)
        axes[1].plot(xs, miss_ys, marker="o", color=color, label=algorithm)

    axes[0].set_title("Schedulability vs Utilization")
    axes[0].set_xlabel("Total Utilization")
    axes[0].set_ylabel("Schedulability Ratio")
    axes[0].set_ylim(0.0, 1.02)

    axes[1].set_title("Miss Ratio vs Utilization")
    axes[1].set_xlabel("Total Utilization")
    axes[1].set_ylabel("Deadline Miss Ratio")
    axes[1].set_ylim(bottom=0.0)

    axes[0].legend(frameon=False)
    fig.savefig(output_path, bbox_inches="tight")
    plt.close(fig)


def plot_metric_vs_algorithm(
    plt,
    stats: list[tuple[str, float, float]],
    title: str,
    ylabel: str,
    output_path: Path,
) -> None:
    algorithms = [item[0] for item in stats]
    means = [item[1] for item in stats]
    stds = [item[2] for item in stats]

    fig, ax = plt.subplots(figsize=(6.0, 4.0), constrained_layout=True)
    colors = [COLOR_CYCLE[idx % len(COLOR_CYCLE)] for idx in range(len(algorithms))]
    ax.bar(algorithms, means, yerr=stds, color=colors, alpha=0.9, capsize=4)
    ax.set_title(title)
    ax.set_ylabel(ylabel)
    fig.savefig(output_path, bbox_inches="tight")
    plt.close(fig)


def plot_makespan_and_response(
    plt,
    makespan_stats: list[tuple[str, float, float]],
    response_stats: list[tuple[str, float, float]],
    output_path: Path,
) -> None:
    by_algorithm = {
        algorithm: {"makespan": (mean, std)}
        for algorithm, mean, std in makespan_stats
    }
    for algorithm, mean, std in response_stats:
        by_algorithm.setdefault(algorithm, {})["response"] = (mean, std)

    algorithms = sorted(by_algorithm)
    makespan_means = [by_algorithm[a]["makespan"][0] for a in algorithms]
    makespan_stds = [by_algorithm[a]["makespan"][1] for a in algorithms]
    response_means = [by_algorithm[a]["response"][0] for a in algorithms]
    response_stds = [by_algorithm[a]["response"][1] for a in algorithms]

    fig, axes = plt.subplots(1, 2, figsize=(11.0, 4.0), constrained_layout=True)

    colors = [COLOR_CYCLE[idx % len(COLOR_CYCLE)] for idx in range(len(algorithms))]
    axes[0].bar(algorithms, makespan_means, yerr=makespan_stds, color=colors, capsize=4)
    axes[0].set_title("Makespan Comparison")
    axes[0].set_ylabel("Makespan (ns)")

    axes[1].bar(algorithms, response_means, yerr=response_stds, color=colors, capsize=4)
    axes[1].set_title("Avg Response Time Comparison")
    axes[1].set_ylabel("Average Response Time (ns)")

    fig.savefig(output_path, bbox_inches="tight")
    plt.close(fig)


def plot_trace_events(plt, events: dict[str, list[float]], output_path: Path) -> None:
    if not events:
        return

    fig, ax = plt.subplots(figsize=(7.0, 4.0), constrained_layout=True)
    for idx, (event, timestamps) in enumerate(sorted(events.items())):
        xs = sorted(timestamps)
        ys = list(range(1, len(xs) + 1))
        ax.step(
            xs,
            ys,
            where="post",
            color=COLOR_CYCLE[idx % len(COLOR_CYCLE)],
            label=event,
        )

    ax.set_title("Trace Event Timeline")
    ax.set_xlabel("Time (ns)")
    ax.set_ylabel("Cumulative event count")
    ax.legend(frameon=False)
    fig.savefig(output_path, bbox_inches="tight")
    plt.close(fig)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Plot HPRSS sweep CSV metrics into publication-friendly figures."
    )
    parser.add_argument(
        "--csv",
        type=Path,
        required=True,
        help="Path to sweep CSV generated by `hprss-sim sweep`.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("plots"),
        help="Directory to write figure files (default: plots).",
    )
    parser.add_argument(
        "--trace-jsonl",
        type=Path,
        default=None,
        help="Optional trace JSONL to generate cumulative event plot.",
    )
    parser.add_argument(
        "--format",
        choices=["png", "pdf", "svg"],
        default="png",
        help="Figure format (default: png).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    rows = load_sweep_rows(args.csv)
    curves = aggregate_sched_miss_by_algorithm(rows)
    wall_stats = aggregate_metric_by_algorithm(rows, "wall_time_us")
    makespan_stats = aggregate_metric_by_algorithm(rows, "makespan")
    response_stats = aggregate_metric_by_algorithm(rows, "avg_response_time")

    try:
        plt = get_plot_backend()
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    apply_publication_style(plt)

    args.output_dir.mkdir(parents=True, exist_ok=True)
    generated = []

    sched_path = args.output_dir / f"schedulability_miss_vs_utilization.{args.format}"
    plot_sched_miss_vs_utilization(plt, curves, sched_path)
    generated.append(sched_path)

    wall_path = args.output_dir / f"wall_time_vs_algorithm.{args.format}"
    plot_metric_vs_algorithm(
        plt,
        wall_stats,
        title="Wall Time by Algorithm",
        ylabel="Wall Time (μs)",
        output_path=wall_path,
    )
    generated.append(wall_path)

    compare_path = args.output_dir / f"makespan_avg_response_vs_algorithm.{args.format}"
    plot_makespan_and_response(plt, makespan_stats, response_stats, compare_path)
    generated.append(compare_path)

    if args.trace_jsonl:
        trace_events = load_trace_jsonl(args.trace_jsonl)
        trace_path = args.output_dir / f"trace_events.{args.format}"
        plot_trace_events(plt, trace_events, trace_path)
        generated.append(trace_path)

    for path in generated:
        print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
