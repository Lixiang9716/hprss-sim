from __future__ import annotations

import csv
import importlib.util
import json
import shutil
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("plot_experiments.py")
SPEC = importlib.util.spec_from_file_location("plot_experiments", SCRIPT_PATH)
plot_experiments = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(plot_experiments)


class PlotExperimentsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.artifacts_dir = Path(__file__).with_name(".plot-test-artifacts")
        self.artifacts_dir.mkdir(exist_ok=True)

    def tearDown(self) -> None:
        if self.artifacts_dir.exists():
            shutil.rmtree(self.artifacts_dir)

    def test_deterministic_aggregation_from_sample_csv(self) -> None:
        csv_path = self.artifacts_dir / "sample.csv"
        fields = [
            "utilization",
            "algorithm",
            "miss_ratio",
            "schedulable",
            "makespan",
            "avg_response_time",
            "wall_time_us",
        ]
        rows = [
            [0.5, "FP-Het", 0.0, "true", 1200, 300, 100],
            [0.5, "FP-Het", 0.2, "false", 1500, 350, 140],
            [0.5, "EDF-Het", 0.0, "true", 1100, 280, 90],
            [0.7, "EDF-Het", 0.1, "false", 1300, 320, 110],
        ]
        with csv_path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(fields)
            writer.writerows(rows)

        parsed = plot_experiments.load_sweep_rows(csv_path)
        curves = plot_experiments.aggregate_sched_miss_by_algorithm(parsed)
        fp_u05 = [point for point in curves["FP-Het"] if abs(point[0] - 0.5) < 1e-9][0]
        self.assertAlmostEqual(fp_u05[1], 0.5)
        self.assertAlmostEqual(fp_u05[2], 0.1)

        wall_stats = {
            algorithm: mean
            for algorithm, mean, _std in plot_experiments.aggregate_metric_by_algorithm(
                parsed, "wall_time_us"
            )
        }
        self.assertAlmostEqual(wall_stats["FP-Het"], 120.0)
        self.assertAlmostEqual(wall_stats["EDF-Het"], 100.0)

    def test_trace_loader_accepts_time_and_t_keys(self) -> None:
        trace_path = self.artifacts_dir / "trace.jsonl"
        records = [
            {"event": "job_complete", "time": 10},
            {"event": "deadline_miss", "t": 20},
        ]
        with trace_path.open("w", encoding="utf-8") as handle:
            for record in records:
                handle.write(json.dumps(record) + "\n")

        events = plot_experiments.load_trace_jsonl(trace_path)
        self.assertEqual(events["job_complete"], [10.0])
        self.assertEqual(events["deadline_miss"], [20.0])


if __name__ == "__main__":
    unittest.main()
