#!/usr/bin/env python3
import json
import sys

payload = json.load(sys.stdin)
if payload.get("adapter_contract") != "hprss-simso-v1":
    print("bad adapter contract", file=sys.stderr)
    raise SystemExit(2)

if payload.get("strict_mode") is not True:
    print("strict_mode must be true", file=sys.stderr)
    raise SystemExit(2)

scenario = payload.get("scenario") or {}
if scenario.get("domain") != "cpu_only":
    print("scenario.domain must be cpu_only", file=sys.stderr)
    raise SystemExit(2)
if scenario.get("core_count") != 1:
    print("scenario.core_count must be 1", file=sys.stderr)
    raise SystemExit(2)

algorithm = payload.get("algorithm") or {}
if algorithm.get("canonical") != "fp":
    print("algorithm.canonical must be fp", file=sys.stderr)
    raise SystemExit(2)

task = payload["tasks"][0]
completions = payload["horizon_ns"] // task["period_ns"]
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
