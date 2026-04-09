#!/usr/bin/env python3
import json
import sys


def require(condition, message):
    if not condition:
        print(message, file=sys.stderr)
        raise SystemExit(2)


payload = json.load(sys.stdin)
require(payload.get("adapter_contract") == "hprss-simso-v1", "bad adapter_contract")
require(payload.get("strict_mode") is True, "strict_mode must be true")
require(payload.get("scheduler") == "edf", "scheduler must be edf")

scenario = payload.get("scenario") or {}
require(scenario.get("domain") == "cpu_only", "scenario.domain must be cpu_only")
require(scenario.get("core_count") == 1, "scenario.core_count must be 1")

algorithm = payload.get("algorithm") or {}
require(algorithm.get("requested") == "edf", "algorithm.requested must be edf")
require(algorithm.get("canonical") == "edf", "algorithm.canonical must be edf")

model = payload.get("model") or {}
require(model.get("time_unit") == "ns", "model.time_unit must be ns")
require(model.get("task_model") == "periodic", "model.task_model must be periodic")
require(
    model.get("mixed_criticality") is False, "model.mixed_criticality must be false"
)

tasks = payload.get("tasks") or []
require(len(tasks) == 1, "single-task-control must provide exactly one task")
task = tasks[0]
require(task.get("task_id") == 0, "task_id must start at 0")
require(task.get("period_ns") == 10, "period_ns mismatch")
require(task.get("deadline_ns") == 10, "deadline_ns mismatch")
require(task.get("wcet_ns") == 3, "wcet_ns mismatch")
require(task.get("priority") == 1, "priority mismatch")

horizon = payload.get("horizon_ns")
require(horizon == 100, "horizon_ns mismatch")
completions = horizon // task["period_ns"]

print(
    json.dumps(
        {
            "scheduler": payload.get("scheduler"),
            "deadline_misses": 0,
            "completion_count": completions,
            "miss_ratio": 0.0,
        }
    )
)
