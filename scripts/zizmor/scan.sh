#!/usr/bin/env bash
# Offline PR-base-locked Zizmor security scan for GitHub Actions workflows and configs.
set -euo pipefail

ZIZMOR_IMAGE="${ZIZMOR_IMAGE:-ghcr.io/zizmorcore/zizmor:1.32.0@sha256:3014cc8f68f663935f69b3c3e0e41f5ce102b268d97db13db0f62208747f860d}"
REPO_ROOT="${GITHUB_WORKSPACE:-$(git rev-parse --show-toplevel)}"

cd "$REPO_ROOT"

# Reject symlinks anywhere under .github/ (both working tree and git index).
if [ -d .github ]; then
  symlink_files="$(find .github -type l 2>/dev/null || true)"
  if [ -n "$symlink_files" ]; then
    echo "::error::Symlinked files under .github/ are forbidden:" >&2
    printf '%s\n' "$symlink_files" >&2
    exit 1
  fi
fi

git_symlinks="$(git ls-files -s -- .github | awk '$1 == "120000" { print $4 }' || true)"
if [ -n "$git_symlinks" ]; then
  echo "::error::Git symlink entries (mode 120000) under .github/ are forbidden:" >&2
  printf '%s\n' "$git_symlinks" >&2
  exit 1
fi

# Reject non-regular or non-YAML files in .github/workflows/.
if [ -d .github/workflows ]; then
  while IFS= read -r -d '' wf; do
    if [ ! -f "$wf" ] || [ -L "$wf" ]; then
      echo "::error::Workflow path is not a regular non-symlink file: $wf" >&2
      exit 1
    fi
    case "$wf" in
      *.yml|*.yaml) ;;
      *)
        echo "::error::Unexpected non-YAML file in .github/workflows/: $wf" >&2
        exit 1
        ;;
    esac
  done < <(find .github/workflows -mindepth 1 -maxdepth 1 -print0)
fi

# Reject dirty or untracked .github/ files in CI unless explicitly bypassed locally.
require_clean="${ZIZMOR_REQUIRE_CLEAN:-${GITHUB_ACTIONS:-false}}"
if [ "$require_clean" = "true" ] || [ "$require_clean" = "1" ]; then
  dirty_github="$(git status --porcelain -- .github)"
  if [ -n "$dirty_github" ]; then
    echo "::error::Unclean .github/ files detected before security scan:" >&2
    printf '%s\n' "$dirty_github" >&2
    exit 1
  fi
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

docker_config_args=()
zizmor_config_args=()

if [ -n "${PR_BASE_SHA:-}" ]; then
  if ! git cat-file -e "${PR_BASE_SHA}^{commit}" 2>/dev/null; then
    git fetch --no-tags --depth=1 origin "$PR_BASE_SHA"
  fi

  # Flag newly introduced zizmor ignore comments in the PR diff.
  added_ignores="$(
    git diff --no-ext-diff --unified=0 "$PR_BASE_SHA" HEAD -- .github/ .pre-commit-config.yaml .pre-commit-hooks.yaml 2>/dev/null \
      | awk '
          /^\+\+\+ b\// { file = substr($0, 7); next }
          /^\+[^+]/ {
            line = substr($0, 2)
            if (line ~ /zizmor:[[:space:]]*ignore\[/) {
              printf "- `%s`: `%s`\n", file, line
            }
          }
        ' || true
  )"

  if [ -n "$added_ignores" ]; then
    echo "::warning::Newly added zizmor ignore comment(s) detected in PR diff."
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      {
        echo "### Newly Added \`zizmor: ignore[\` Comments"
        echo ""
        echo "Review these inline suppressions carefully before merging:"
        echo ""
        printf '%s\n' "$added_ignores"
        echo ""
      } >> "$GITHUB_STEP_SUMMARY"
    fi
  fi

  # Lock Zizmor configuration strictly to the PR base commit so PRs cannot weaken rules.
  base_config=""
  for candidate in .github/zizmor.yml .github/zizmor.yaml zizmor.yml zizmor.yaml; do
    if git cat-file -e "${PR_BASE_SHA}:${candidate}" 2>/dev/null; then
      git show "${PR_BASE_SHA}:${candidate}" > "${tmpdir}/base-zizmor.yml"
      base_config="${tmpdir}/base-zizmor.yml"
      break
    fi
  done

  if [ -n "$base_config" ]; then
    docker_config_args=(-v "${base_config}:/tmp/base-zizmor.yml:ro")
    zizmor_config_args=(--config /tmp/base-zizmor.yml)
  else
    zizmor_config_args=(--no-config)
  fi
fi

docker run --rm \
  --cap-drop=ALL \
  --security-opt=no-new-privileges \
  -v "${REPO_ROOT}:/workspace:ro" \
  ${docker_config_args[@]+"${docker_config_args[@]}"} \
  -w /workspace \
  "$ZIZMOR_IMAGE" \
  --persona=auditor \
  --format=github \
  --collect=all \
  --strict-collection \
  --offline \
  ${zizmor_config_args[@]+"${zizmor_config_args[@]}"} \
  .
