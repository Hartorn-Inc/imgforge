#!/usr/bin/env bash
# === IMGFORGE UNIX INSTALLER ===
#
# PURPOSE:
# Downloads, SHA-256 verifies, and atomically installs a release binary of
# `imgforge` for macOS or Linux into `$IMGFORGE_INSTALL_DIR` (default `~/.local/bin`).
#
# DESIGN RATIONALE ("WHY"):
# Avoids piping unverified archives directly to extraction or writing partially
# unpacked binaries into `$PATH`. All downloads are staged in a private temp
# directory, verified against `SHA256SUMS.txt`, and moved atomically.

set -euo pipefail

IMGFORGE_REPO="${IMGFORGE_REPO:-Hartorn-Inc/imgforge}"
IMGFORGE_VERSION="${IMGFORGE_VERSION:-latest}"
IMGFORGE_INSTALL_DIR="${IMGFORGE_INSTALL_DIR:-${HOME}/.local/bin}"
IMGFORGE_INSTALL_DIR="${IMGFORGE_INSTALL_DIR%/}"
if [ -z "$IMGFORGE_INSTALL_DIR" ]; then
  IMGFORGE_INSTALL_DIR="/"
fi

for cmd in curl tar awk; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "error: required command '$cmd' was not found in PATH" >&2
    exit 1
  fi
done

if command -v sha256sum >/dev/null 2>&1; then
  SHA256_CMD="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  SHA256_CMD="shasum"
else
  echo "error: neither 'sha256sum' nor 'shasum' was found in PATH" >&2
  exit 1
fi

os_name="$(uname -s)"
arch_name="$(uname -m)"
case "${os_name}:${arch_name}" in
  Linux:x86_64 | Linux:amd64)
    TARGET="x86_64-unknown-linux-gnu"
    ;;
  Linux:aarch64 | Linux:arm64)
    TARGET="aarch64-unknown-linux-gnu"
    ;;
  Darwin:arm64 | Darwin:aarch64)
    TARGET="aarch64-apple-darwin"
    ;;
  Darwin:x86_64)
    TARGET="x86_64-apple-darwin"
    ;;
  *)
    echo "error: unsupported platform '${os_name}/${arch_name}'" >&2
    exit 1
    ;;
esac

EXPLICIT_TAG=""
if [ -z "$IMGFORGE_VERSION" ] || [ "$IMGFORGE_VERSION" = "latest" ]; then
  BASE_URL="https://github.com/${IMGFORGE_REPO}/releases/latest/download"
else
  raw_ver="$IMGFORGE_VERSION"
  case "$raw_ver" in
    v*) raw_ver="${raw_ver#v}" ;;
    V*) raw_ver="${raw_ver#V}" ;;
  esac
  if ! printf '%s' "$raw_ver" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'; then
    echo "error: IMGFORGE_VERSION '${IMGFORGE_VERSION}' must be 'latest' or valid SemVer (for example '0.1.0' or 'v0.1.0')" >&2
    exit 1
  fi
  EXPLICIT_TAG="v${raw_ver}"
  BASE_URL="https://github.com/${IMGFORGE_REPO}/releases/download/${EXPLICIT_TAG}"
fi

TMP_DIR="$(mktemp -d)"
STAGED_BIN=""
cleanup() {
  if [ -n "$STAGED_BIN" ] && [ -e "$STAGED_BIN" ]; then
    rm -f "$STAGED_BIN"
  fi
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

SUMS_FILE="${TMP_DIR}/SHA256SUMS.txt"
echo "Fetching release checksums from ${BASE_URL}/SHA256SUMS.txt..."
curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "$SUMS_FILE"

if [ -n "$EXPLICIT_TAG" ]; then
  EXPECTED_ARCHIVE="imgforge-${EXPLICIT_TAG}-${TARGET}.tar.gz"
  match_line="$(awk -v wanted="$EXPECTED_ARCHIVE" '
    {
      hash = tolower($1)
      file = $2
      sub(/^\*/, "", file)
      if (length(hash) == 64 && hash ~ /^[0-9a-f]+$/ && file == wanted) {
        count++
        matched_hash = hash
        matched_file = file
      }
    }
    END {
      if (count == 1) {
        print matched_hash " " matched_file
      } else {
        exit 1
      }
    }
  ' "$SUMS_FILE")" || {
    echo "error: checksum entry for '${EXPECTED_ARCHIVE}' was not found in SHA256SUMS.txt" >&2
    exit 1
  }
