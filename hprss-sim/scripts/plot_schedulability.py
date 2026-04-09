#!/usr/bin/env python3
"""Plot schedulability ratio vs utilization from sweep CSV."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from plot_experiments import (
    aggregate_sched_miss_by_algorithm,
    apply_publication_style,
    get_plot_backend,
    load_sweep_rows,
)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate schedulability-vs-utilization curves from sweep CSV output."
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
        default=Path("plots/schedulability_vs_utilization.png"),
        help="Output figure path (default: plots/schedulability_vs_utilization.png).",
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
    suffix = f".{file_format}"
    if path.suffix.lower() == suffix:
        return path
    return path.with_suffix(suffix)


def plot_schedulability_only(plt, curves: dict[str, list[tuple[float, float, float]]], output: Path) -> None:
    fig, ax = plt.subplots(figsize=(6.4, 4.0), constrained_layout=True)
    for algorithm, points in sorted(curves.items()):
        xs = [point[0] for point in points]
        ys = [point[1] for point in points]
        ax.plot(xs, ys, marker="o", label=algorithm)

    ax.set_title("Schedulability vs Utilization")
    ax.set_xlabel("Total Utilization")
    ax.set_ylabel("Schedulability Ratio")
    ax.set_ylim(0.0, 1.02)
    ax.legend(frameon=False)
    fig.savefig(output, bbox_inches="tight")
    plt.close(fig)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = normalized_output_path(args.output, args.format)
    try:
        rows = load_sweep_rows(args.csv)
        curves = aggregate_sched_miss_by_algorithm(rows)
        plt = get_plot_backend()
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    apply_publication_style(plt)
    output.parent.mkdir(parents=True, exist_ok=True)
    plot_schedulability_only(plt, curves, output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
