#!/usr/bin/env python3
import json
import sys

payload = json.load(sys.stdin)
if payload.get("adapter_contract") != "hprss-simso-v1":
    print("bad adapter contract", file=sys.stderr)
    raise SystemExit(2)

print(
    json.dumps(
        {
            "scheduler": payload.get("scheduler"),
            "misses": 7,
            "completions": 777,
            "miss_ratio": 0.123456,
        }
    )
)
