#!/usr/bin/env python3
import json
import os
import sys


def _require(condition: bool, message: str) -> None:
    if not condition:
        print(message, file=sys.stderr)
        raise SystemExit(2)


def main() -> int:
    payload = json.load(sys.stdin)
    _require(payload.get("adapter_contract") == "hprss-simso-v1", "bad adapter contract")
    _require("tasks" in payload and isinstance(payload["tasks"], list), "tasks missing")
    _require("scheduler" in payload, "scheduler missing")

    mode = os.getenv("HPRSS_SIMSO_FIXTURE_MODE", "legacy_sentinel")
    scheduler = payload["scheduler"]
    scheduler_tag = "edf" if scheduler == "edf" else "fp"

    if mode == "single_task_match":
        task = payload["tasks"][0]
        completions = payload["horizon_ns"] // task["period_ns"]
        out = {
            "scheduler": scheduler_tag,
            "deadline_misses": 0,
            "completion_count": completions,
            "miss_ratio": 0.0,
        }
    elif mode == "legacy_sentinel":
        out = {
            "scheduler": scheduler_tag,
            "misses": 7,
            "completions": 777,
            "miss_ratio": 0.123456,
        }
    else:
        print(f"unsupported fixture mode {mode}", file=sys.stderr)
        return 3

    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
