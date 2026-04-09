from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("check_survey_taxonomy_matrix.py")
SPEC = importlib.util.spec_from_file_location("check_survey_taxonomy_matrix", SCRIPT_PATH)
validator = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(validator)


class SurveyTaxonomyMatrixValidationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repo_root = Path(__file__).parents[1]

    def test_inventory_extractors_have_expected_core_keys(self) -> None:
        sched = validator.extract_scheduler_inventory(
            self.repo_root / "crates/hprss-sim/src/scheduler_catalog.rs"
        )
        analysis = validator.extract_analysis_inventory(
            self.repo_root / "crates/hprss-sim/src/main.rs"
        )

        self.assertIn("fp", sched)
        self.assertEqual(sched["fp"], "FP-Het")
        self.assertIn("gpu-preemptive-priority", sched)
        self.assertIn("none", analysis)
        self.assertIn("simso-scope-extension", analysis)

    def test_current_matrix_validates_against_inventory(self) -> None:
        validator.validate_from_paths(self.repo_root)


if __name__ == "__main__":
    unittest.main()
