#!/usr/bin/env python3
import json
import sys

payload = json.load(sys.stdin)
if payload.get("adapter_contract") != "hprss-simso-v1":
    print("bad adapter contract", file=sys.stderr)
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
