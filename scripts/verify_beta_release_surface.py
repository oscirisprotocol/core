#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import io
import json
import ssl
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any


REQUIRED_ENDPOINTS = [
    ("participant-status-summary.json", ("job_id", "participant_summary", "stages")),
    ("proof-feed.json", ("source", "anchors", "receipts")),
    ("contributor-manifest.json", ("install", "workflow", "public_proofs")),
    (
        "beta-release-manifest.json",
        ("channel", "latest_version", "published_at", "release_page_url", "release_notes", "assets"),
    ),
]

DEFAULT_REQUIRED_PLATFORMS = [
    "macos-aarch64",
    "linux-x86_64",
    "windows-x86_64",
]


@dataclass
class EndpointCheck:
    path: str
    ok: bool
    status_code: int | None = None
    bytes: int | None = None
    missing_keys: list[str] = field(default_factory=list)
    error: str | None = None


@dataclass
class AssetCheck:
    platform: str
    filename: str
    url: str
    ok: bool
    status_code: int | None = None
    bytes: int | None = None
    sha256_expected: str | None = None
    sha256_actual: str | None = None
    archive_format: str | None = None
    tar_members: list[str] = field(default_factory=list)
    error: str | None = None


@dataclass
class PlatformCoverageCheck:
    required: list[str]
    present: list[str]
    missing: list[str] = field(default_factory=list)
    duplicates: list[str] = field(default_factory=list)
    ok: bool = True


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Verify the public OSCIRIS beta release surface."
    )
    parser.add_argument(
        "--base-url",
        required=True,
        help="Base URL hosting participant-status-summary.json, proof-feed.json, contributor-manifest.json, and beta-release-manifest.json.",
    )
    parser.add_argument(
        "--allow-missing-sha256",
        action="store_true",
        help="Treat missing asset sha256 values as warnings instead of failures.",
    )
    parser.add_argument(
        "--output",
        help="Optional path for the JSON summary. Defaults to stdout only.",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=float,
        default=30.0,
        help="HTTP timeout for each request. Default: 30.",
    )
    parser.add_argument(
        "--require-platform",
        action="append",
        dest="required_platforms",
        help=(
            "Required asset platform key. Repeat for multiple values. "
            "Defaults to macos-aarch64, linux-x86_64, and windows-x86_64."
        ),
    )
    parser.add_argument(
        "--release-manifest-only",
        action="store_true",
        help=(
            "Verify only beta-release-manifest.json, its release page, and asset set. "
            "Skip the other public website JSON endpoints."
        ),
    )
    return parser.parse_args()


def fetch_bytes(url: str, timeout_seconds: float) -> tuple[int, bytes]:
    request = urllib.request.Request(url, headers={"User-Agent": "osciris-release-surface-verifier/0.1"})
    context = None
    if urllib.parse.urlparse(url).scheme == "https":
        try:
            import certifi  # type: ignore

            context = ssl.create_default_context(cafile=certifi.where())
        except Exception:  # noqa: BLE001
            context = ssl.create_default_context()

    with urllib.request.urlopen(request, timeout=timeout_seconds, context=context) as response:
        payload = response.read()
        status = getattr(response, "status", None)
        if status is None:
            getcode = getattr(response, "getcode", None)
            if callable(getcode):
                status = getcode()
        if status is None:
            status = 200
        return int(status), payload


