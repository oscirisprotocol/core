#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
DEFAULT_VERSION="$(python3 - "${REPO_ROOT}/Cargo.toml" <<'PY'
from pathlib import Path
import re
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
match = re.search(r"version\s*=\s*\"([^\"]+)\"", text)
if not match:
    raise SystemExit("workspace version not found in Cargo.toml")
print(match.group(1))
PY
)"

if [[ ! -f "${REPO_ROOT}/Cargo.toml" ]]; then
  echo "protocol-rs Cargo.toml not found at ${REPO_ROOT}" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
  echo "sha256sum or shasum is required" >&2
  exit 1
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

create_deterministic_tarball() {
  python3 - "$1" "$2" "$3" "$4" <<'PY'
import gzip
import io
import tarfile
from pathlib import Path
import sys

binary_path = Path(sys.argv[1])
archive_path = Path(sys.argv[2])
license_path = Path(sys.argv[3])
notice_path = Path(sys.argv[4])
members = (
    ("osciris-node", binary_path.read_bytes(), 0o755),
    ("LICENSE", license_path.read_bytes(), 0o644),
    ("NOTICE", notice_path.read_bytes(), 0o644),
)

with archive_path.open("wb") as raw_file:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw_file, mtime=0) as gz_file:
        with tarfile.open(fileobj=gz_file, mode="w") as archive:
            for name, payload, mode in members:
                info = tarfile.TarInfo(name=name)
                info.size = len(payload)
                info.mode = mode
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                archive.addfile(info, io.BytesIO(payload))
PY
}

create_deterministic_zip() {
  python3 - "$1" "$2" "$3" "$4" <<'PY'
from pathlib import Path
import sys
import zipfile

binary_path = Path(sys.argv[1])
archive_path = Path(sys.argv[2])
license_path = Path(sys.argv[3])
notice_path = Path(sys.argv[4])
members = (
    ("osciris-node.exe", binary_path.read_bytes(), 0o755),
    ("LICENSE", license_path.read_bytes(), 0o644),
    ("NOTICE", notice_path.read_bytes(), 0o644),
)

with zipfile.ZipFile(archive_path, "w") as archive:
    for name, payload, mode in members:
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = mode << 16
        archive.writestr(info, payload)
PY
}

usage() {
  cat <<'EOF'
Usage:
  bash scripts/package_beta_release.sh \
    --version 0.1.1 \
    --channel beta \
    --release-page-url https://github.com/oscirisprotocol/core/releases/tag/v0.1.1 \
    --release-notes "Beta collaboration release" \
    --base-download-url https://github.com/oscirisprotocol/core/releases/download/v0.1.1 \
    --asset macos-aarch64=/path/to/osciris-node \
    [--asset linux-x86_64=/path/to/osciris-node] \
    [--asset windows-x86_64=/path/to/osciris-node.exe] \
    [--output-dir /tmp/osciris-release] \
    [--manifest-out /tmp/osciris-release/beta-release-manifest.json]

Flags:
  --asset <platform>=<binary-path>  Add one packaged asset. Repeat for each platform.
  --output-dir <dir>                Directory for generated release archives. Default: dist/beta-release
  --manifest-out <path>             Manifest JSON output path. Default: <output-dir>/beta-release-manifest.json
  --published-at <iso8601>          Explicit manifest published timestamp. Default: current UTC.
EOF
}

version="$DEFAULT_VERSION"
channel="beta"
release_page_url=""
release_notes=""
base_download_url=""
output_dir="${REPO_ROOT}/dist/beta-release"
manifest_out=""
published_at=""
declare -a asset_args=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      version="$2"
      shift 2
      ;;
    --channel)
      channel="$2"
      shift 2
      ;;
    --release-page-url)
      release_page_url="$2"
      shift 2
      ;;
    --release-notes)
      release_notes="$2"
      shift 2
      ;;
    --base-download-url)
      base_download_url="${2%/}"
      shift 2
      ;;
    --output-dir)
      output_dir="$2"
      shift 2
      ;;
    --manifest-out)
      manifest_out="$2"
      shift 2
      ;;
    --published-at)
      published_at="$2"
      shift 2
      ;;
    --asset)
      asset_args+=("$2")
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "$release_page_url" || -z "$release_notes" || -z "$base_download_url" ]]; then
  echo "--release-page-url, --release-notes, and --base-download-url are required" >&2
  exit 1
fi

if [[ ${#asset_args[@]} -eq 0 ]]; then
  echo "at least one --asset <platform>=<binary-path> is required" >&2
  exit 1
fi

if [[ -z "$manifest_out" ]]; then
  manifest_out="${output_dir}/beta-release-manifest.json"
fi

if [[ -z "$published_at" ]]; then
  published_at="$(python3 - <<'PY'
from datetime import datetime, timezone
print(datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"))
PY
)"
fi

mkdir -p "$output_dir"

release_tmp="$(mktemp -d)"
trap 'rm -rf "$release_tmp"' EXIT

manifest_jsonl="${release_tmp}/assets.jsonl"
touch "$manifest_jsonl"

for spec in "${asset_args[@]}"; do
  platform="${spec%%=*}"
  binary_path="${spec#*=}"

  if [[ -z "$platform" || "$platform" == "$binary_path" ]]; then
    echo "invalid --asset value: ${spec}" >&2
    exit 1
  fi

  if [[ ! -f "$binary_path" ]]; then
    echo "asset binary not found: ${binary_path}" >&2
    exit 1
  fi

  if [[ "$platform" == windows-* ]]; then
    filename="osciris-node-${platform}.zip"
    binary_name="osciris-node.exe"
  else
    filename="osciris-node-${platform}.tar.gz"
    binary_name="osciris-node"
  fi
  tar_root="${release_tmp}/${platform}"
  mkdir -p "$tar_root"
  install -m 0755 "$binary_path" "${tar_root}/${binary_name}"

  archive_path="${output_dir}/${filename}"
  if [[ "$platform" == windows-* ]]; then
    create_deterministic_zip \
      "${tar_root}/${binary_name}" \
      "$archive_path" \
      "${REPO_ROOT}/LICENSE" \
      "${REPO_ROOT}/NOTICE"
  else
    create_deterministic_tarball \
      "${tar_root}/${binary_name}" \
      "$archive_path" \
      "${REPO_ROOT}/LICENSE" \
      "${REPO_ROOT}/NOTICE"
  fi
  checksum="$(sha256_file "$archive_path")"

  python3 - "$platform" "$filename" "${base_download_url}/${filename}" "$checksum" >> "$manifest_jsonl" <<'PY'
import json
import sys

print(json.dumps({
    "platform": sys.argv[1],
    "filename": sys.argv[2],
    "url": sys.argv[3],
    "sha256": sys.argv[4],
}))
PY

  echo "packaged ${platform}: ${archive_path}"
done

python3 - "$channel" "$version" "$published_at" "$release_page_url" "$release_notes" "$manifest_jsonl" "$manifest_out" <<'PY'
import json
import sys
from pathlib import Path

assets_path = Path(sys.argv[6])
assets = [json.loads(line) for line in assets_path.read_text(encoding="utf-8").splitlines() if line.strip()]
manifest = {
    "channel": sys.argv[1],
    "latest_version": sys.argv[2],
    "published_at": sys.argv[3],
    "release_page_url": sys.argv[4],
    "release_notes": sys.argv[5],
    "assets": assets,
}
Path(sys.argv[7]).write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
print(json.dumps(manifest, indent=2))
PY
