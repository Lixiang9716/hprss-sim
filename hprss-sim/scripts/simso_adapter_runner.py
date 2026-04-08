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


def _require_field(obj: dict[str, Any], key: str) -> Any:
    if key not in obj:
        raise ValueError(f"missing required field: {key}")
    return obj[key]


def _require_str(obj: dict[str, Any], key: str) -> str:
    value = _require_field(obj, key)
    if not isinstance(value, str):
        raise ValueError(f"field `{key}` must be string")
    return value


def _require_int(obj: dict[str, Any], key: str) -> int:
    value = _require_field(obj, key)
    if type(value) is not int:
        raise ValueError(f"field `{key}` must be integer")
    return value


def _require_list(obj: dict[str, Any], key: str) -> list[Any]:
    value = _require_field(obj, key)
    if not isinstance(value, list):
        raise ValueError(f"field `{key}` must be list")
    return value


def _load_input() -> dict[str, Any]:
    payload = json.load(sys.stdin)
    _require(isinstance(payload, dict), "input must be a JSON object")
    adapter_contract = _require_str(payload, "adapter_contract")
    _require(adapter_contract == ADAPTER_CONTRACT, "adapter_contract mismatch")
    _require_str(payload, "workload")
    _require_str(payload, "scheduler")
    _require_int(payload, "horizon_ns")
    _require_list(payload, "tasks")
    return payload


def _coerce_tasks(raw_tasks: list[dict[str, Any]]) -> list[TaskSpec]:
    tasks: list[TaskSpec] = []
    for item in raw_tasks:
        _require(isinstance(item, dict), "task must be object")
        tasks.append(
            TaskSpec(
                period_ns=_require_int(item, "period_ns"),
                deadline_ns=_require_int(item, "deadline_ns"),
                wcet_ns=_require_int(item, "wcet_ns"),
                priority=_require_int(item, "priority"),
            )
        )
    return tasks


def _run_simso(payload: dict[str, Any]) -> dict[str, Any]:
    try:
        _install_imp_compat()
        from simso.configuration import Configuration  # type: ignore
        from simso.core.Model import Model  # type: ignore
    except Exception as exc:  # pragma: no cover - depends on environment
        raise RuntimeError(
            "SimSo is not available. Install dependency with: pip install simso"
        ) from exc

    tasks = _coerce_tasks(payload["tasks"])
    horizon_ns = _require_int(payload, "horizon_ns")
    scheduler = _require_str(payload, "scheduler").strip().lower()
    _require(scheduler in {"fp", "edf"}, "scheduler must be fp or edf")

    config = Configuration()
    config.duration = horizon_ns
    config.cycles_per_ms = 1
    config.add_processor("CPU0", identifier=0, speed=1.0)
    config.scheduler_info.clas = (
        "simso.schedulers.FP" if scheduler == "fp" else "simso.schedulers.EDF"
    )

    max_priority = max((task.priority for task in tasks), default=0)
    if scheduler == "fp":
        config.task_data_fields = {"priority": ("priority", "int")}

    for index, task in enumerate(tasks):
        _require(task.period_ns > 0, "period_ns must be > 0")
        task_data = {}
        if scheduler == "fp":
            task_data["priority"] = int(max_priority + 1 - task.priority)
        config.add_task(
            name=f"T{index}",
            identifier=index + 1,
            task_type="Periodic",
            abort_on_miss=False,
            period=task.period_ns,
            activation_date=0,
            wcet=task.wcet_ns,
            acet=task.wcet_ns,
            deadline=task.deadline_ns,
            data=task_data,
        )

    config.check_all()
    model = Model(config)
    model.run_model()
    _require(model.results is not None, "SimSo run produced no results")

    misses = 0
    completions = 0
    for task_result in model.results.tasks.values():
        misses += int(task_result.exceeded_count)
        completions += sum(
            1 for job in task_result.jobs if job.end_date is not None and not job.aborted
        )

    miss_ratio = 0.0 if completions == 0 else misses / float(completions)
    return {
        "scheduler": scheduler,
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


def _install_imp_compat() -> None:
    if "imp" in sys.modules:
        return
    import importlib.machinery
    import importlib.util
    import types

    imp = types.ModuleType("imp")

    def find_module(name: str, path: list[str] | None = None):
        spec = importlib.machinery.PathFinder.find_spec(name, path)
        if spec is None or spec.origin is None:
            raise ImportError(name)
        return (None, spec.origin, (None, None, None))

    def load_module(name: str, _file, pathname: str, _description):
        spec = importlib.util.spec_from_file_location(name, pathname)
        if spec is None or spec.loader is None:
            raise ImportError(name)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        sys.modules[name] = module
        return module

    imp.find_module = find_module  # type: ignore[attr-defined]
    imp.load_module = load_module  # type: ignore[attr-defined]
    sys.modules["imp"] = imp


if __name__ == "__main__":
    raise SystemExit(main())