def ensure_mapping(value: Any, path: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{path} did not decode to a JSON object")
    return value


def verify_json_endpoint(base_url: str, path: str, required_keys: tuple[str, ...], timeout_seconds: float) -> tuple[EndpointCheck, dict[str, Any] | None]:
    url = f"{base_url.rstrip('/')}/{path}"
    try:
        status_code, payload = fetch_bytes(url, timeout_seconds)
        parsed = ensure_mapping(json.loads(payload), path)
        missing_keys = [key for key in required_keys if key not in parsed]
        return (
            EndpointCheck(
                path=path,
                ok=not missing_keys,
                status_code=status_code,
                bytes=len(payload),
                missing_keys=missing_keys,
            ),
            parsed,
        )
    except urllib.error.HTTPError as exc:
        return (
            EndpointCheck(
                path=path,
                ok=False,
                status_code=exc.code,
                error=f"HTTP {exc.code}",
            ),
            None,
        )
    except Exception as exc:  # noqa: BLE001
        return (
            EndpointCheck(
                path=path,
                ok=False,
                error=str(exc),
            ),
            None,
        )


def verify_release_page(url: str, timeout_seconds: float) -> dict[str, Any]:
    try:
        status_code, payload = fetch_bytes(url, timeout_seconds)
        return {
            "url": url,
            "ok": True,
            "status_code": status_code,
            "bytes": len(payload),
        }
    except urllib.error.HTTPError as exc:
        return {
            "url": url,
            "ok": False,
            "status_code": exc.code,
            "error": f"HTTP {exc.code}",
        }
    except Exception as exc:  # noqa: BLE001
        return {
            "url": url,
            "ok": False,
            "error": str(exc),
        }


def verify_asset(asset: dict[str, Any], timeout_seconds: float, allow_missing_sha256: bool) -> AssetCheck:
    platform = str(asset.get("platform", ""))
    filename = str(asset.get("filename", ""))
    url = str(asset.get("url", ""))
    sha256_expected = asset.get("sha256")

    if not platform or not filename or not url:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            error="asset is missing platform, filename, or url",
        )

    try:
        status_code, payload = fetch_bytes(url, timeout_seconds)
    except urllib.error.HTTPError as exc:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=exc.code,
            sha256_expected=sha256_expected,
            error=f"HTTP {exc.code}",
        )
    except Exception as exc:  # noqa: BLE001
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            sha256_expected=sha256_expected,
            error=str(exc),
        )

    sha256_actual = hashlib.sha256(payload).hexdigest()
    basename = Path(urllib.parse.urlparse(url).path).name
    if basename != filename:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=status_code,
            bytes=len(payload),
            sha256_expected=sha256_expected,
            sha256_actual=sha256_actual,
            error=f"url filename mismatch: {basename} != {filename}",
        )

    if not sha256_expected and not allow_missing_sha256:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=status_code,
            bytes=len(payload),
            sha256_actual=sha256_actual,
            error="asset sha256 is missing from manifest",
        )

    if sha256_expected and sha256_actual != sha256_expected:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=status_code,
            bytes=len(payload),
            sha256_expected=sha256_expected,
            sha256_actual=sha256_actual,
            error="asset sha256 mismatch",
        )

    if filename.endswith(".tar.gz"):
        archive_format = "tar.gz"
        expected_member = "osciris-node"
        try:
            with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
                members = [member.name for member in archive.getmembers() if member.isfile()]
        except tarfile.TarError as exc:
            return AssetCheck(
                platform=platform,
                filename=filename,
                url=url,
                ok=False,
                status_code=status_code,
                bytes=len(payload),
                sha256_expected=sha256_expected,
                sha256_actual=sha256_actual,
                archive_format=archive_format,
                error=f"invalid tar.gz asset: {exc}",
            )
    elif filename.endswith(".zip"):
        archive_format = "zip"
        expected_member = "osciris-node.exe"
        try:
            with zipfile.ZipFile(io.BytesIO(payload)) as archive:
                members = [info.filename for info in archive.infolist() if not info.is_dir()]
        except zipfile.BadZipFile as exc:
            return AssetCheck(
                platform=platform,
                filename=filename,
                url=url,
                ok=False,
                status_code=status_code,
                bytes=len(payload),
                sha256_expected=sha256_expected,
                sha256_actual=sha256_actual,
                archive_format=archive_format,
                error=f"invalid zip asset: {exc}",
            )
    else:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=status_code,
            bytes=len(payload),
            sha256_expected=sha256_expected,
            sha256_actual=sha256_actual,
            error="unsupported asset archive format",
        )

    required_members = {expected_member, "LICENSE", "NOTICE"}
    missing_members = sorted(required_members.difference(members))
    if missing_members:
        return AssetCheck(
            platform=platform,
            filename=filename,
            url=url,
            ok=False,
            status_code=status_code,
            bytes=len(payload),
            sha256_expected=sha256_expected,
            sha256_actual=sha256_actual,
            archive_format=archive_format,
            tar_members=members,
            error=f"archive is missing required members: {', '.join(missing_members)}",
        )

    return AssetCheck(
        platform=platform,
        filename=filename,
        url=url,
        ok=True,
        status_code=status_code,
        bytes=len(payload),
        sha256_expected=sha256_expected,
        sha256_actual=sha256_actual,
        archive_format=archive_format,
        tar_members=members,
    )


