#!/usr/bin/env python3
"""=== RELEASE SEMVER PLANNER ===

PURPOSE:
Computes the next deterministic SemVer release tag (`vMAJOR.MINOR.PATCH`) from
Git tag history and Conventional Commits between the latest reachable release
tag and `HEAD`, or validates an explicit `--release-version` override.

DESIGN RATIONALE ("WHY"):
Push-to-main automated releases must never silently emit duplicate tags, skip
version monotonicity across branches, or release non-functional maintenance
commits (`docs:`, `chore:`, `ci:`, `refactor:`, `style:`, `test:`). Centralizing
this logic in a standalone, tested Python script keeps `.github/workflows/release.yml`
minimal, auditable, and free of complex inline shell logic.

EXECUTION LIFECYCLE / DATA FLOW:
1. Validate optional `--release-version` strictly as `X.Y.Z` (no leading zeroes).
2. Inspect `HEAD` tags (`git tag --points-at HEAD`), reachable tags
   (`git tag --merged HEAD`), and all tags (`git tag --list`).
3. If `HEAD` already points to a reachable `vMAJOR.MINOR.PATCH` tag:
   - Fail closed if `--release-version` was requested.
   - Otherwise emit `should_release=false`.
4. If `--release-version` was supplied:
   - Verify it is strictly greater than every existing `vMAJOR.MINOR.PATCH` tag
     across the repository, then emit `should_release=true`.
5. If no `vMAJOR.MINOR.PATCH` tag is reachable from `HEAD`:
   - Fail closed if unreachable `vMAJOR.MINOR.PATCH` tags exist on other branches.
   - Otherwise emit the initial baseline release `v0.1.0`.
6. Otherwise scan `<prev_tag>..HEAD` commit messages (`%B%x00`), map the highest
   Conventional Commit bump (`major` -> `minor` while `major == 0`, `feat` ->
   `minor`, `fix`/`perf`/`*(deps)` -> `patch`), and emit `should_release=false`
   when no releasing commit is present.
"""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import subprocess
import sys
from typing import Literal

BumpKind = Literal["major", "minor", "patch"]

SEMVER_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
TAG_RE = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
HEADER_RE = re.compile(
    r"^(?P<type>[A-Za-z0-9_-]+)(?:\((?P<scope>[^()\r\n]+)\))?(?P<bang>!)?:(?:\s|$)"
)
BREAKING_FOOTER_RE = re.compile(r"^BREAKING[ -]CHANGE:\s")

BUMP_RANK: dict[BumpKind, int] = {
    "patch": 1,
    "minor": 2,
    "major": 3,
}


class ReleasePlanError(RuntimeError):
    """Raised when release tag calculation violates a safety or SemVer invariant."""


@dataclasses.dataclass(frozen=True)
class ReleasePlan:
    """Deterministic release decision and outputs for CI workflows."""

    should_release: bool
    tag: str
    version: str
    previous_tag: str

    def to_github_outputs(self) -> list[str]:
        """Render key=value lines suitable for `$GITHUB_OUTPUT`."""
        return [
            f"should_release={'true' if self.should_release else 'false'}",
            f"tag={self.tag}",
            f"version={self.version}",
            f"previous_tag={self.previous_tag}",
        ]


def parse_semver_core(raw: str) -> tuple[int, int, int]:
    """Parse strict `X.Y.Z` (no leading zeroes or prefix) into an integer tuple."""
    match = SEMVER_RE.fullmatch(raw)
    if match is None:
        raise ReleasePlanError(
            f"version '{raw}' must strictly match MAJOR.MINOR.PATCH without leading zeroes or 'v' prefix"
        )
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def parse_release_tag(tag: str) -> tuple[int, int, int] | None:
    """Parse a strict `vMAJOR.MINOR.PATCH` Git tag, returning None for non-release tags."""
    match = TAG_RE.fullmatch(tag.strip())
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def format_semver_core(version: tuple[int, int, int]) -> str:
    """Format an integer SemVer tuple as `MAJOR.MINOR.PATCH`."""
    return f"{version[0]}.{version[1]}.{version[2]}"


def classify_commit_bump(message: str) -> BumpKind | None:
    """Classify a single Git commit message (`%B`) into a SemVer bump kind.

    Precedence:
    1. Breaking (`!:` in header or `BREAKING CHANGE: ` / `BREAKING-CHANGE: ` line) -> `major`
    2. Feature (`feat:` / `feat(...):`) -> `minor`
    3. Patch (`fix:` / `perf:` / `*(deps):`) -> `patch`
    4. All other commits (`docs:`, `chore:`, `ci:`, `refactor:`, `style:`, `test:`, etc.) -> `None`
    """
    normalized = message.replace("\r\n", "\n").strip()
    if not normalized:
        return None

    lines = normalized.split("\n")
    subject = lines[0].strip()
    header_match = HEADER_RE.match(subject)

    if header_match is not None and header_match.group("bang") == "!":
        return "major"

    for line in lines[1:]:
        if BREAKING_FOOTER_RE.match(line):
            return "major"

    if header_match is None:
        return None

    commit_type = header_match.group("type").lower()
    scope = (header_match.group("scope") or "").lower()

    if commit_type == "feat":
        return "minor"
    if commit_type in ("fix", "perf") or scope == "deps":
        return "patch"
    return None


def apply_bump(previous: tuple[int, int, int], bump: BumpKind) -> tuple[int, int, int]:
    """Apply a SemVer bump, mapping `major` to `minor` while `major == 0`."""
    major, minor, patch = previous
    if bump == "major":
        if major == 0:
            return (0, minor + 1, 0)
        return (major + 1, 0, 0)
    if bump == "minor":
        return (major, minor + 1, 0)
    return (major, minor, patch + 1)


