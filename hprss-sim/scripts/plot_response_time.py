#!/usr/bin/env python3
"""Plot response-time CDF and boxplot from sweep CSV."""

from __future__ import annotations

import argparse
import csv
import math
import sys
from collections import defaultdict
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from plot_experiments import REQUIRED_COLUMNS, apply_publication_style, get_plot_backend


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate response-time distribution figures from sweep CSV output."
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
        default=Path("plots/response_time_distribution.png"),
        help="Output figure path (default: plots/response_time_distribution.png).",
    )
    parser.add_argument(
        "--metric",
        choices=["avg_response_time", "worst_response_time"],
        default="avg_response_time",
        help="Response-time metric to plot (default: avg_response_time).",
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


def load_rows_for_metric(csv_path: Path, metric: str) -> list[dict[str, float | str | bool]]:
    required = set(REQUIRED_COLUMNS)
    required.add(metric)
    with csv_path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        if not reader.fieldnames:
            raise ValueError(f"{csv_path} has no header")
        missing = sorted(required.difference(reader.fieldnames))
        if missing:
            raise ValueError(
                f"{csv_path} missing required columns for response-time plot: {', '.join(missing)}"
            )
        rows: list[dict[str, float | str | bool]] = []
        for line_no, raw in enumerate(reader, start=2):
            try:
                rows.append(
                    {
                        "algorithm": str(raw["algorithm"]).strip(),
                        metric: float(raw[metric]),
                    }
                )
            except Exception as exc:  # noqa: BLE001
                raise ValueError(f"failed to parse {csv_path}:{line_no}: {exc}") from exc
    if not rows:
        raise ValueError(f"{csv_path} contains no data rows")
    return rows


def metric_samples_by_algorithm(rows: list[dict[str, float | str | bool]], metric: str) -> dict[str, list[float]]:
    grouped: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        value = row.get(metric)
        if value is None:
            raise ValueError(
                f"CSV is missing '{metric}' column required for --metric={metric}."
            )
        sample = float(value)
        if math.isnan(sample) or math.isinf(sample):
            raise ValueError(f"invalid {metric} value: {sample}")
        grouped[str(row["algorithm"])].append(sample)

    if not grouped:
        raise ValueError("no samples available to plot")
    return {algorithm: sorted(samples) for algorithm, samples in sorted(grouped.items())}


def cdf_points(samples: list[float]) -> tuple[list[float], list[float]]:
    xs = samples
    n = len(samples)
    ys = [idx / n for idx in range(1, n + 1)]
    return xs, ys


def plot_response_distributions(plt, grouped: dict[str, list[float]], metric: str, output: Path) -> None:
    fig, axes = plt.subplots(1, 2, figsize=(11.0, 4.0), constrained_layout=True)

    labels = list(grouped.keys())
    for algorithm, samples in grouped.items():
        xs, ys = cdf_points(samples)
        axes[0].step(xs, ys, where="post", label=algorithm)

    axes[0].set_title(f"{metric} CDF")
    axes[0].set_xlabel(f"{metric} (ns)")
    axes[0].set_ylabel("CDF")
    axes[0].set_ylim(0.0, 1.02)
    axes[0].legend(frameon=False)

    box_data = [grouped[label] for label in labels]
    try:
        axes[1].boxplot(box_data, tick_labels=labels, showfliers=False)
    except TypeError:
        axes[1].boxplot(box_data, labels=labels, showfliers=False)
    axes[1].set_title(f"{metric} Boxplot")
    axes[1].set_ylabel(f"{metric} (ns)")

    fig.savefig(output, bbox_inches="tight")
    plt.close(fig)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = normalized_output_path(args.output, args.format)
    try:
        rows = load_rows_for_metric(args.csv, args.metric)
        grouped = metric_samples_by_algorithm(rows, args.metric)
        plt = get_plot_backend()
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    apply_publication_style(plt)
    output.parent.mkdir(parents=True, exist_ok=True)
    plot_response_distributions(plt, grouped, args.metric, output)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
