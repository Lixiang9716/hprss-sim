#!/usr/bin/env python3
import sys

print("forced non-zero failure", file=sys.stderr)
raise SystemExit(9)