else
  match_line="$(awk -v target="$TARGET" '
    {
      hash = tolower($1)
      file = $2
      sub(/^\*/, "", file)
      if (length(hash) == 64 && hash ~ /^[0-9a-f]+$/ && file ~ ("^imgforge-v[0-9]+\\.[0-9]+\\.[0-9]+([+-][0-9A-Za-z.-]+)?-" target "\\.tar\\.gz$")) {
        count++
        matched_hash = hash
        matched_file = file
      }
    }
    END {
      if (count == 1) {
        print matched_hash " " matched_file
      } else {
        exit 1
      }
    }
  ' "$SUMS_FILE")" || {
    echo "error: could not resolve a unique archive for target '${TARGET}' in SHA256SUMS.txt" >&2
    exit 1
  }
fi

EXPECTED_SHA256="${match_line%% *}"
ARCHIVE_NAME="${match_line#* }"
ARCHIVE_PATH="${TMP_DIR}/${ARCHIVE_NAME}"

echo "Downloading ${BASE_URL}/${ARCHIVE_NAME}..."
curl -fsSL "${BASE_URL}/${ARCHIVE_NAME}" -o "$ARCHIVE_PATH"

if [ "$SHA256_CMD" = "sha256sum" ]; then
  ACTUAL_SHA256="$(sha256sum "$ARCHIVE_PATH" | awk '{print tolower($1)}')"
else
  ACTUAL_SHA256="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print tolower($1)}')"
fi

if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
  echo "error: SHA-256 checksum verification failed for '${ARCHIVE_NAME}'" >&2
  echo "  expected: ${EXPECTED_SHA256}" >&2
  echo "  actual:   ${ACTUAL_SHA256}" >&2
  exit 1
fi

EXTRACT_DIR="${TMP_DIR}/extract"
mkdir -p "$EXTRACT_DIR"
tar -xzf "$ARCHIVE_PATH" -C "$EXTRACT_DIR"

EXTRACTED_BIN="${EXTRACT_DIR}/imgforge"
if [ ! -f "$EXTRACTED_BIN" ] || [ -L "$EXTRACTED_BIN" ]; then
  echo "error: extracted archive did not contain a regular 'imgforge' binary at archive root" >&2
  exit 1
fi

mkdir -p "$IMGFORGE_INSTALL_DIR"
STAGED_BIN="$(mktemp "${IMGFORGE_INSTALL_DIR}/.imgforge.XXXXXX")"
cp "$EXTRACTED_BIN" "$STAGED_BIN"
chmod 0755 "$STAGED_BIN"
mv -f "$STAGED_BIN" "${IMGFORGE_INSTALL_DIR}/imgforge"
STAGED_BIN=""

INSTALLED_BIN="${IMGFORGE_INSTALL_DIR}/imgforge"
echo "Installed $("${INSTALLED_BIN}" --version) to ${INSTALLED_BIN}"

case ":${PATH}:" in
  *":${IMGFORGE_INSTALL_DIR}:"*)
    ;;
  *)
    echo ""
    echo "Note: '${IMGFORGE_INSTALL_DIR}' is not in your PATH."
    echo "Add it by appending the following line to your shell profile (~/.bashrc, ~/.zshrc, or ~/.profile):"
    echo "  export PATH=\"${IMGFORGE_INSTALL_DIR}:\$PATH\""
    ;;
esac
