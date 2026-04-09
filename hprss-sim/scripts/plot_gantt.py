#!/usr/bin/env python3
"""Plot a Gantt-style timeline from trace JSONL output."""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from plot_experiments import apply_publication_style, get_plot_backend


EventRow = tuple[str, int, int, float]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a Gantt-style timeline from trace JSONL output."
    )
    parser.add_argument(
        "--trace-jsonl",
        type=Path,
        required=True,
        help="Path to trace JSONL from `hprss-sim --trace-output`.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("plots/trace_gantt.png"),
        help="Output figure path (default: plots/trace_gantt.png).",
    )
    parser.add_argument(
        "--format",
        choices=["png", "pdf", "svg"],
        default=None,
        help="Override output format extension (png/pdf/svg).",
    )
    parser.add_argument(
        "--bar-width-ns",
        type=float,
        default=None,
        help="Optional fixed bar width in ns. Defaults to 1%% of time span.",
    )
    return parser.parse_args(argv)


def normalized_output_path(path: Path, file_format: str | None) -> Path:
    if file_format is None:
        return path
    return path.with_suffix(f".{file_format}")


def _extract_numeric_field(
    data: dict[str, Any],
    path: Path,
    line_no: int,
    keys: tuple[str, ...],
    field_name: str,
    caster,
):
    for key in keys:
        if key not in data:
            continue
        value = data[key]
        try:
            return caster(value)
        except (TypeError, ValueError) as exc:
            raise ValueError(
                f"{path}:{line_no}:{field_name}: invalid value {value!r}"
            ) from exc
    return None


def load_trace_rows(path: Path) -> list[EventRow]:
    rows: list[EventRow] = []
    with path.open("r", encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, start=1):
            text = line.strip()
            if not text:
                continue
            try:
                data = json.loads(text)
            except json.JSONDecodeError as exc:
                raise ValueError(f"invalid JSON at {path}:{line_no}: {exc}") from exc
            if not isinstance(data, dict):
                raise ValueError(f"invalid record at {path}:{line_no}: expected object")

            timestamp = _extract_numeric_field(
                data, path, line_no, ("time", "t"), "time", float
            )
            task_id = _extract_numeric_field(
                data, path, line_no, ("task_id", "task"), "task_id", int
            )
            job_id = _extract_numeric_field(
                data, path, line_no, ("job_id", "job"), "job_id", int
            )
            event = str(data.get("event", "unknown"))
            if timestamp is None:
                raise ValueError(f"{path}:{line_no}:time: missing required field")
            if task_id is None:
                raise ValueError(f"{path}:{line_no}:task_id: missing required field")
            if job_id is None:
                raise ValueError(f"{path}:{line_no}:job_id: missing required field")
            rows.append((event, task_id, job_id, timestamp))

    if not rows:
        raise ValueError(f"{path} contains no trace events")
    rows.sort(key=lambda item: item[3])
    return rows


def plot_trace_gantt(plt, rows: list[EventRow], output: Path, bar_width_ns: float | None) -> None:
    tasks = sorted({task_id for _event, task_id, _job_id, _time in rows})
    lane = {task_id: idx for idx, task_id in enumerate(tasks)}
    min_t = min(item[3] for item in rows)
    max_t = max(item[3] for item in rows)
    span = max(max_t - min_t, 1.0)
    width = bar_width_ns if bar_width_ns and bar_width_ns > 0 else max(1.0, span * 0.01)

    grouped: dict[str, list[tuple[float, int]]] = defaultdict(list)
    for event, task_id, _job_id, timestamp in rows:
        grouped[event].append((timestamp, lane[task_id]))

    fig, ax = plt.subplots(figsize=(11.0, 4.5), constrained_layout=True)
    for event, points in sorted(grouped.items()):
        bars = [(ts - width / 2.0, width) for ts, _lane in points]
        ys = [(ln - 0.35, 0.7) for _ts, ln in points]
        for (x0, w), (y0, h) in zip(bars, ys):
            ax.broken_barh([(x0, w)], (y0, h), alpha=0.85, label=event)

    handles, labels = ax.get_legend_handles_labels()
    dedup: dict[str, Any] = {}
    for handle, label in zip(handles, labels):
        dedup.setdefault(label, handle)

    ax.set_title("Trace Timeline (Gantt-style)")
    ax.set_xlabel("Time (ns)")
    ax.set_ylabel("Task lane")
    ax.set_yticks(list(range(len(tasks))))
    ax.set_yticklabels([f"task-{task_id}" for task_id in tasks])
    ax.legend(dedup.values(), dedup.keys(), frameon=False)

    fig.savefig(output, bbox_inches="tight")
    plt.close(fig)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = normalized_output_path(args.output, args.format)
    try:
        rows = load_trace_rows(args.trace_jsonl)
        plt = get_plot_backend()
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    apply_publication_style(plt)
    output.parent.mkdir(parents=True, exist_ok=True)
    plot_trace_gantt(plt, rows, output, args.bar_width_ns)
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
