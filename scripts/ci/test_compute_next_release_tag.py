#!/usr/bin/env python3
"""Parametrized unittest suite for `scripts/ci/compute_next_release_tag.py`."""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import compute_next_release_tag as planner  # noqa: E402


def _git(repo: pathlib.Path, *args: str) -> str:
    """Run a Git command in a temporary test repository."""
    proc = subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    )
    return proc.stdout.strip()


def _init_repo(tmp_dir: pathlib.Path) -> pathlib.Path:
    """Initialize a clean Git repository with deterministic author identity."""
    repo = tmp_dir / "repo"
    repo.mkdir()
    _git(repo, "init", "-b", "main")
    _git(repo, "config", "user.name", "Test Runner")
    _git(repo, "config", "user.email", "test@example.invalid")
    return repo


def _commit(repo: pathlib.Path, message: str, filename: str = "work.txt") -> None:
    """Create a commit with the provided message."""
    target = repo / filename
    prior = target.read_text(encoding="utf-8") if target.exists() else ""
    target.write_text(f"{prior}{message}\n", encoding="utf-8")
    _git(repo, "add", filename)
    _git(repo, "commit", "-m", message)


class TestComputeNextReleaseTag(unittest.TestCase):
    """Parametrized verification of SemVer planning and fail-closed invariants."""

    def test_initial_baseline_release_cases(self) -> None:
        cases = [
            ("initial-default-v0.1.0", None, "true", "v0.1.0", "0.1.0", ""),
            ("initial-with-override-v1.0.0", "1.0.0", "true", "v1.0.0", "1.0.0", ""),
        ]
        for case_id, override, exp_release, exp_tag, exp_ver, exp_prev in cases:
            with self.subTest(case=case_id):
                with tempfile.TemporaryDirectory() as tmp:
                    repo = _init_repo(pathlib.Path(tmp))
                    _commit(repo, "chore: bootstrap repository")
                    plan = planner.compute_next_release(repo, release_version=override)
                    self.assertEqual(
                        (
                            "true" if plan.should_release else "false",
                            plan.tag,
                            plan.version,
                            plan.previous_tag,
                        ),
                        (exp_release, exp_tag, exp_ver, exp_prev),
                    )

    def test_conventional_commit_bumps_from_existing_tags(self) -> None:
        cases = [
            (
                "zero-major-patch-from-fix",
                "v0.1.0",
                ["fix: handle empty palette"],
                True,
                "v0.1.1",
                "0.1.1",
            ),
            (
                "zero-major-patch-from-perf",
                "v0.1.0",
                ["perf(core): speed up resampling"],
                True,
                "v0.1.1",
                "0.1.1",
            ),
            (
                "zero-major-patch-from-chore-deps",
                "v0.1.0",
                ["chore(deps): bump image from 0.25.9 to 0.25.10"],
                True,
                "v0.1.1",
                "0.1.1",
            ),
            (
                "zero-major-patch-from-ci-deps",
                "v0.1.0",
                ["ci(deps): bump actions/checkout"],
                True,
                "v0.1.1",
                "0.1.1",
            ),
            (
                "zero-major-minor-from-feat",
                "v0.1.2",
                ["fix: small tweak", "feat(cli): add export profile"],
                True,
                "v0.2.0",
                "0.2.0",
            ),
            (
                "zero-major-breaking-maps-to-minor-bang",
                "v0.2.3",
                ["feat!: redesign scene schema"],
                True,
                "v0.3.0",
                "0.3.0",
            ),
            (
                "zero-major-breaking-maps-to-minor-footer",
                "v0.2.3",
                ["refactor: clean args\n\nBREAKING CHANGE: drop legacy flag"],
                True,
                "v0.3.0",
                "0.3.0",
            ),
            (
                "one-plus-major-from-bang",
                "v1.4.2",
                ["fix!: remove deprecated endpoint"],
                True,
                "v2.0.0",
                "2.0.0",
            ),
            (
                "one-plus-major-from-hyphen-breaking-footer",
                "v1.4.2",
                ["chore: drop compat\n\nBREAKING-CHANGE: minimum rust 1.88"],
                True,
                "v2.0.0",
                "2.0.0",
            ),
            (
                "one-plus-minor-from-feat",
                "v1.4.2",
                ["docs: update guide", "feat(ui): add zoom reset"],
                True,
                "v1.5.0",
                "1.5.0",
            ),
            (
                "one-plus-patch-from-fix",
                "v1.4.2",
                ["docs: update guide", "fix(server): bind loopback only"],
                True,
                "v1.4.3",
                "1.4.3",
            ),
            (
                "non-releasing-commits-skip-release",
                "v1.4.2",
                [
                    "docs: add examples",
                    "chore: tidy makefile",
                    "ci: cache cargo",
                    "refactor: extract helper",
                    "style: run prettier",
                    "test: add parameterized cases",
                ],
                False,
                "",
                "",
            ),
        ]

        for case_id, base_tag, messages, exp_should, exp_tag, exp_ver in cases:
            with self.subTest(case=case_id):
                with tempfile.TemporaryDirectory() as tmp:
                    repo = _init_repo(pathlib.Path(tmp))
                    _commit(repo, "chore: initial commit")
                    _git(repo, "tag", "-a", base_tag, "-m", f"Release {base_tag}")
                    for idx, msg in enumerate(messages):
                        _commit(repo, msg, filename=f"file_{idx}.txt")

                    plan = planner.compute_next_release(repo)
                    self.assertEqual(plan.should_release, exp_should)
                    self.assertEqual(plan.tag, exp_tag)
                    self.assertEqual(plan.version, exp_ver)
                    self.assertEqual(plan.previous_tag, base_tag)

    def test_already_tagged_head_behavior(self) -> None:
        cases = [
            ("head-tagged-no-override-skips", None, False, None),
            ("head-tagged-with-override-errors", "0.2.0", None, "already points to release tag"),
        ]
        for case_id, override, exp_should, exp_err in cases:
            with self.subTest(case=case_id):
                with tempfile.TemporaryDirectory() as tmp:
                    repo = _init_repo(pathlib.Path(tmp))
                    _commit(repo, "feat: initial feature")
                    _git(repo, "tag", "-a", "v0.1.0", "-m", "v0.1.0")

                    if exp_err is not None:
                        with self.assertRaises(planner.ReleasePlanError) as ctx:
                            planner.compute_next_release(repo, release_version=override)
                        self.assertIn(exp_err, str(ctx.exception))
                    else:
                        plan = planner.compute_next_release(repo, release_version=override)
                        self.assertEqual(plan.should_release, exp_should)
                        self.assertEqual(plan.tag, "")
                        self.assertEqual(plan.version, "")
                        self.assertEqual(plan.previous_tag, "v0.1.0")

    def test_manual_release_version_validation(self) -> None:
        cases = [
            ("valid-greater-than-existing", "1.0.0", True, None),
            ("valid-forces-release-over-docs-only", "0.1.1", True, None),
            ("rejects-leading-v", "v0.2.0", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-leading-zero-major", "01.0.0", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-leading-zero-minor", "1.02.0", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-leading-zero-patch", "1.0.03", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-missing-patch", "1.2", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-prerelease", "0.2.0-rc.1", False, "strictly match MAJOR.MINOR.PATCH"),
            ("rejects-equal-to-existing-tag", "0.1.0", False, "strictly greater"),
            ("rejects-lower-than-existing-tag", "0.0.9", False, "strictly greater"),
            ("rejects-lower-than-unreachable-tag", "0.9.0", False, "strictly greater"),
        ]

        for case_id, override, should_succeed, exp_err in cases:
            with self.subTest(case=case_id):
                with tempfile.TemporaryDirectory() as tmp:
                    repo = _init_repo(pathlib.Path(tmp))
                    _commit(repo, "feat: base")
                    _git(repo, "tag", "v0.1.0")
                    # Create an unreachable tag on a side branch to ensure global comparison.
                    _git(repo, "checkout", "-b", "side")
                    _commit(repo, "feat: side feature", filename="side.txt")
                    _git(repo, "tag", "v1.0.0-alpha.1")  # non-release tag ignored
                    if case_id == "rejects-lower-than-unreachable-tag":
                        _git(repo, "tag", "v1.0.0")
                    _git(repo, "checkout", "main")
                    _commit(repo, "docs: update readme")

                    if should_succeed:
                        plan = planner.compute_next_release(repo, release_version=override)
                        self.assertTrue(plan.should_release)
                        self.assertEqual(plan.tag, f"v{override}")
                        self.assertEqual(plan.version, override)
                        self.assertEqual(plan.previous_tag, "v0.1.0")
                    else:
                        with self.assertRaises(planner.ReleasePlanError) as ctx:
                            planner.compute_next_release(repo, release_version=override)
                        assert exp_err is not None
                        self.assertIn(exp_err, str(ctx.exception))

    def test_unreachable_release_tag_blocks_implicit_baseline(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = _init_repo(pathlib.Path(tmp))
            _commit(repo, "feat: root")
            _git(repo, "checkout", "-b", "other")
            _commit(repo, "feat: other branch release", filename="other.txt")
            _git(repo, "tag", "v0.5.0")
            _git(repo, "checkout", "main")
            _commit(repo, "feat: main work", filename="main.txt")

            with self.assertRaises(planner.ReleasePlanError) as ctx:
                planner.compute_next_release(repo)
            self.assertIn("another branch", str(ctx.exception))

            # Explicit override strictly greater than unreachable tag succeeds.
            plan = planner.compute_next_release(repo, release_version="0.6.0")
            self.assertTrue(plan.should_release)
            self.assertEqual(plan.tag, "v0.6.0")
            self.assertEqual(plan.previous_tag, "")

    def test_cli_writes_github_output_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            repo = _init_repo(tmp_path)
            _commit(repo, "feat: initial")
            gh_output = tmp_path / "github_output.txt"

            exit_code = planner.main(
                [
                    "--repo",
                    str(repo),
                    "--release-version",
                    "",
                    "--github-output",
                    str(gh_output),
                ]
            )
            self.assertEqual(exit_code, 0)
            self.assertEqual(
                gh_output.read_text(encoding="utf-8").splitlines(),
                [
                    "should_release=true",
                    "tag=v0.1.0",
                    "version=0.1.0",
                    "previous_tag=",
                ],
            )


if __name__ == "__main__":
    unittest.main()
