# Task Plan

## Guard Public Beta Manifest Publication

### Objective

Prevent the beta release operator flow from copying a manifest to the website
when that manifest references GitHub release assets that are not actually
reachable yet.

### Spec

- Keep `scripts/run_beta_release_flow.sh` as the main operator entrypoint.
- When `--website-public-dir` is provided, verify the generated release surface
  before copying `beta-release-manifest.json` into the website public directory.
- Fail closed by default if the generated manifest points to missing or invalid
  release assets.
- Allow an explicit operator override for the current source-fallback scenario.
- Update the maintainer checklist so the guarded behavior is clear.

### Checklist

- [x] Add guarded manifest publication to `scripts/run_beta_release_flow.sh`
- [x] Document the override path and publication sequence
- [x] Verify the guard blocks publication when release assets are unavailable
- [x] Verify the override still allows local fallback-oriented publication

### Review

- `scripts/run_beta_release_flow.sh` now verifies the generated manifest against
  its referenced asset URLs before copying `beta-release-manifest.json` into a
  website public directory.
- Default behavior is fail-closed: when the generated manifest still points to
  unreachable GitHub assets, the script exits before publishing the website
  manifest.
- The explicit `--allow-unverified-website-manifest` override preserves the
  intentional source-fallback publication path, but emits a warning.
- Verification run:
  - `bash -n scripts/run_beta_release_flow.sh`
  - `bash -n scripts/package_beta_release.sh`
  - local guarded publication attempt without override: failed as expected and
    wrote no files to `/tmp/osciris-beta-flow-test/site`
  - local guarded publication attempt with override: succeeded and copied
    `/tmp/osciris-beta-flow-test/site/beta-release-manifest.json`

## Verify Required Beta Platforms

### Objective

Make the release verifier fail when required public beta platforms are missing
from the manifest, even if the listed assets themselves are valid.

### Spec

- Add explicit required-platform checks to
  `scripts/verify_beta_release_surface.py`.
- Detect duplicate platform entries in the manifest.
- Improve bootstrap diagnostics when the current machine's platform is not
  present in the public manifest.
- Update the maintainer checklist to state the required public platform set.

### Checklist

- [x] Add required-platform validation to `scripts/verify_beta_release_surface.py`
- [x] Improve missing-platform diagnostics in `scripts/bootstrap_beta_collaboration.sh`
- [x] Update release docs for platform coverage expectations
- [x] Verify the verifier fails for a manifest that omits Linux or duplicates a platform

### Review

- The verifier now enforces `macos-aarch64` and `linux-x86_64` coverage by
  default and reports duplicate platform entries.
- `bootstrap_beta_collaboration.sh` now fails clearly when the current machine
  platform is absent from the manifest instead of silently selecting the first
  listed asset.
- Verification confirmed:
  - the live public surface fails when Linux is missing
  - synthetic duplicate-platform manifests are rejected
  - file-based verification works after handling `file://` responses without
    HTTP status codes

## Deterministic Beta Packaging

### Objective

Make release tarballs byte-stable for identical binaries so manifest checksums
do not drift across repeated packaging runs.

### Spec

- Replace metadata-sensitive tarball generation with deterministic archive
  creation.
- Keep release publication guarded, but allow a bounded wait for GitHub release
  asset propagation before website manifest publication.
- Verify repeated packaging runs produce identical tarball checksums.

### Checklist

- [x] Make `scripts/package_beta_release.sh` emit deterministic tarballs
- [x] Add bounded retry behavior to website-manifest publication checks
- [x] Verify repeated packaging runs produce identical asset hashes
- [x] Re-run release publication with deterministic assets and copy the updated manifest into the website repo

### Review

- `scripts/package_beta_release.sh` now writes tarballs through Python with
  fixed gzip `mtime`, fixed tar entry metadata, and stable file contents.
- Repeated packaging runs against the same binaries now produce identical
  tarball hashes for both `macos-aarch64` and `linux-x86_64`.
- `scripts/run_beta_release_flow.sh` now retries manifest verification for a
  bounded window before refusing website publication, which covers GitHub asset
  propagation lag after upload.
- The `v0.1.0` GitHub release now contains both public beta tarballs with the
  deterministic hashes:
  - `macos-aarch64`: `7f5fa15a315761a035340c0e5ba748c5470ee8b112a40c6dd6b5a31de7f580f2`
  - `linux-x86_64`: `984d0dc3222f4eda7339bc11f01a90e9b0b843e46a094ee648afdb29225e9f73`
- The updated `beta-release-manifest.json` has been copied into the local
  `OSCIRISLABS/public/` tree, but the live `https://oscirislabs.com` domain is
  still serving the previous manifest until the website repo update is pushed
  and deployed.

## Add Windows Beta Release Support

### Objective

Add `windows-x86_64` as a first-class beta release platform for Windows
developers and NVIDIA GPU users.

### Spec

- Package Windows as a deterministic `.zip` containing `osciris-node.exe`.
- Keep macOS and Linux packaged as deterministic `.tar.gz` archives.
- Require `windows-x86_64` in the public beta manifest contract.
- Verify both `.tar.gz` and `.zip` asset formats.
- Add a PowerShell bootstrap installer for Windows.
- Normalize CLI beta asset selection to stable manifest platform keys.
- Update beta docs and release acceptance checks for Windows.

### Checklist

- [x] Add deterministic Windows zip packaging
- [x] Extend release verifier for zip assets and required Windows platform
- [x] Add Windows PowerShell bootstrap installer
- [x] Normalize CLI beta platform key selection
- [x] Update release and collaborator docs
- [x] Verify packaging and verifier behavior
- [ ] Verify Windows-target cargo check on a Windows/MSVC runner

### Review

- `scripts/package_beta_release.sh` now emits deterministic `.zip` archives for
  `windows-*` platforms and deterministic `.tar.gz` archives for Unix
  platforms.
- `scripts/verify_beta_release_surface.py` now requires `windows-x86_64` by
  default and validates `.zip` assets containing `osciris-node.exe`.
- Added `scripts/bootstrap_beta_collaboration.ps1` for native Windows
  onboarding with manifest selection, SHA-256 verification, extraction, sync,
  and update check.
- `osciris-node network check-updates` now selects a stable beta platform key
  (`windows-x86_64`, `linux-x86_64`, `macos-aarch64`) and no longer falls back
  to the first manifest asset when the current platform is missing.
- Added `.github/workflows/release.yml` with macOS, Linux, and Windows release
  builds, then packaging through the repo's release script.
- Verification completed:
  - `bash -n scripts/package_beta_release.sh`
  - `bash -n scripts/bootstrap_beta_collaboration.sh`
  - `bash -n scripts/run_beta_release_flow.sh`
  - synthetic three-platform packaging generated macOS/Linux tarballs and a
    Windows zip
  - manifest-only verifier passed against the synthetic three-platform surface
  - repeated Windows zip packaging produced identical SHA-256
  - `cargo check -p osciris-cli --bin osciris-node` passed on the host
- Local `cargo check --target x86_64-pc-windows-msvc -p osciris-cli --bin
  osciris-node` could not complete on macOS because native C dependencies need
  Windows/MSVC SDK headers (`windows.h`, `assert.h`). The Windows build must be
  verified on `windows-latest` through GitHub Actions or a real Windows machine.