def _run_git(repo: pathlib.Path, args: list[str]) -> str:
    """Execute a Git command inside `repo`, raising `ReleasePlanError` on failure."""
    proc = subprocess.run(
        ["git", *args],
        cwd=repo,
        check=False,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        stderr = proc.stderr.strip() or proc.stdout.strip() or f"exit code {proc.returncode}"
        raise ReleasePlanError(f"git {' '.join(args)} failed: {stderr}")
    return proc.stdout


def _collect_release_tags(raw_lines: str) -> list[tuple[str, tuple[int, int, int]]]:
    """Filter raw `git tag` output lines to strict `vMAJOR.MINOR.PATCH` pairs."""
    tags: list[tuple[str, tuple[int, int, int]]] = []
    for line in raw_lines.splitlines():
        candidate = line.strip()
        if not candidate:
            continue
        parsed = parse_release_tag(candidate)
        if parsed is not None:
            tags.append((candidate, parsed))
    return tags


def compute_next_release(
    repo: pathlib.Path,
    release_version: str | None = None,
) -> ReleasePlan:
    """Compute the deterministic next release plan for `repo` at `HEAD`."""
    _run_git(repo, ["rev-parse", "--verify", "HEAD"])

    requested_version = release_version if release_version not in (None, "") else None
    requested_tuple = parse_semver_core(requested_version) if requested_version is not None else None

    all_tags = _collect_release_tags(_run_git(repo, ["tag", "--list"]))
    head_tags = _collect_release_tags(_run_git(repo, ["tag", "--points-at", "HEAD"]))
    reachable_tags = _collect_release_tags(_run_git(repo, ["tag", "--merged", "HEAD"]))

    if head_tags:
        latest_head_tag, _ = max(head_tags, key=lambda item: item[1])
        if requested_version is not None:
            raise ReleasePlanError(
                f"HEAD already points to release tag '{latest_head_tag}'; cannot override with '{requested_version}'"
            )
        return ReleasePlan(
            should_release=False,
            tag="",
            version="",
            previous_tag=latest_head_tag,
        )

    previous_tag = ""
    previous_tuple: tuple[int, int, int] | None = None
    if reachable_tags:
        previous_tag, previous_tuple = max(reachable_tags, key=lambda item: item[1])

    if requested_version is not None and requested_tuple is not None:
        for existing_tag, existing_tuple in all_tags:
            if requested_tuple <= existing_tuple:
                raise ReleasePlanError(
                    f"requested version '{requested_version}' must be strictly greater than existing tag '{existing_tag}'"
                )
        return ReleasePlan(
            should_release=True,
            tag=f"v{requested_version}",
            version=requested_version,
            previous_tag=previous_tag,
        )

    if previous_tuple is None:
        if all_tags:
            highest_existing, _ = max(all_tags, key=lambda item: item[1])
            raise ReleasePlanError(
                f"no vMAJOR.MINOR.PATCH tag is reachable from HEAD, but '{highest_existing}' exists on another branch; pass --release-version explicitly"
            )
        return ReleasePlan(
            should_release=True,
            tag="v0.1.0",
            version="0.1.0",
            previous_tag="",
        )

    raw_log = _run_git(repo, ["log", f"{previous_tag}..HEAD", "--format=%B%x00"])
    highest_bump: BumpKind | None = None
    for raw_commit in raw_log.split("\x00"):
        commit_bump = classify_commit_bump(raw_commit)
        if commit_bump is None:
            continue
        if highest_bump is None or BUMP_RANK[commit_bump] > BUMP_RANK[highest_bump]:
            highest_bump = commit_bump

    if highest_bump is None:
        return ReleasePlan(
            should_release=False,
            tag="",
            version="",
            previous_tag=previous_tag,
        )

    next_tuple = apply_bump(previous_tuple, highest_bump)
    next_version = format_semver_core(next_tuple)
    next_tag = f"v{next_version}"

    for existing_tag, _ in all_tags:
        if existing_tag == next_tag:
            raise ReleasePlanError(
                f"computed tag '{next_tag}' already exists on another branch; pass --release-version explicitly"
            )

    return ReleasePlan(
        should_release=True,
        tag=next_tag,
        version=next_version,
        previous_tag=previous_tag,
    )


def main(argv: list[str] | None = None) -> int:
    """CLI entrypoint for `compute_next_release_tag.py`."""
    parser = argparse.ArgumentParser(
        description="Compute the next deterministic SemVer release tag from Git history."
    )
    parser.add_argument(
        "--repo",
        type=pathlib.Path,
        default=pathlib.Path("."),
        help="Path to the Git repository (default: current working directory).",
    )
    parser.add_argument(
        "--release-version",
        default=None,
        help="Optional explicit SemVer version (X.Y.Z) overriding commit-based bump calculation.",
    )
    parser.add_argument(
        "--github-output",
        type=pathlib.Path,
        default=None,
        help="Optional path to append GitHub Actions step outputs (e.g. $GITHUB_OUTPUT).",
    )
    args = parser.parse_args(argv)

    try:
        plan = compute_next_release(args.repo, release_version=args.release_version)
    except ReleasePlanError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    output_lines = plan.to_github_outputs()
    for line in output_lines:
        print(line)

    if args.github_output is not None:
        with args.github_output.open("a", encoding="utf-8") as handle:
            for line in output_lines:
                handle.write(f"{line}\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