def verify_platform_coverage(
    manifest: dict[str, Any],
    required_platforms: list[str],
) -> PlatformCoverageCheck:
    assets = manifest.get("assets", [])
    present: list[str] = []
    duplicates: list[str] = []
    seen: set[str] = set()

    if isinstance(assets, list):
        for asset in assets:
            if isinstance(asset, dict):
                platform = asset.get("platform")
                if isinstance(platform, str) and platform:
                    present.append(platform)
                    if platform in seen and platform not in duplicates:
                        duplicates.append(platform)
                    seen.add(platform)

    missing = [platform for platform in required_platforms if platform not in seen]
    return PlatformCoverageCheck(
        required=required_platforms,
        present=present,
        missing=missing,
        duplicates=duplicates,
        ok=not missing and not duplicates,
    )


def build_summary(args: argparse.Namespace) -> tuple[dict[str, Any], bool]:
    endpoint_results: list[EndpointCheck] = []
    endpoint_payloads: dict[str, dict[str, Any]] = {}
    required_platforms = args.required_platforms or list(DEFAULT_REQUIRED_PLATFORMS)
    endpoint_specs = (
        [item for item in REQUIRED_ENDPOINTS if item[0] == "beta-release-manifest.json"]
        if args.release_manifest_only
        else REQUIRED_ENDPOINTS
    )

    for path, required_keys in endpoint_specs:
        result, payload = verify_json_endpoint(
            args.base_url,
            path,
            required_keys,
            args.timeout_seconds,
        )
        endpoint_results.append(result)
        if payload is not None:
            endpoint_payloads[path] = payload

    manifest = endpoint_payloads.get("beta-release-manifest.json")
    release_page = (
        verify_release_page(str(manifest["release_page_url"]), args.timeout_seconds)
        if manifest is not None
        else {"ok": False, "error": "beta-release-manifest.json unavailable"}
    )

    asset_results: list[AssetCheck] = []
    platform_coverage = PlatformCoverageCheck(
        required=required_platforms,
        present=[],
        missing=list(required_platforms),
        duplicates=[],
        ok=False,
    )
    if manifest is not None:
        platform_coverage = verify_platform_coverage(manifest, required_platforms)
        assets = manifest.get("assets")
        if isinstance(assets, list):
            for asset in assets:
                if isinstance(asset, dict):
                    asset_results.append(
                        verify_asset(asset, args.timeout_seconds, args.allow_missing_sha256)
                    )
                else:
                    asset_results.append(
                        AssetCheck(
                            platform="",
                            filename="",
                            url="",
                            ok=False,
                            error="asset entry is not a JSON object",
                        )
                    )

    success = all(result.ok for result in endpoint_results)
    success = success and bool(asset_results) and all(result.ok for result in asset_results)
    success = success and bool(release_page.get("ok"))
    success = success and platform_coverage.ok

    summary = {
        "ok": success,
        "base_url": args.base_url.rstrip("/"),
        "allow_missing_sha256": args.allow_missing_sha256,
        "required_platforms": required_platforms,
        "release_manifest_only": args.release_manifest_only,
        "endpoints": [asdict(result) for result in endpoint_results],
        "release_page": release_page,
        "platform_coverage": asdict(platform_coverage),
        "assets": [asdict(result) for result in asset_results],
    }
    return summary, success


def main() -> int:
    args = parse_args()
    summary, success = build_summary(args)
    rendered = json.dumps(summary, indent=2) + "\n"
    if args.output:
        Path(args.output).write_text(rendered, encoding="utf-8")
    sys.stdout.write(rendered)
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
