#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${OSCIRIS_BASE_URL:-https://oscirislabs.com}"
WORK_ROOT="${OSCIRIS_WORK_ROOT:-${TMPDIR:-/tmp}/osciris-client}"
INSTALL_DIR="${OSCIRIS_INSTALL_DIR:-$HOME/.local/bin}"
BIN_NAME="osciris-node"
BIN_PATH="${INSTALL_DIR}/${BIN_NAME}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="${OSCIRIS_REPO_ROOT:-$DEFAULT_REPO_ROOT}"

mkdir -p "$WORK_ROOT" "$INSTALL_DIR"

case "$(uname -s 2>/dev/null || true)" in
  MINGW*|MSYS*|CYGWIN*)
    echo "Use scripts/bootstrap_beta_collaboration.ps1 for native Windows installs." >&2
    exit 1
    ;;
esac

has_local_checkout() {
  [[ -f "${REPO_ROOT}/Cargo.toml" && -f "${REPO_ROOT}/crates/osciris-cli/Cargo.toml" ]]
}

install_from_local_checkout() {
  local build_target_dir

if ! command -v cargo >/dev/null 2>&1; then
  return 1
fi

  if ! has_local_checkout; then
    return 1
  fi

  echo "OSCIRIS beta release asset unavailable. Building ${BIN_NAME} from local checkout at ${REPO_ROOT}."
  build_target_dir="$(mktemp -d)"
  if ! CARGO_TARGET_DIR="$build_target_dir" cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml" -p osciris-cli --bin "${BIN_NAME}"; then
    rm -rf "$build_target_dir"
    return 1
  fi

  install -m 0755 "${build_target_dir}/release/${BIN_NAME}" "${BIN_PATH}"
  rm -rf "$build_target_dir"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 1
  fi
}

download_release_asset() {
  local manifest_json temp_dir asset_url asset_filename asset_sha256 archive_path
  manifest_json="$(curl -fsSL "${BASE_URL%/}/beta-release-manifest.json")"
  temp_dir="$(mktemp -d)"
  asset_url=""
  asset_filename=""
  asset_sha256=""

  if ! python3 - "$manifest_json" > "$temp_dir/asset-info.txt" <<'PY'
import json
import os
import platform
import re
import sys

manifest = json.loads(sys.argv[1])
system = platform.system().lower()
machine = platform.machine().lower()

if system == "darwin":
    platform_key = "macos-aarch64" if machine in {"arm64", "aarch64"} else "macos-x86_64"
elif system == "linux":
    platform_key = "linux-x86_64" if machine in {"x86_64", "amd64"} else f"linux-{machine}"
else:
    platform_key = f"{system}-{machine}"

assets = manifest.get("assets", [])
selected = next((asset for asset in assets if asset.get("platform") == platform_key), None)

if selected is None:
    available = sorted(
        asset.get("platform")
        for asset in assets
        if isinstance(asset, dict) and isinstance(asset.get("platform"), str) and asset.get("platform")
    )
    if available:
        raise SystemExit(
            "beta manifest does not list a downloadable asset for "
            f"{platform_key}; available platforms: {', '.join(available)}"
        )
    raise SystemExit("beta manifest does not list any downloadable assets")

filename = selected.get("filename")
if not isinstance(filename, str) or not filename:
    raise SystemExit("selected beta asset is missing filename")
if filename != os.path.basename(filename) or "/" in filename or "\\" in filename or ".." in filename:
    raise SystemExit(f"selected beta asset filename is not safe: {filename!r}")
if not re.fullmatch(r"osciris-node-[A-Za-z0-9_.-]+\.(tar\.gz|zip)", filename):
    raise SystemExit(f"selected beta asset filename has unexpected format: {filename!r}")
sha256 = selected.get("sha256")
if not isinstance(sha256, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", sha256):
    raise SystemExit(f"selected beta asset {filename!r} is missing a valid SHA-256 checksum")
print(selected["url"])
print(filename)
print(sha256.lower())
PY
  then
    cat "$temp_dir/asset-info.txt" >&2 || true
    rm -rf "$temp_dir"
    return 1
  fi

  while IFS= read -r line; do
    if [[ -z "$asset_url" ]]; then
      asset_url="$line"
    elif [[ -z "$asset_filename" ]]; then
      asset_filename="$line"
    elif [[ -z "$asset_sha256" ]]; then
      asset_sha256="$line"
      break
    fi
  done < "$temp_dir/asset-info.txt"

  if [[ -z "$asset_url" || -z "$asset_filename" || -z "$asset_sha256" ]]; then
    rm -rf "$temp_dir"
    echo "failed to resolve a downloadable asset from beta-release-manifest.json" >&2
    return 1
  fi

  archive_path="$temp_dir/release-asset.tar.gz"
  if ! curl -fsSL "$asset_url" -o "$archive_path"; then
    rm -rf "$temp_dir"
    return 1
  fi

  if ! checksum_actual="$(sha256_file "$archive_path")"; then
    echo "release asset checksum verification failed because no sha256 tool is available" >&2
    rm -rf "$temp_dir"
    return 1
  fi

  if [[ "$checksum_actual" != "$asset_sha256" ]]; then
    echo "release asset checksum mismatch for ${asset_filename}" >&2
    echo "expected: ${asset_sha256}" >&2
    echo "actual:   ${checksum_actual}" >&2
    rm -rf "$temp_dir"
    return 1
  fi

  if ! tar -xzf "$archive_path" -C "$temp_dir"; then
    rm -rf "$temp_dir"
    return 1
  fi

  if ! install -m 0755 "$temp_dir/$BIN_NAME" "$BIN_PATH"; then
    rm -rf "$temp_dir"
    return 1
  fi

  rm -rf "$temp_dir"
}

RUN_BIN=""

if command -v "$BIN_NAME" >/dev/null 2>&1; then
  RUN_BIN="$(command -v "$BIN_NAME")"
else
  if ! download_release_asset; then
    if ! install_from_local_checkout; then
      cat >&2 <<EOF
OSCIRIS collaborator bootstrap could not install ${BIN_NAME} from the published beta manifest.

Tried:
- ${BASE_URL%/}/beta-release-manifest.json
- repo fallback at ${REPO_ROOT}

Next actions:
1. publish the missing GitHub release asset referenced by beta-release-manifest.json, or
2. run this script from a protocol-rs checkout with Cargo installed, or
3. install manually with: cargo install --path crates/osciris-cli
EOF
      exit 1
    fi
  fi
fi

export PATH="$INSTALL_DIR:$PATH"
RUN_BIN="${RUN_BIN:-$BIN_PATH}"

"$RUN_BIN" network sync-published \
  --work-root "$WORK_ROOT" \
  --base-url "$BASE_URL"

"$RUN_BIN" network check-updates \
  --work-root "$WORK_ROOT" \
  --base-url "$BASE_URL"

echo "OSCIRIS collaborator bootstrap complete."
echo "Binary: $RUN_BIN"
echo "Work root: $WORK_ROOT"
