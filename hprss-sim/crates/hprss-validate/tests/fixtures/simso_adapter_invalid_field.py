#!/usr/bin/env python3
import json

print(
    json.dumps(
        {
            "scheduler": "fp",
            "deadline_misses": 0,
            "completion_count": "invalid",
            "miss_ratio": 0.0,
        }
    )
)
