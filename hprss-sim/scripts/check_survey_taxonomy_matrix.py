#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ALLOWED_STATUS = {"implemented", "partial", "missing", "unsupported"}


def _extract_fn_body(source: str, fn_signature: str) -> str:
    start = source.find(fn_signature)
    if start < 0:
        raise ValueError(f"cannot locate function signature: {fn_signature}")
    brace = source.find("{", start)
    if brace < 0:
        raise ValueError(f"cannot locate opening brace for: {fn_signature}")
    depth = 0
    for idx in range(brace, len(source)):
        ch = source[idx]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return source[brace + 1 : idx]
    raise ValueError(f"unterminated function body for: {fn_signature}")


def extract_scheduler_inventory(scheduler_catalog_path: Path) -> dict[str, str]:
    source = scheduler_catalog_path.read_text(encoding="utf-8")
    key_body = _extract_fn_body(source, "pub(crate) fn scheduler_key(kind: SchedulerKind) -> &'static str")
    label_body = _extract_fn_body(source, "pub(crate) fn scheduler_label(kind: SchedulerKind) -> &'static str")
    key_pairs = dict(re.findall(r'SchedulerKind::(\w+)\s*=>\s*"([^"]+)"', key_body))
    label_pairs = dict(re.findall(r'SchedulerKind::(\w+)\s*=>\s*"([^"]+)"', label_body))
    inventory: dict[str, str] = {}
    for variant, key in key_pairs.items():
        if variant not in label_pairs:
            raise ValueError(f"missing scheduler_label mapping for variant {variant}")
        inventory[key] = label_pairs[variant]
    return inventory


def extract_analysis_inventory(main_rs_path: Path) -> set[str]:
    source = main_rs_path.read_text(encoding="utf-8")
    key_body = _extract_fn_body(source, "fn key(self) -> &'static str")
    return set(re.findall(r'AnalysisMode::\w+\s*=>\s*"([^"]+)"', key_body))


def _assert(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def validate_matrix(
    matrix: dict,
    scheduler_inventory: dict[str, str],
    analysis_inventory: set[str],
    repo_root: Path,
) -> None:
    class_ids: set[str] = set()
    mapped_scheduler: set[str] = set()
    mapped_analysis: set[str] = set()

    for entry in matrix.get("classes", []):
        class_id = entry["class_id"]
        _assert(class_id not in class_ids, f"duplicate class_id: {class_id}")
        class_ids.add(class_id)
        _assert(entry["class_status"] in ALLOWED_STATUS, f"invalid class_status in {class_id}")

        class_scheduler_keys = {m["key"] for m in entry.get("scheduler_mappings", [])}
        class_analysis_keys = {m["key"] for m in entry.get("analysis_mappings", [])}

        for mapping in entry.get("scheduler_mappings", []):
            key = mapping["key"]
            _assert(mapping["implementation_status"] in ALLOWED_STATUS, f"invalid scheduler status for {key}")
            _assert(key in scheduler_inventory, f"unknown scheduler key in matrix: {key}")
            _assert(mapping["label"] == scheduler_inventory[key], f"scheduler label drift for {key}")
            _assert(key not in mapped_scheduler, f"scheduler key duplicated in matrix: {key}")
            mapped_scheduler.add(key)

        for mapping in entry.get("analysis_mappings", []):
            key = mapping["key"]
            _assert(mapping["implementation_status"] in ALLOWED_STATUS, f"invalid analysis status for {key}")
            _assert(key in analysis_inventory, f"unknown analysis key in matrix: {key}")
            _assert(key not in mapped_analysis, f"analysis key duplicated in matrix: {key}")
            mapped_analysis.add(key)

        traces = entry.get("paper_traceability", [])
        _assert(isinstance(traces, list) and traces, f"missing paper_traceability for class {class_id}")

        traced_scheduler_keys: set[str] = set()
        traced_analysis_keys: set[str] = set()

        for trace in traces:
            _assert(trace.get("paper_class"), f"paper_class missing in class {class_id}")
            evidence_paths = trace.get("implementation_evidence_paths", [])
            _assert(
                isinstance(evidence_paths, list) and evidence_paths,
                f"implementation_evidence_paths missing for class {class_id}",
            )
            for rel in evidence_paths:
                _assert((repo_root / rel).exists(), f"evidence path does not exist: {rel}")

            for key in trace.get("covered_scheduler_keys", []):
                _assert(key in class_scheduler_keys, f"traced scheduler key not in class mapping: {key}")
                traced_scheduler_keys.add(key)

            for key in trace.get("covered_analysis_keys", []):
                _assert(key in class_analysis_keys, f"traced analysis key not in class mapping: {key}")
                traced_analysis_keys.add(key)

        _assert(
            traced_scheduler_keys == class_scheduler_keys,
            f"paper_traceability scheduler coverage mismatch in {class_id}",
        )
        _assert(
            traced_analysis_keys == class_analysis_keys,
            f"paper_traceability analysis coverage mismatch in {class_id}",
        )

    _assert(set(scheduler_inventory.keys()) == mapped_scheduler, "scheduler mappings must match exported inventory")
    _assert(analysis_inventory == mapped_analysis, "analysis mappings must match exported inventory")


def validate_from_paths(repo_root: Path) -> None:
    matrix_path = repo_root / "docs/superpowers/specs/survey-taxonomy-matrix.json"
    scheduler_catalog_path = repo_root / "crates/hprss-sim/src/scheduler_catalog.rs"
    main_rs_path = repo_root / "crates/hprss-sim/src/main.rs"

    matrix = json.loads(matrix_path.read_text(encoding="utf-8"))
    scheduler_inventory = extract_scheduler_inventory(scheduler_catalog_path)
    analysis_inventory = extract_analysis_inventory(main_rs_path)
    validate_matrix(matrix, scheduler_inventory, analysis_inventory, repo_root)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate survey taxonomy matrix against scheduler/analysis inventory.")
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="Path to hprss-sim repository root (default: script-derived).",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        validate_from_paths(args.repo_root.resolve())
    except Exception as exc:
        print(f"[FAIL] survey taxonomy matrix validation failed: {exc}")
        return 1
    print("[OK] survey taxonomy matrix is consistent with scheduler/analysis inventory")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
