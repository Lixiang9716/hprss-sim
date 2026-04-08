#!/usr/bin/env python3
"""Plot multi-scheduler comparison bars from sweep CSV."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from plot_experiments import (
    aggregate_metric_by_algorithm,
    aggregate_sched_miss_by_algorithm,
    apply_publication_style,
    get_plot_backend,
    load_sweep_rows,
)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate algorithm comparison bars from sweep CSV output."
    )
    parser.add_argument(
        "--csv",
        type=Path,
        required=True,
        help="Path to sweep CSV from `hprss-sim sweep`.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("plots/scheduler_comparison.png"),
        help="Output figure path (default: plots/scheduler_comparison.png).",
    )
    parser.add_argument(
        "--format",
        choices=["png", "pdf", "svg"],
        default=None,
        help="Override output format extension (png/pdf/svg).",
    )
    return parser.parse_args(argv)


def normalized_output_path(path: Path, file_format: str | None) -> Path:
    if file_format is None:
        return path
    return path.with_suffix(f".{file_format}")


def schedulability_ratio_by_algorithm(rows: list[dict[str, object]]) -> dict[str, float]:
    curves = aggregate_sched_miss_by_algorithm(rows)
    ratios: dict[str, float] = {}
    for algorithm, points in curves.items():
        if not points:
            continue
        ratios[algorithm] = sum(point[1] for point in points) / len(points)
    return ratios


def plot_comparison(plt, rows: list[dict[str, object]], output: Path) -> None:
    ratio_stats = schedulability_ratio_by_algorithm(rows)
    miss_stats = {name: mean for name, mean, _std in aggregate_metric_by_algorithm(rows, "miss_ratio")}
    response_stats = {
        name: mean for name, mean, _std in aggregate_metric_by_algorithm(rows, "avg_response_time")
    }
    wall_stats = {name: mean for name, mean, _std in aggregate_metric_by_algorithm(rows, "wall_time_us")}

    algorithms = sorted(ratio_stats)
    if not algorithms:
        raise ValueError("no algorithm samples found in CSV")

    fig, axes = plt.subplots(2, 2, figsize=(11.0, 7.0), constrained_layout=True)
    bar_cfg = {"alpha": 0.9}

    axes[0, 0].bar(algorithms, [ratio_stats[a] for a in algorithms], **bar_cfg)
    axes[0, 0].set_title("Mean Schedulability Ratio")
    axes[0, 0].set_ylim(0.0, 1.02)

    axes[0, 1].bar(algorithms, [miss_stats[a] for a in algorithms], **bar_cfg)
    axes[0, 1].set_title("Mean Miss Ratio")
    axes[0, 1].set_ylim(bottom=0.0)

    axes[1, 0].bar(algorithms, [response_stats[a] for a in algorithms], **bar_cfg)
    axes[1, 0].set_title("Mean Avg Response Time (ns)")

    axes[1, 1].bar(algorithms, [wall_stats[a] for a in algorithms], **bar_cfg)
    axes[1, 1].set_title("Mean Wall Time (μs)")

    for ax in axes.flat:
        ax.tick_params(axis="x", labelrotation=20)

    fig.savefig(output, bbox_inches="tight")
    plt.close(fig)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = normalized_output_path(args.output, args.format)
    try:
        rows = load_sweep_rows(args.csv)
        plt = get_plot_backend()
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    apply_publication_style(plt)
    output.parent.mkdir(parents=True, exist_ok=True)
    try:
        plot_comparison(plt, rows, output)
    except ValueError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
