#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

if [[ ! -f "${REPO_ROOT}/Cargo.toml" ]]; then
  echo "protocol-rs Cargo.toml not found at ${REPO_ROOT}" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

usage() {
  cat <<'EOF'
Usage:
  bash scripts/run_beta_release_flow.sh \
    --version 0.1.0 \
    --release-notes "Beta collaboration release" \
    --asset macos-aarch64=/path/to/osciris-node \
    --asset linux-x86_64=/path/to/osciris-node \
    --asset windows-x86_64=/path/to/osciris-node.exe \
    [--channel beta] \
    [--release-repo oscirisprotocol/core] \
    [--target-commit <git-sha>] \
    [--output-dir dist/beta-release] \
    [--website-public-dir /absolute/path/to/OSCIRISLABS/public] \
    [--verify-base-url https://oscirislabs.com] \
    [--allow-unverified-website-manifest] \
    [--publish-release]

Flags:
  --publish-release             Create or update the GitHub prerelease after packaging.
  --website-public-dir <dir>    Copy beta-release-manifest.json into a website public dir.
  --allow-unverified-website-manifest
                                Allow website manifest publication even when the
                                generated release assets are not yet reachable.
  --verify-base-url <url>       Run verify_beta_release_surface.py against the given base URL.
EOF
}

version=""
channel="beta"
release_notes=""
release_repo="oscirisprotocol/core"
target_commit=""
output_dir="${REPO_ROOT}/dist/beta-release"
website_public_dir=""
verify_base_url=""
publish_release=false
allow_unverified_website_manifest=false
website_manifest_verify_attempts=6
website_manifest_verify_sleep_seconds=5
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
    --release-notes)
      release_notes="$2"
      shift 2
      ;;
    --release-repo)
      release_repo="$2"
      shift 2
      ;;
    --target-commit)
      target_commit="$2"
      shift 2
      ;;
    --output-dir)
      output_dir="$2"
      shift 2
      ;;
    --website-public-dir)
      website_public_dir="$2"
      shift 2
      ;;
    --verify-base-url)
      verify_base_url="$2"
      shift 2
      ;;
    --publish-release)
      publish_release=true
      shift
      ;;
    --allow-unverified-website-manifest)
      allow_unverified_website_manifest=true
      shift
      ;;
    --website-manifest-verify-attempts)
      website_manifest_verify_attempts="$2"
      shift 2
      ;;
    --website-manifest-verify-sleep-seconds)
      website_manifest_verify_sleep_seconds="$2"
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

if [[ -z "$version" || -z "$release_notes" ]]; then
  echo "--version and --release-notes are required" >&2
  exit 1
fi

if [[ ${#asset_args[@]} -eq 0 ]]; then
  echo "at least one --asset <platform>=<binary-path> is required" >&2
  exit 1
fi

if [[ -z "$target_commit" ]]; then
  target_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
fi

release_tag="v${version}"
release_page_url="https://github.com/${release_repo}/releases/tag/${release_tag}"
download_base_url="https://github.com/${release_repo}/releases/download/${release_tag}"
manifest_path="${output_dir%/}/beta-release-manifest.json"

mkdir -p "$output_dir"

package_cmd=(
  bash "${REPO_ROOT}/scripts/package_beta_release.sh"
  --version "$version"
  --channel "$channel"
  --release-page-url "$release_page_url"
  --release-notes "$release_notes"
  --base-download-url "$download_base_url"
  --output-dir "$output_dir"
)

for spec in "${asset_args[@]}"; do
  package_cmd+=(--asset "$spec")
done

echo "Packaging beta release artifacts..."
"${package_cmd[@]}"

declare -a archive_paths=()
while IFS= read -r line; do
  archive_paths+=("$line")
done < <(python3 - "$manifest_path" <<'PY'
import json
import sys
from pathlib import Path

manifest = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
for asset in manifest.get("assets", []):
    print(Path(sys.argv[1]).parent / asset["filename"])
PY
)

if [[ "$publish_release" == true ]]; then
  if ! command -v gh >/dev/null 2>&1; then
    echo "gh is required for --publish-release" >&2
    exit 1
  fi

  echo "Publishing GitHub release ${release_tag} to ${release_repo}..."
  if gh release view "$release_tag" -R "$release_repo" >/dev/null 2>&1; then
    gh release upload "$release_tag" "${archive_paths[@]}" -R "$release_repo" --clobber
  else
    gh release create \
      "$release_tag" \
      "${archive_paths[@]}" \
      -R "$release_repo" \
      --target "$target_commit" \
      --title "$release_tag" \
      --notes "$release_notes" \
      --prerelease
  fi
fi

if [[ -n "$website_public_dir" ]]; then
  generated_manifest_ok=true
  generated_verify_output="$(mktemp)"
  generated_manifest_ok=false
  for ((attempt = 1; attempt <= website_manifest_verify_attempts; attempt++)); do
    if python3 "${REPO_ROOT}/scripts/verify_beta_release_surface.py" \
      --base-url "file://${output_dir}" \
      --release-manifest-only \
      --output "$generated_verify_output" >/dev/null; then
      generated_manifest_ok=true
      break
    fi

    if (( attempt < website_manifest_verify_attempts )); then
      sleep "$website_manifest_verify_sleep_seconds"
    fi
  done

  if [[ "$generated_manifest_ok" != true && "$allow_unverified_website_manifest" != true ]]; then
    echo "Refusing to publish beta-release-manifest.json to ${website_public_dir}." >&2
    echo "The generated manifest references release assets that did not pass verification." >&2
    echo "Publish the GitHub release assets first, or rerun with --allow-unverified-website-manifest for the source-fallback path." >&2
    echo "Generated verification summary: ${generated_verify_output}" >&2
    exit 1
  fi

  mkdir -p "$website_public_dir"
  cp "$manifest_path" "${website_public_dir%/}/beta-release-manifest.json"
  echo "Copied manifest to ${website_public_dir%/}/beta-release-manifest.json"
  if [[ "$generated_manifest_ok" != true ]]; then
    echo "Warning: published website manifest without verified release assets because --allow-unverified-website-manifest was set." >&2
    echo "Generated verification summary: ${generated_verify_output}" >&2
  else
    rm -f "$generated_verify_output"
  fi
fi

if [[ -n "$verify_base_url" ]]; then
  echo "Verifying public release surface at ${verify_base_url}..."
  python3 "${REPO_ROOT}/scripts/verify_beta_release_surface.py" \
    --base-url "$verify_base_url"
fi

echo
echo "OSCIRIS beta release flow complete."
echo "Manifest: ${manifest_path}"
for archive in "${archive_paths[@]}"; do
  echo "Asset: ${archive}"
done
echo "Release tag: ${release_tag}"
echo "Release page: ${release_page_url}"
