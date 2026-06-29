#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${OSCIRIS_BASE_URL:-https://oscirislabs.com}"
WORK_ROOT="${OSCIRIS_WORK_ROOT:-${TMPDIR:-/tmp}/osciris-client}"
INSTALL_DIR="${OSCIRIS_INSTALL_DIR:-$HOME/.local/bin}"
BIN_NAME="osciris-node"
BIN_PATH="${INSTALL_DIR}/${BIN_NAME}"

mkdir -p "$WORK_ROOT" "$INSTALL_DIR"

if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
  manifest_json="$(curl -fsSL "${BASE_URL%/}/beta-release-manifest.json")"
  temp_dir="$(mktemp -d)"
  trap 'rm -rf "$temp_dir"' EXIT

  python3 - "$manifest_json" > "$temp_dir/asset-info.txt" <<'PY'
import json
import platform
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
if selected is None and assets:
    selected = assets[0]

if selected is None:
    raise SystemExit("beta manifest does not list any downloadable assets")

print(selected["url"])
print(selected["filename"])
PY
  asset_url=""
  asset_filename=""
  while IFS= read -r line; do
    if [[ -z "$asset_url" ]]; then
      asset_url="$line"
    elif [[ -z "$asset_filename" ]]; then
      asset_filename="$line"
      break
    fi
  done < "$temp_dir/asset-info.txt"

  if [[ -z "$asset_url" || -z "$asset_filename" ]]; then
    echo "failed to resolve a downloadable asset from beta-release-manifest.json" >&2
    exit 1
  fi

  curl -fsSL "$asset_url" -o "$temp_dir/$asset_filename"
  tar -xzf "$temp_dir/$asset_filename" -C "$temp_dir"
  install -m 0755 "$temp_dir/$BIN_NAME" "$BIN_PATH"
fi

export PATH="$INSTALL_DIR:$PATH"

"$BIN_NAME" network sync-published \
  --work-root "$WORK_ROOT" \
  --base-url "$BASE_URL"

"$BIN_NAME" network check-updates \
  --work-root "$WORK_ROOT" \
  --base-url "$BASE_URL"

echo "OSCIRIS collaborator bootstrap complete."
echo "Binary: $(command -v "$BIN_NAME")"
echo "Work root: $WORK_ROOT"
