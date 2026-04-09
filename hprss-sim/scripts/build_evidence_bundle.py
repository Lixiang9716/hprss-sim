#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

REPRO_REL = Path("artifacts/reproduction/alg-paper-reproduction-suite")
PLOTS_REL = Path("plots")
BUNDLE_REL = Path("artifacts/evidence/latest")
REQUIRED_REPRO_FILES = (
    "manifest.json",
    "sweep.csv",
    "suite_records.jsonl",
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def gather_files(base_dir: Path) -> list[Path]:
    return sorted([p for p in base_dir.rglob("*") if p.is_file()])


def resolve_git_commit(repo_root: Path) -> str:
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repo_root,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "failed to resolve git commit")
    return completed.stdout.strip()


def build_evidence_bundle(repo_root: Path, commit_hash: str, generated_at: str) -> Path:
    repro_dir = repo_root / REPRO_REL
    if not repro_dir.exists():
        raise FileNotFoundError(f"missing reproduction directory: {repro_dir}")

    for filename in REQUIRED_REPRO_FILES:
        required = repro_dir / filename
        if not required.is_file():
            raise FileNotFoundError(f"missing required reproduction file: {required}")

    source_entries: list[tuple[Path, Path]] = []
    for src in gather_files(repro_dir):
        rel = src.relative_to(repro_dir)
        source_entries.append((src, Path("reproduction") / rel))

    plots_dir = repo_root / PLOTS_REL
    if plots_dir.exists():
        for src in gather_files(plots_dir):
            rel = src.relative_to(plots_dir)
            source_entries.append((src, Path("plots") / rel))

    source_entries = sorted(source_entries, key=lambda item: item[1].as_posix())

    bundle_dir = repo_root / BUNDLE_REL
    if bundle_dir.exists():
        shutil.rmtree(bundle_dir)
    bundle_dir.mkdir(parents=True, exist_ok=True)

    files_manifest: list[dict[str, str]] = []
    for src, bundle_rel in source_entries:
        target = bundle_dir / bundle_rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, target)
        files_manifest.append(
            {
                "source_path": src.relative_to(repo_root).as_posix(),
                "bundle_path": (BUNDLE_REL / bundle_rel).as_posix(),
                "sha256": sha256_file(target),
            }
        )

    manifest = {
        "git_commit": commit_hash,
        "generated_at_utc": generated_at,
        "bundle_root": BUNDLE_REL.as_posix(),
        "source_paths": [entry["source_path"] for entry in files_manifest],
        "files": files_manifest,
    }
    manifest_path = bundle_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True), encoding="utf-8")
    return manifest_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Build deterministic evidence bundle")
    parser.add_argument("--repo-root", type=Path, default=Path("."), help="Repository root")
    args = parser.parse_args(argv)

    repo_root = args.repo_root.resolve()
    generated_at = datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")

    try:
        manifest_path = build_evidence_bundle(
            repo_root=repo_root,
            commit_hash=resolve_git_commit(repo_root),
            generated_at=generated_at,
        )
    except Exception as exc:  # noqa: BLE001
        print(str(exc), file=sys.stderr)
        return 2

    print(f"Evidence bundle created: {manifest_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
