#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


RUN_INT_FIELDS = {
    "analysis_assumption_count",
    "approximation_assumption_count",
    "total_jobs",
    "completed_jobs",
    "deadline_misses",
    "makespan_ns",
    "worst_response_ns",
    "transfer_overhead_ns",
    "preemption_count",
    "migration_count",
    "events_processed",
    "wall_time_us",
}
RUN_FLOAT_FIELDS = {
    "miss_ratio",
    "avg_response_ns",
    "bus_contention_ratio",
    "energy_total_joules",
}
RUN_BOOL_FIELDS = {"schedulable"}
RUN_TEXT_FIELDS = {
    "algorithm",
    "algorithm_key",
    "algorithm_family",
    "analysis_mode",
    "analysis_scope",
    "workload_source",
    "approximation_assumptions",
}
SWEEP_INT_FIELDS = {
    "task_count",
    "seed",
    "analysis_assumption_count",
    "approximation_assumption_count",
    "total_jobs",
    "completed_jobs",
    "deadline_misses",
    "makespan",
    "events_processed",
    "wall_time_us",
    "transfer_overhead",
    "worst_response_time",
    "preemption_count",
    "migration_count",
}
SWEEP_FLOAT_FIELDS = {
    "utilization",
    "miss_ratio",
    "avg_response_time",
    "bus_contention_ratio",
    "energy_total_joules",
}
SWEEP_BOOL_FIELDS = {"schedulable"}


