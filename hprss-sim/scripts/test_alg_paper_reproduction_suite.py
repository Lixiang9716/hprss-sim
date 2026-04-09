from __future__ import annotations

import importlib.util
import io
import json
import shutil
import unittest
from contextlib import redirect_stdout
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("alg_paper_reproduction_suite.py")
SPEC = importlib.util.spec_from_file_location("alg_paper_reproduction_suite", SCRIPT_PATH)
repro_suite = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(repro_suite)


class ReproductionSuiteTests(unittest.TestCase):
    def setUp(self) -> None:
        self.artifacts_dir = Path(__file__).with_name(".repro-suite-test-artifacts")
        self.artifacts_dir.mkdir(exist_ok=True)

    def tearDown(self) -> None:
        if self.artifacts_dir.exists():
            shutil.rmtree(self.artifacts_dir)

    def test_load_preset_contains_new_algorithm_families(self) -> None:
        preset_path = Path(__file__).parents[1] / "configs/repro/alg_paper_reproduction_suite.json"
        preset = repro_suite.load_preset(preset_path)
        schedulers = set(preset["sweep"]["schedulers"])
        self.assertIn("global-edf", schedulers)
        self.assertIn("gang", schedulers)
        self.assertIn("xsched", schedulers)
        self.assertIn("gpu-preemptive-priority", schedulers)

        analysis_modes = set(preset["sweep"]["analysis_modes"])
        self.assertIn("none", analysis_modes)
        self.assertIn("rta-uniform-global-fp-scaffold", analysis_modes)
        self.assertIn("util-vectors", analysis_modes)
        self.assertIn("shape", analysis_modes)

        self.assertGreaterEqual(len(preset["openmp_runs"]), 1)
        self.assertGreaterEqual(len(preset["karami_runs"]), 1)
        self.assertTrue(
            any("karami_profile_json" in run for run in preset["karami_runs"])
        )

    def test_parse_run_summary_extracts_machine_readable_fields(self) -> None:
        summary = """HPRSS Simulation Summary
algorithm      : Global-EDF
algorithm_key  : global-edf
algorithm_family: deadline-driven
analysis_mode  : none
analysis_scope : none
analysis_assumption_count: 0
workload_source: replay-openmp
approximation_assumption_count: 0
total_jobs      : 12
completed_jobs  : 12
deadline_misses : 0
miss_ratio      : 0.000000
schedulable     : true
makespan_ns     : 20000
avg_response_ns : 123.0
worst_response_ns: 456
transfer_overhead_ns: 0
preemption_count : 0
migration_count  : 0
bus_contention_ratio: 0.0
energy_total_joules: 0.001
events_processed: 77
wall_time_us    : 99
"""
        parsed = repro_suite.parse_run_summary(summary)
        self.assertEqual(parsed["algorithm_key"], "global-edf")
        self.assertEqual(parsed["analysis_mode"], "none")
        self.assertEqual(parsed["analysis_assumption_count"], 0)
        self.assertEqual(parsed["total_jobs"], 12)
        self.assertEqual(parsed["schedulable"], True)

    def test_normalize_sweep_row_typed_fields(self) -> None:
        row = repro_suite.normalize_sweep_row(
            {
                "utilization": "0.55",
                "seed": "7",
                "algorithm_key": "global-edf",
                "analysis_mode": "none",
                "schedulable": "true",
                "total_jobs": "12",
                "miss_ratio": "0.0",
            }
        )
        self.assertEqual(row["seed"], 7)
        self.assertEqual(row["total_jobs"], 12)
        self.assertEqual(row["utilization"], 0.55)
        self.assertEqual(row["miss_ratio"], 0.0)
        self.assertTrue(row["schedulable"])

    def test_write_jsonl_is_stable_and_shape_checked(self) -> None:
        rows = [
            {
                "suite": "alg-paper-reproduction-suite",
                "algorithm_key": "gang",
                "analysis_mode": "none",
                "seed": 7,
                "platform_config_hash": "abc",
            },
            {
                "suite": "alg-paper-reproduction-suite",
                "algorithm_key": "global-edf",
                "analysis_mode": "rta-uniform-global-fp-scaffold",
                "seed": 13,
                "platform_config_hash": "abc",
            },
        ]
        output = self.artifacts_dir / "suite.jsonl"
        repro_suite.write_jsonl(output, rows)

        lines = output.read_text(encoding="utf-8").strip().splitlines()
        self.assertEqual(len(lines), 2)
        decoded = [json.loads(line) for line in lines]
        self.assertEqual(decoded[0]["algorithm_key"], "gang")
        self.assertIn("platform_config_hash", decoded[1])

    def test_dry_run_includes_sweep_openmp_and_karami_command_paths(self) -> None:
        repo_root = Path(__file__).parents[1]
        preset_path = repo_root / "configs/repro/alg_paper_reproduction_suite.json"
        preset = repro_suite.load_preset(preset_path)
        capture = io.StringIO()
        with redirect_stdout(capture):
            rc = repro_suite.run_suite(
                preset=preset,
                repo_root=repo_root,
                archive_dir=self.artifacts_dir / "dry-run",
                dry_run=True,
            )
        self.assertEqual(rc, 0)
        payload = json.loads(capture.getvalue())
        commands = payload["commands"]
        self.assertGreaterEqual(len(commands), 3)
        joined = [" ".join(cmd) for cmd in commands]
        self.assertTrue(any("--schedulers" in cmd and "xsched" in cmd and "gpu-preemptive-priority" in cmd for cmd in joined))
        self.assertTrue(any("--openmp-specialized-json" in cmd for cmd in joined))
        self.assertTrue(any("--karami-profile-json" in cmd for cmd in joined))

    def test_dry_run_handles_missing_karami_runs_key(self) -> None:
        repo_root = Path(__file__).parents[1]
        preset_path = repo_root / "configs/repro/alg_paper_reproduction_suite.json"
        preset = repro_suite.load_preset(preset_path)
        preset.pop("karami_runs", None)
        capture = io.StringIO()
        with redirect_stdout(capture):
            rc = repro_suite.run_suite(
                preset=preset,
                repo_root=repo_root,
                archive_dir=self.artifacts_dir / "dry-run-no-karami",
                dry_run=True,
            )
        self.assertEqual(rc, 0)
        payload = json.loads(capture.getvalue())
        commands = payload["commands"]
        joined = [" ".join(cmd) for cmd in commands]
        self.assertFalse(any("--karami-profile-json" in cmd for cmd in joined))


if __name__ == "__main__":
    unittest.main()
