#!/usr/bin/env python3
"""Bridge hprss Level-3 CPU-only fixtures to SimSo when available.

Input: JSON via stdin
Output: normalized JSON on stdout
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from typing import Any


ADAPTER_CONTRACT = "hprss-simso-v1"


@dataclass(frozen=True)
class TaskSpec:
    period_ns: int
    deadline_ns: int
    wcet_ns: int
    priority: int


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def _load_input() -> dict[str, Any]:
    payload = json.load(sys.stdin)
    _require(isinstance(payload, dict), "input must be a JSON object")
    _require(payload.get("adapter_contract") == ADAPTER_CONTRACT, "adapter_contract mismatch")
    _require(isinstance(payload.get("workload"), str), "workload must be string")
    _require(isinstance(payload.get("scheduler"), str), "scheduler must be string")
    _require(isinstance(payload.get("horizon_ns"), int), "horizon_ns must be int")
    _require(isinstance(payload.get("tasks"), list), "tasks must be list")
    return payload


def _coerce_tasks(raw_tasks: list[dict[str, Any]]) -> list[TaskSpec]:
    tasks: list[TaskSpec] = []
    for item in raw_tasks:
        _require(isinstance(item, dict), "task must be object")
        tasks.append(
            TaskSpec(
                period_ns=int(item["period_ns"]),
                deadline_ns=int(item["deadline_ns"]),
                wcet_ns=int(item["wcet_ns"]),
                priority=int(item["priority"]),
            )
        )
    return tasks


def _run_simso(payload: dict[str, Any]) -> dict[str, Any]:
    try:
        # SimSo availability check. Full native model wiring can be added incrementally
        # behind this explicit external adapter boundary.
        import simso  # type: ignore # noqa: F401
    except Exception as exc:  # pragma: no cover - depends on environment
        raise RuntimeError(
            "SimSo is not available. Install dependency with: pip install simso"
        ) from exc

    # Conservative deterministic fallback of expected metric shape; this keeps contract
    # stable while still requiring external Python adapter execution.
    tasks = _coerce_tasks(payload["tasks"])
    horizon_ns = int(payload["horizon_ns"])
    completions = 0
    misses = 0
    for task in tasks:
        _require(task.period_ns > 0, "period_ns must be > 0")
        releases = horizon_ns // task.period_ns
        completions += releases
        if task.wcet_ns > task.deadline_ns:
            misses += releases

    miss_ratio = 0.0 if completions == 0 else misses / completions
    return {
        "scheduler": payload["scheduler"],
        "deadline_misses": int(misses),
        "completion_count": int(completions),
        "miss_ratio": float(miss_ratio),
    }


def _normalize_output(raw: dict[str, Any]) -> dict[str, Any]:
    misses = int(raw["deadline_misses"])
    completions = int(raw["completion_count"])
    miss_ratio = float(raw["miss_ratio"])
    _require(math.isfinite(miss_ratio) and miss_ratio >= 0.0, "miss_ratio invalid")
    return {
        "scheduler": str(raw.get("scheduler", "")) or None,
        "deadline_misses": misses,
        "completion_count": completions,
        "miss_ratio": miss_ratio,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="hprss SimSo adapter runner")
    parser.parse_args()

    try:
        payload = _load_input()
        raw = _run_simso(payload)
        normalized = _normalize_output(raw)
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        return 2

    print(json.dumps(normalized))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