def load_preset(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for row in rows:
            handle.write(json.dumps(row, sort_keys=True, separators=(",", ":")) + "\n")


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_run_summary(stdout: str) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for line in stdout.splitlines():
        line = re.sub(r"\x1b\[[0-9;]*m", "", line)
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        key = key.strip().replace(" ", "_")
        value = value.strip()
        if key in RUN_INT_FIELDS:
            out[key] = int(value)
        elif key in RUN_FLOAT_FIELDS:
            out[key] = float(value)
        elif key in RUN_BOOL_FIELDS:
            out[key] = value.lower() == "true"
        elif key in RUN_TEXT_FIELDS:
            out[key] = value
        else:
            continue
    return out


def normalize_sweep_row(raw: dict[str, str]) -> dict[str, Any]:
    row: dict[str, Any] = dict(raw)
    for key in SWEEP_INT_FIELDS:
        if key in row and row[key] != "":
            row[key] = int(row[key])
    for key in SWEEP_FLOAT_FIELDS:
        if key in row and row[key] != "":
            row[key] = float(row[key])
    for key in SWEEP_BOOL_FIELDS:
        if key in row and row[key] != "":
            row[key] = row[key].strip().lower() == "true"
    return row


def _f64_token(value: float) -> str:
    token = f"{value:.6f}".rstrip("0").rstrip(".")
    return token if token else "0"


def _contiguous_seed_range(seeds: list[int]) -> str:
    if not seeds:
        raise ValueError("sweep.seeds must not be empty")
    if len(seeds) == 1:
        return str(seeds[0])
    expected = list(range(seeds[0], seeds[0] + len(seeds)))
    if seeds != expected:
        raise ValueError("sweep.seeds must be contiguous to map into hprss-sim range syntax")
    return f"{seeds[0]}:{seeds[-1]}"


def _utilization_token(utils: list[float]) -> str:
    if not utils:
        raise ValueError("sweep.utilizations must not be empty")
    if len(utils) == 1:
        return _f64_token(utils[0])
    step = round(utils[1] - utils[0], 6)
    for idx in range(1, len(utils)):
        if round(utils[idx] - utils[idx - 1], 6) != step:
            raise ValueError("sweep.utilizations must be arithmetic progression")
    return f"{_f64_token(utils[0])}:{_f64_token(step)}:{_f64_token(utils[-1])}"


def run_command(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True)


def run_suite(preset: dict[str, Any], repo_root: Path, archive_dir: Path, dry_run: bool) -> int:
    platform = repo_root / preset["platform"]
    platform_hash = file_sha256(platform)
    archive_dir.mkdir(parents=True, exist_ok=True)

    commands: list[list[str]] = []
    records: list[dict[str, Any]] = []

    sweep = preset["sweep"]
    sweep_csv = archive_dir / "sweep.csv"
    sweep_cmd = [
        "cargo",
        "run",
        "--release",
        "-p",
        "hprss-sim",
        "--",
        "--platform",
        preset["platform"],
        "sweep",
        "--utilizations",
        _utilization_token(sweep["utilizations"]),
        "--task-counts",
        ",".join(str(v) for v in sweep["task_counts"]),
        "--seeds",
        _contiguous_seed_range(sweep["seeds"]),
        "--schedulers",
        ",".join(sweep["schedulers"]),
        "--analysis-modes",
        ",".join(sweep["analysis_modes"]),
        "--jobs",
        str(sweep.get("jobs", 1)),
        "--output",
        str(sweep_csv),
    ]
    commands.append(sweep_cmd)

    openmp_runs = preset.get("openmp_runs", [])
    karami_runs = preset.get("karami_runs", [])
    for run in openmp_runs:
        commands.append(
            [
                "cargo",
                "run",
                "--release",
                "-p",
                "hprss-sim",
                "--",
                "--platform",
                preset["platform"],
                "--seed",
                str(run["seed"]),
                "--scheduler",
                run["scheduler"],
                "--analysis-mode",
                run["analysis_mode"],
                "--openmp-specialized-json",
                run["openmp_specialized_json"],
                "run",
            ]
        )
    for run in karami_runs:
        commands.append(
            [
                "cargo",
                "run",
                "--release",
                "-p",
                "hprss-sim",
                "--",
                "--platform",
                preset["platform"],
                "--seed",
                str(run["seed"]),
                "--scheduler",
                run["scheduler"],
                "--analysis-mode",
                run["analysis_mode"],
                "--karami-profile-json",
                run["karami_profile_json"],
                "run",
            ]
        )

    if dry_run:
        print(json.dumps({"commands": commands}, indent=2))
        return 0

    sweep_completed = run_command(sweep_cmd, repo_root)
    if sweep_completed.returncode != 0:
        raise RuntimeError(
            f"sweep command failed ({sweep_completed.returncode}): {sweep_completed.stderr.strip()}"
        )
    with sweep_csv.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            normalized = normalize_sweep_row(row)
            normalized["suite"] = preset["suite"]
            normalized["scenario_type"] = "synthetic-sweep"
            normalized["scenario_status"] = "ok"
            normalized["platform_config"] = preset["platform"]
            normalized["platform_config_hash"] = platform_hash
            records.append(normalized)

    for run in openmp_runs:
        cmd = [
            "cargo",
            "run",
            "--release",
            "-p",
            "hprss-sim",
            "--",
            "--platform",
            preset["platform"],
            "--seed",
            str(run["seed"]),
            "--scheduler",
            run["scheduler"],
            "--analysis-mode",
            run["analysis_mode"],
            "--openmp-specialized-json",
            run["openmp_specialized_json"],
            "run",
        ]
        completed = run_command(cmd, repo_root)
        log_path = archive_dir / f"{run['name']}.log"
        merged_log = completed.stdout
        if completed.stderr:
            merged_log += "\n[stderr]\n" + completed.stderr
        log_path.write_text(merged_log, encoding="utf-8")

        if completed.returncode == 0:
            parsed = parse_run_summary(completed.stdout)
            parsed["scenario_status"] = "ok"
        else:
            parsed = {
                "scenario_status": "failed",
                "command_exit_code": completed.returncode,
                "error": completed.stderr.strip() or "command failed",
            }
        parsed["suite"] = preset["suite"]
        parsed["scenario_type"] = "openmp-adapter"
        parsed["scenario_name"] = run["name"]
        parsed["seed"] = run["seed"]
        parsed["platform_config"] = preset["platform"]
        parsed["platform_config_hash"] = platform_hash
        parsed["openmp_specialized_json"] = run["openmp_specialized_json"]
        records.append(parsed)

    for run in karami_runs:
        cmd = [
            "cargo",
            "run",
            "--release",
            "-p",
            "hprss-sim",
            "--",
            "--platform",
            preset["platform"],
            "--seed",
            str(run["seed"]),
            "--scheduler",
            run["scheduler"],
            "--analysis-mode",
            run["analysis_mode"],
            "--karami-profile-json",
            run["karami_profile_json"],
            "run",
        ]
        completed = run_command(cmd, repo_root)
        log_path = archive_dir / f"{run['name']}.log"
        merged_log = completed.stdout
        if completed.stderr:
            merged_log += "\n[stderr]\n" + completed.stderr
        log_path.write_text(merged_log, encoding="utf-8")

        if completed.returncode == 0:
            parsed = parse_run_summary(completed.stdout)
            parsed["scenario_status"] = "ok"
        else:
            parsed = {
                "scenario_status": "failed",
                "command_exit_code": completed.returncode,
                "error": completed.stderr.strip() or "command failed",
            }
        parsed["suite"] = preset["suite"]
        parsed["scenario_type"] = "karami-paper-profile"
        parsed["scenario_name"] = run["name"]
        parsed["seed"] = run["seed"]
        parsed["platform_config"] = preset["platform"]
        parsed["platform_config_hash"] = platform_hash
        parsed["karami_profile_json"] = run["karami_profile_json"]
        records.append(parsed)

    records_path = archive_dir / "suite_records.jsonl"
    write_jsonl(records_path, records)

    manifest = {
        "suite": preset["suite"],
        "platform_config": preset["platform"],
        "platform_config_hash": platform_hash,
        "record_count": len(records),
        "outputs": {
            "sweep_csv": str(sweep_csv),
            "suite_records_jsonl": str(records_path),
        },
        "commands": commands,
    }
    (archive_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8"
    )

    print(f"Reproduction suite complete: {records_path} ({len(records)} rows)")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run reproducible algorithm paper suite")
    parser.add_argument(
        "--preset",
        type=Path,
        default=Path("configs/repro/alg_paper_reproduction_suite.json"),
        help="Preset JSON path",
    )
    parser.add_argument(
        "--archive-dir",
        type=Path,
        default=None,
        help="Override archive directory",
    )
    parser.add_argument("--dry-run", action="store_true", help="Print commands only")
    args = parser.parse_args(argv)

    repo_root = Path(__file__).resolve().parents[1]
    preset_path = (repo_root / args.preset).resolve() if not args.preset.is_absolute() else args.preset
    preset = load_preset(preset_path)
    archive_dir = (
        (repo_root / args.archive_dir) if args.archive_dir is not None else (repo_root / preset["archive_dir"])
    )

    try:
        return run_suite(preset, repo_root, archive_dir, args.dry_run)
    except Exception as exc:  # noqa: BLE001
        print(str(exc), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
