from __future__ import annotations

import csv
import importlib.util
import json
import shutil
import unittest
from pathlib import Path

SCRIPTS_DIR = Path(__file__).parent


def load_module(name: str):
    script = SCRIPTS_DIR / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, script)
    module = importlib.util.module_from_spec(spec)
    assert spec and spec.loader
    spec.loader.exec_module(module)
    return module


plot_schedulability = load_module("plot_schedulability")
plot_response_time = load_module("plot_response_time")
plot_comparison = load_module("plot_comparison")
plot_gantt = load_module("plot_gantt")


class PlotSuiteTests(unittest.TestCase):
    def setUp(self) -> None:
        self.artifacts_dir = SCRIPTS_DIR / ".plot-suite-test-artifacts"
        self.artifacts_dir.mkdir(exist_ok=True)
        self.csv_path = self.artifacts_dir / "sample.csv"
        self.trace_path = self.artifacts_dir / "sample_trace.jsonl"

        fields = [
            "utilization",
            "task_count",
            "seed",
            "algorithm",
            "total_jobs",
            "completed_jobs",
            "deadline_misses",
            "miss_ratio",
            "schedulable",
            "makespan",
            "avg_response_time",
            "events_processed",
            "wall_time_us",
            "config_hash",
            "git_commit",
            "timestamp",
            "per_device_utilization",
            "transfer_overhead",
            "blocking_breakdown",
            "worst_response_time",
            "preemption_count",
            "migration_count",
            "bus_contention_ratio",
            "energy_total_joules",
        ]
        rows = [
            [0.5, 10, 1, "FP-Het", 10, 10, 0, 0.0, "true", 1000, 200.0, 100, 50, "abc", "head", "1", "[]", 0, "{}", 250, 0, 0, 0.0, 1.0],
            [0.7, 10, 1, "FP-Het", 10, 8, 2, 0.2, "false", 1300, 260.0, 120, 70, "abc", "head", "1", "[]", 0, "{}", 320, 1, 0, 0.1, 1.1],
            [0.5, 10, 1, "EDF-Het", 10, 10, 0, 0.0, "true", 900, 180.0, 95, 45, "abc", "head", "1", "[]", 0, "{}", 240, 0, 0, 0.0, 0.9],
            [0.7, 10, 1, "EDF-Het", 10, 9, 1, 0.1, "false", 1200, 230.0, 115, 65, "abc", "head", "1", "[]", 0, "{}", 300, 1, 0, 0.05, 1.0],
        ]
        with self.csv_path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(fields)
            writer.writerows(rows)

        trace_records = [
            {"event": "job_complete", "time": 100.0, "task_id": 0, "job_id": 1},
            {"event": "deadline_miss", "time": 150.0, "task_id": 1, "job_id": 2},
            {"event": "job_complete", "t": 200.0, "task": 0, "job": 3},
        ]
        with self.trace_path.open("w", encoding="utf-8") as handle:
            for item in trace_records:
                handle.write(json.dumps(item) + "\n")

    def tearDown(self) -> None:
        if self.artifacts_dir.exists():
            shutil.rmtree(self.artifacts_dir)

    def test_schedulability_plot_cli(self) -> None:
        out = self.artifacts_dir / "sched.png"
        rc = plot_schedulability.main(["--csv", str(self.csv_path), "--output", str(out)])
        self.assertEqual(rc, 0)
        self.assertTrue(out.exists())

    def test_response_time_plot_cli(self) -> None:
        out = self.artifacts_dir / "response.svg"
        rc = plot_response_time.main(
            [
                "--csv",
                str(self.csv_path),
                "--output",
                str(out),
                "--metric",
                "worst_response_time",
                "--format",
                "svg",
            ]
        )
        self.assertEqual(rc, 0)
        self.assertTrue(out.exists())

    def test_comparison_plot_cli(self) -> None:
        out = self.artifacts_dir / "comparison.pdf"
        rc = plot_comparison.main(
            ["--csv", str(self.csv_path), "--output", str(out), "--format", "pdf"]
        )
        self.assertEqual(rc, 0)
        self.assertTrue(out.exists())

    def test_gantt_plot_cli(self) -> None:
        out = self.artifacts_dir / "gantt.png"
        rc = plot_gantt.main(["--trace-jsonl", str(self.trace_path), "--output", str(out)])
        self.assertEqual(rc, 0)
        self.assertTrue(out.exists())

    def test_gantt_loader_rejects_missing_required_fields(self) -> None:
        bad_trace = self.artifacts_dir / "bad_trace.jsonl"
        with bad_trace.open("w", encoding="utf-8") as handle:
            handle.write('{"event":"job_complete","time":10,"task_id":1}\n')
        with self.assertRaises(ValueError):
            plot_gantt.load_trace_rows(bad_trace)

    def test_response_plot_cli_reports_missing_metric_column(self) -> None:
        missing_csv = self.artifacts_dir / "missing_worst.csv"
        with missing_csv.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(
                [
                    "utilization",
                    "algorithm",
                    "miss_ratio",
                    "schedulable",
                    "makespan",
                    "avg_response_time",
                    "wall_time_us",
                ]
            )
            writer.writerow([0.5, "FP-Het", 0.0, "true", 1000, 200.0, 50.0])

        rc = plot_response_time.main(
            ["--csv", str(missing_csv), "--metric", "worst_response_time"]
        )
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main()
