#!/usr/bin/env python3
"""Compare simulator CSV metrics against hardware CSV metrics."""

from __future__ import annotations

import argparse
import csv
import json
import sys
from pathlib import Path
from statistics import fmean
from typing import Any

STABLE_KEYS = [
    "algorithm_key",
    "analysis_mode",
    "utilization",
    "task_count",
    "seed",
]

METRICS = [
    "miss_ratio",
    "makespan",
    "avg_response_time",
    "deadline_misses",
    "completed_jobs",
]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare simulation CSV metrics with hardware CSV metrics."
    )
    parser.add_argument("--sim-csv", type=Path, required=True, help="Simulator CSV path")
    parser.add_argument("--hw-csv", type=Path, required=True, help="Hardware CSV path")
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Optional output report JSON path",
    )
    return parser.parse_args(argv)


def load_csv(csv_path: Path) -> tuple[list[dict[str, str]], list[str]]:
    with csv_path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        if not reader.fieldnames:
            raise ValueError(f"{csv_path} has no header")
        rows = [dict(row) for row in reader]
    return rows, list(reader.fieldnames)


def parse_number(value: str | None) -> float | None:
    if value is None:
        return None
    text = value.strip()
    if not text:
        return None
    try:
        return float(text)
    except ValueError:
        return None


def make_key(row: dict[str, str], keys: list[str]) -> tuple[str, ...]:
    return tuple((row.get(key) or "").strip() for key in keys)


def build_row_map(
    rows: list[dict[str, str]],
    keys: list[str],
    dataset_name: str,
) -> dict[tuple[str, ...], dict[str, str]]:
    mapped: dict[tuple[str, ...], dict[str, str]] = {}
    for idx, row in enumerate(rows, start=2):
        row_key = make_key(row, keys)
        if row_key in mapped:
            raise ValueError(
                f"duplicate join key in {dataset_name} at CSV row {idx}: {row_key}"
            )
        mapped[row_key] = row
    return mapped


def compute_report(
    sim_rows: list[dict[str, str]],
    sim_fields: list[str],
    hw_rows: list[dict[str, str]],
    hw_fields: list[str],
) -> dict[str, Any]:
    join_keys = [key for key in STABLE_KEYS if key in sim_fields and key in hw_fields]
    if not join_keys:
        raise ValueError("no common stable join keys found across input CSVs")

    sim_map = build_row_map(sim_rows, join_keys, "sim")
    hw_map = build_row_map(hw_rows, join_keys, "hw")

    sim_keys = set(sim_map)
    hw_keys = set(hw_map)
    matched_keys = sorted(sim_keys & hw_keys)

    metric_samples: dict[str, list[float]] = {metric: [] for metric in METRICS}
    per_row: list[dict[str, Any]] = []

    common_metrics = [metric for metric in METRICS if metric in sim_fields and metric in hw_fields]

    for key in matched_keys:
        sim = sim_map[key]
        hw = hw_map[key]
        key_obj = {name: value for name, value in zip(join_keys, key, strict=False)}
        deltas: dict[str, dict[str, float] | None] = {}

        for metric in common_metrics:
            sim_value = parse_number(sim.get(metric))
            hw_value = parse_number(hw.get(metric))
            if sim_value is None or hw_value is None:
                deltas[metric] = None
                continue
            delta = sim_value - hw_value
            abs_delta = abs(delta)
            metric_samples[metric].append(abs_delta)
            deltas[metric] = {
                "sim": sim_value,
                "hw": hw_value,
                "delta": delta,
                "abs_delta": abs_delta,
            }

        per_row.append({"key": key_obj, "deltas": deltas})

    metric_summary: dict[str, dict[str, float | int] | None] = {}
    for metric in METRICS:
        samples = metric_samples[metric]
        if not samples:
            metric_summary[metric] = None
            continue
        metric_summary[metric] = {
            "count": len(samples),
            "max_abs_delta": max(samples),
            "mean_abs_delta": fmean(samples),
        }

    return {
        "join_keys": join_keys,
        "matched_rows": len(matched_keys),
        "unmatched_sim_rows": len(sim_keys - hw_keys),
        "unmatched_hw_rows": len(hw_keys - sim_keys),
        "metrics": metric_summary,
        "rows": per_row,
    }


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        sim_rows, sim_fields = load_csv(args.sim_csv)
        hw_rows, hw_fields = load_csv(args.hw_csv)
        report = compute_report(sim_rows, sim_fields, hw_rows, hw_fields)
    except (FileNotFoundError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    payload = json.dumps(report, indent=2, sort_keys=True)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(payload + "\n", encoding="utf-8")
    print(payload)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
