from __future__ import annotations

import importlib.util
import json
import shutil
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("build_evidence_bundle.py")
SPEC = importlib.util.spec_from_file_location("build_evidence_bundle", SCRIPT_PATH)
bundle = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(bundle)


class BuildEvidenceBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repo_root = Path(__file__).with_name(".evidence-bundle-test-repo")
        if self.repo_root.exists():
            shutil.rmtree(self.repo_root)
        (self.repo_root / ".git").mkdir(parents=True, exist_ok=True)
        (
            self.repo_root / "artifacts/reproduction/alg-paper-reproduction-suite"
        ).mkdir(parents=True, exist_ok=True)
        (self.repo_root / "plots").mkdir(parents=True, exist_ok=True)

        (self.repo_root / "artifacts/reproduction/alg-paper-reproduction-suite/manifest.json").write_text(
            '{"suite":"alg-paper-reproduction-suite"}\n',
            encoding="utf-8",
        )
        (self.repo_root / "artifacts/reproduction/alg-paper-reproduction-suite/sweep.csv").write_text(
            "algorithm_key,schedulable\nfp,true\n",
            encoding="utf-8",
        )
        (self.repo_root / "artifacts/reproduction/alg-paper-reproduction-suite/suite_records.jsonl").write_text(
            '{"suite":"alg-paper-reproduction-suite"}\n',
            encoding="utf-8",
        )
        (self.repo_root / "plots/schedulability.png").write_bytes(b"fakepng")

    def tearDown(self) -> None:
        if self.repo_root.exists():
            shutil.rmtree(self.repo_root)

    def test_build_bundle_writes_manifest_and_copies_files(self) -> None:
        manifest_path = bundle.build_evidence_bundle(
            repo_root=self.repo_root,
            commit_hash="abc123",
            generated_at="2025-01-01T00:00:00Z",
        )
        self.assertEqual(
            manifest_path,
            self.repo_root / "artifacts/evidence/latest/manifest.json",
        )
        payload = json.loads(manifest_path.read_text(encoding="utf-8"))
        self.assertEqual(payload["git_commit"], "abc123")
        self.assertEqual(payload["generated_at_utc"], "2025-01-01T00:00:00Z")
        self.assertGreaterEqual(len(payload["files"]), 4)

        copied = self.repo_root / "artifacts/evidence/latest/reproduction/sweep.csv"
        self.assertTrue(copied.exists())
        copied_plot = self.repo_root / "artifacts/evidence/latest/plots/schedulability.png"
        self.assertTrue(copied_plot.exists())

    def test_missing_required_reproduction_file_raises(self) -> None:
        (self.repo_root / "artifacts/reproduction/alg-paper-reproduction-suite/sweep.csv").unlink()
        with self.assertRaises(FileNotFoundError):
            bundle.build_evidence_bundle(
                repo_root=self.repo_root,
                commit_hash="abc123",
                generated_at="2025-01-01T00:00:00Z",
            )


if __name__ == "__main__":
    unittest.main()
