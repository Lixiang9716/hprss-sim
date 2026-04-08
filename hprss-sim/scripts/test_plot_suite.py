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
plot_experiments = load_module("plot_experiments")


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

    def test_response_helpers_group_and_cdf_monotonic(self) -> None:
        rows = [
            {"algorithm": "B", "avg_response_time": 30.0},
            {"algorithm": "A", "avg_response_time": 20.0},
            {"algorithm": "A", "avg_response_time": 10.0},
        ]
        grouped = plot_response_time.metric_samples_by_algorithm(rows, "avg_response_time")
        self.assertEqual(list(grouped.keys()), ["A", "B"])
        self.assertEqual(grouped["A"], [10.0, 20.0])
        xs, ys = plot_response_time.cdf_points(grouped["A"])
        self.assertEqual(xs, [10.0, 20.0])
        self.assertEqual(ys, [0.5, 1.0])
        self.assertTrue(all(ys[i] <= ys[i + 1] for i in range(len(ys) - 1)))

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
        with self.assertRaisesRegex(
            ValueError, rf"{bad_trace}:1:job_id"
        ):
            plot_gantt.load_trace_rows(bad_trace)

    def test_gantt_loader_rejects_invalid_numeric_field_with_context(self) -> None:
        bad_trace = self.artifacts_dir / "bad_time_trace.jsonl"
        with bad_trace.open("w", encoding="utf-8") as handle:
            handle.write('{"event":"job_complete","time":"bad","task_id":1,"job_id":3}\n')
        with self.assertRaisesRegex(
            ValueError, rf"{bad_trace}:1:time"
        ):
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

    def test_response_plot_loader_requires_only_algorithm_and_selected_metric(self) -> None:
        minimal_csv = self.artifacts_dir / "minimal_avg.csv"
        with minimal_csv.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(["algorithm", "avg_response_time"])
            writer.writerow(["FP-Het", 123.0])
            writer.writerow(["EDF-Het", 98.0])

        rows = plot_response_time.load_rows_for_metric(minimal_csv, "avg_response_time")
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["algorithm"], "FP-Het")
        self.assertEqual(rows[1]["avg_response_time"], 98.0)

    def test_schedulability_aggregation_deterministic_values(self) -> None:
        rows = [
            {"algorithm": "FP-Het", "utilization": 0.7, "miss_ratio": 0.2, "schedulable": False},
            {"algorithm": "FP-Het", "utilization": 0.5, "miss_ratio": 0.0, "schedulable": True},
            {"algorithm": "FP-Het", "utilization": 0.7, "miss_ratio": 0.0, "schedulable": True},
            {"algorithm": "EDF-Het", "utilization": 0.5, "miss_ratio": 0.1, "schedulable": False},
            {"algorithm": "EDF-Het", "utilization": 0.5, "miss_ratio": 0.0, "schedulable": True},
        ]
        curves = plot_experiments.aggregate_sched_miss_by_algorithm(rows)
        self.assertEqual([point[0] for point in curves["FP-Het"]], [0.5, 0.7])
        fp_u07 = curves["FP-Het"][1]
        self.assertAlmostEqual(fp_u07[1], 0.5)
        self.assertAlmostEqual(fp_u07[2], 0.1)
        edf_u05 = curves["EDF-Het"][0]
        self.assertAlmostEqual(edf_u05[1], 0.5)
        self.assertAlmostEqual(edf_u05[2], 0.05)


if __name__ == "__main__":
    unittest.main()
