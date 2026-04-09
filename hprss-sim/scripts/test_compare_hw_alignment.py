from __future__ import annotations

import csv
import importlib.util
import json
import shutil
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("compare_hw_alignment.py")
SPEC = importlib.util.spec_from_file_location("compare_hw_alignment", SCRIPT_PATH)
compare_hw_alignment = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(compare_hw_alignment)


class CompareHwAlignmentTests(unittest.TestCase):
    def setUp(self) -> None:
        self.artifacts_dir = Path(__file__).with_name(".hw-align-test-artifacts")
        self.artifacts_dir.mkdir(exist_ok=True)

    def tearDown(self) -> None:
        if self.artifacts_dir.exists():
            shutil.rmtree(self.artifacts_dir)

    def _write_csv(
        self,
        path: Path,
        fieldnames: list[str],
        rows: list[dict[str, object]],
    ) -> Path:
        with path.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.DictWriter(handle, fieldnames=fieldnames)
            writer.writeheader()
            for row in rows:
                writer.writerow(row)
        return path

    def test_compute_report_matches_and_summarizes_deltas(self) -> None:
        fields = [
            "algorithm_key",
            "analysis_mode",
            "utilization",
            "task_count",
            "seed",
            "miss_ratio",
            "makespan",
            "avg_response_time",
            "deadline_misses",
            "completed_jobs",
        ]
        sim_csv = self._write_csv(
            self.artifacts_dir / "sim.csv",
            fields,
            [
                {
                    "algorithm_key": "fp",
                    "analysis_mode": "none",
                    "utilization": 0.6,
                    "task_count": 10,
                    "seed": 1,
                    "miss_ratio": 0.10,
                    "makespan": 1000,
                    "avg_response_time": 100,
                    "deadline_misses": 2,
                    "completed_jobs": 48,
                },
                {
                    "algorithm_key": "edf",
                    "analysis_mode": "none",
                    "utilization": 0.6,
                    "task_count": 10,
                    "seed": 1,
                    "miss_ratio": 0.02,
                    "makespan": 900,
                    "avg_response_time": 80,
                    "deadline_misses": 1,
                    "completed_jobs": 49,
                },
            ],
        )

        hw_csv = self._write_csv(
            self.artifacts_dir / "hw.csv",
            fields,
            [
                {
                    "algorithm_key": "fp",
                    "analysis_mode": "none",
                    "utilization": 0.6,
                    "task_count": 10,
                    "seed": 1,
                    "miss_ratio": 0.12,
                    "makespan": 1020,
                    "avg_response_time": 110,
                    "deadline_misses": 3,
                    "completed_jobs": 47,
                },
                {
                    "algorithm_key": "edf",
                    "analysis_mode": "none",
                    "utilization": 0.6,
                    "task_count": 10,
                    "seed": 1,
                    "miss_ratio": 0.01,
                    "makespan": 910,
                    "avg_response_time": 82,
                    "deadline_misses": 0,
                    "completed_jobs": 50,
                },
            ],
        )

        sim_rows, sim_fields = compare_hw_alignment.load_csv(sim_csv)
        hw_rows, hw_fields = compare_hw_alignment.load_csv(hw_csv)
        report = compare_hw_alignment.compute_report(sim_rows, sim_fields, hw_rows, hw_fields)

        self.assertEqual(report["join_keys"], compare_hw_alignment.STABLE_KEYS)
        self.assertEqual(report["matched_rows"], 2)
        self.assertEqual(report["metrics"]["miss_ratio"]["count"], 2)
        self.assertAlmostEqual(report["metrics"]["miss_ratio"]["max_abs_delta"], 0.02)
        self.assertAlmostEqual(report["metrics"]["miss_ratio"]["mean_abs_delta"], 0.015)
        self.assertAlmostEqual(report["metrics"]["completed_jobs"]["mean_abs_delta"], 1.0)

        row_by_algo = {
            item["key"]["algorithm_key"]: item
            for item in report["rows"]
        }
        self.assertAlmostEqual(
            row_by_algo["fp"]["deltas"]["avg_response_time"]["delta"],
            -10.0,
        )

    def test_main_writes_json_report(self) -> None:
        sim_fields = [
            "algorithm_key",
            "utilization",
            "task_count",
            "seed",
            "miss_ratio",
            "makespan",
            "avg_response_time",
            "deadline_misses",
            "completed_jobs",
        ]
        sim_csv = self._write_csv(
            self.artifacts_dir / "sim_min.csv",
            sim_fields,
            [
                {
                    "algorithm_key": "fp",
                    "utilization": 0.5,
                    "task_count": 8,
                    "seed": 7,
                    "miss_ratio": 0.05,
                    "makespan": 500,
                    "avg_response_time": 55,
                    "deadline_misses": 1,
                    "completed_jobs": 20,
                }
            ],
        )
        hw_csv = self._write_csv(
            self.artifacts_dir / "hw_min.csv",
            sim_fields,
            [
                {
                    "algorithm_key": "fp",
                    "utilization": 0.5,
                    "task_count": 8,
                    "seed": 7,
                    "miss_ratio": 0.03,
                    "makespan": 530,
                    "avg_response_time": 50,
                    "deadline_misses": 2,
                    "completed_jobs": 19,
                }
            ],
        )

        output_path = self.artifacts_dir / "report.json"
        code = compare_hw_alignment.main(
            [
                "--sim-csv",
                str(sim_csv),
                "--hw-csv",
                str(hw_csv),
                "--output",
                str(output_path),
            ]
        )

        self.assertEqual(code, 0)
        data = json.loads(output_path.read_text(encoding="utf-8"))
        self.assertEqual(data["matched_rows"], 1)
        self.assertEqual(data["join_keys"], ["algorithm_key", "utilization", "task_count", "seed"])
        self.assertAlmostEqual(data["metrics"]["makespan"]["max_abs_delta"], 30.0)


if __name__ == "__main__":
    unittest.main()
