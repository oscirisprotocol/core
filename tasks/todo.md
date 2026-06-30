# Task Plan

## Cross-Platform GPU Requirements Table

### Objective

Publish a single contributor-facing table covering NVIDIA, AMD, and Apple
Silicon while separating network acceptance from published performance
evidence.

### Checklist

- [x] Review current runtime support and published benchmark evidence
- [x] Add NVIDIA, AMD, and Apple Silicon platform rows
- [x] Separate network admission, workload profiles, and benchmark status
- [x] State current GPU-job eligibility for every platform
- [x] Align README and beta collaboration guidance with capability-based admission
- [x] Verify Markdown structure and changed-file scope

### Review

- Any supported node may join and publish truthful capability; there is no GPU
  or memory minimum for network admission.
- The current 7B CUDA profile baseline remains Linux x86_64 with NVIDIA CUDA
  and at least 24 GB VRAM.
- NVIDIA evidence covers A10G 24 GB for bounded 3B inference and 7B QLoRA, and
  L40S 48 GB for bounded 7B inference.
- AMD ROCm and Apple Silicon MPS/MLX have no published OSCIRIS GPU benchmark.
  Their 24 GB values are first 7B profile baselines, not admission minimums.
- AMD and Apple Silicon providers are accepted through declared capability and
  can be targeted by compatible jobs. Current automatic matching still
  requires operator-side runtime filtering.
- Verification passed: table structure, local Markdown links, stale exclusion
  wording scan, `git diff --check`, and `cargo test --workspace --locked`
  (62 tests).

## Apache-2.0 And GPU Hardware Publication

### Objective

Publish OSCIRIS Core under Apache License 2.0 and give beta contributors one
clear, evidence-bounded hardware requirements source.

### Spec

- Replace the current MIT license with the canonical Apache License 2.0 text.
- Align Cargo metadata, README license wording, and NOTICE with Apache-2.0.
- Include `LICENSE` and `NOTICE` in crate packages and binary release archives.
- Preserve `v0.1.0` as the historical MIT boundary and make `v0.1.1` the first
  Apache-2.0 release.
- Keep OSCIRIS names, logos, hosted services, commercial terms, rewards, and
  SLAs outside the software license grant.
- Publish one canonical hardware requirements document.
- Distinguish CLI/control participation from GPU worker eligibility.
- Document that current matching enforces job type and declared VRAM, not the
  complete accelerator runtime.
- Mark 24 GB VRAM as the evidence-backed floor for the current GPU beta.
- Keep lower-VRAM participation explicitly unpromised until smaller job
  profiles are tested and published.

### Checklist

- [x] Complete Apache-2.0 migration across license and metadata surfaces
- [x] Add canonical GPU hardware requirements
- [x] Replace duplicated hardware tables with links to the canonical document
- [x] Verify license references, Markdown links, Cargo metadata, and tests

### Review

- Replaced the root MIT license with the canonical Apache License 2.0 text and
  changed all workspace crate metadata to `Apache-2.0`.
- Set workspace version `0.1.1` as the first Apache-era source/release boundary;
  the already-published `v0.1.0` artifacts remain historically MIT-licensed.
- Reduced `NOTICE` to attribution and moved marks and service boundaries to
  `TRADEMARKS.md`.
- Added matching `LICENSE` and `NOTICE` files to every crate package and made
  CI fail if those copies drift from the repository files.
- Updated deterministic macOS, Linux, and Windows release packaging to include
  the binary, `LICENSE`, and `NOTICE`; the release verifier now rejects
  archives missing either legal file.
- Added `docs/hardware_requirements.md` as the canonical requirements source.
  It separates non-GPU participation from the evidence-backed current GPU
  floor: Linux/NVIDIA CUDA with 24 GB VRAM for current beta work.
- Kept lower-VRAM participation explicitly unpromised until smaller job
  profiles are tested. The document also states that current automatic
  matching checks job type and VRAM, not the complete runtime.
- Verification passed:
  - canonical Apache text byte comparison
  - Apache metadata for all five workspace crates
  - `LICENSE` and `NOTICE` inclusion in all five crate packages
  - Bash syntax and Python parser checks
  - local Markdown link audit
  - deterministic tar and zip reproduction
  - positive three-platform archive verification
  - negative rejection of an archive missing `LICENSE` and `NOTICE`
  - `cargo test --workspace --locked`: 62 tests passed
- Git history ownership review showed one contributor identity:
  `MESHACH ISHAYA <meshach@ashinity.com>`.
- Publication completed:
  - pull request: `https://github.com/oscirisprotocol/core/pull/4`
  - merge commit: `5e5694e751a70f8549abf8c3149019f28c515f5d`
  - release: `https://github.com/oscirisprotocol/core/releases/tag/v0.1.1`
  - release workflow: `https://github.com/oscirisprotocol/core/actions/runs/28414760251`
- The release workflow passed for macOS, Linux, and Windows. Verification
  downloaded the public assets, matched every SHA-256 digest, and confirmed
  that each archive contains its binary, `LICENSE`, and `NOTICE`.
- The downloaded macOS release binary reports `osciris-node 0.1.1`.
- The public hardware requirements are available at
  `https://github.com/oscirisprotocol/core/blob/main/docs/hardware_requirements.md`.
- GitHub repositories, releases, and raw repository artifacts are the
  publication authority. Website hosting runtimes such as Railway do not gate
  publication.

## Publish Safety, Participant Warnings, And License (Superseded)

### Objective

Make the public repo safe to point developers at by documenting publish links,
beta warnings, heterogeneous hardware participation, and the effective license.

### Checklist

- [x] Confirm current crate metadata license before choosing public license language
- [x] Add root MIT `LICENSE` matching the existing workspace metadata
- [x] Add `NOTICE` for OSCIRIS marks, hosted services, commercial pilots,
  rewards, settlement terms, and SLAs
- [x] Add public beta publish links to `README.md`
- [x] Add participant warnings for beta status, data handling, hardware
  attestation limits, Windows NVIDIA smoke testing, and operator responsibility
- [x] Document heterogeneous provider tiers covering CPU, Apple Silicon/MPS/MLX,
  NVIDIA CUDA, and frontier hardware
- [x] Update beta collaboration docs with platform support, checksum
  expectations, provider hardware guidance, and participant warnings

### Review

- Historical note: this section records the license state before the
  Apache-2.0 migration. It is superseded by the plan above.
- The repo remains MIT-licensed because the workspace metadata already declared
  `license = "MIT"`. A root `LICENSE` now makes that explicit for public GitHub
  consumers.
- Public messaging should use heterogeneous compute language: CPU, Apple
  Silicon/MPS, and NVIDIA CUDA can participate, with job routing constrained by
  declared and verified capability.
- Enterprise claims remain bounded: CUDA providers are recommended for
  high-throughput enterprise AI workloads; Apple Silicon is valid for MPS/MLX
  compatible workloads and beta participation; Windows NVIDIA hosts require
  smoke testing before production-readiness claims.

## Fix Protocol CLI Security Findings

### Objective

Close the three reportable protocol CLI security findings from the focused audit.

### Spec

- Make Unix and Windows bootstrap clients fail closed when a selected manifest
  asset has no SHA-256 checksum.
- Stop using manifest-controlled filenames as local download paths; require
  strict basename-safe archive names and download to fixed temp filenames.
- Add an explicit trusted assigner public-key allowlist to auto-provider
  execution and require it for `network run-provider`.
- Keep changes minimal and compatible with existing beta release artifacts.

### Checklist

- [x] Harden Bash bootstrap checksum and filename handling
- [x] Harden PowerShell bootstrap checksum and filename handling
- [x] Add trusted assigner enforcement to auto-provider execution
- [x] Update CLI/docs/task notes for new provider flag
- [x] Run targeted tests and syntax checks

### Review

- `scripts/bootstrap_beta_collaboration.sh` now requires a selected asset to
  include a valid 64-character SHA-256 checksum, validates the manifest
  filename as a safe basename, and downloads to a fixed temp archive path.
- `scripts/bootstrap_beta_collaboration.ps1` now applies the same checksum and
  filename controls for the Windows zip path and downloads to a fixed temp zip.
- `network run-provider` now requires
  `--trusted-assigner-public-key-base64`; auto-provider execution rejects
  assignments signed by keys outside that configured trust set.
- Updated provider runbook examples to include the trusted assigner key flag.
- Verification run:
  - `cargo fmt --check`
  - `bash -n scripts/bootstrap_beta_collaboration.sh`
  - `bash -n scripts/package_beta_release.sh`
  - `bash -n scripts/run_beta_release_flow.sh`
  - synthetic Bash bootstrap missing-SHA256 manifest rejection
  - synthetic Bash bootstrap unsafe filename rejection with no escaped file
    written
  - `cargo test -p osciris-node job_matching -- --nocapture`
  - `cargo test -p osciris-node assignment_trust -- --nocapture`
  - `cargo test -p osciris-node --lib`
  - `cargo check -p osciris-cli --bin osciris-node`
  - `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com --release-manifest-only`
- PowerShell syntax validation was not run locally because `pwsh` is not
  installed on this macOS host.

## Audit Protocol CLI Security Surface

### Objective

Assess the OSCIRIS protocol CLI and directly related beta release/bootstrap
surface for exploitable security issues.

### Spec

- Scope the audit to `crates/osciris-cli`, release/update/bootstrap scripts,
  release workflow packaging, and beta docs that define expected behavior.
- Follow the Codex Security phased structure: threat model, finding discovery,
  validation, attack-path analysis, and final report.
- Prioritize update/install risks, malicious manifests, archive extraction,
  checksum verification, command execution, filesystem writes, path handling,
  credential leakage, and provider/proof trust boundaries.
- Do not edit source code during the audit unless explicitly asked for fixes.

### Checklist

- [x] Generate or reuse repository threat model
- [x] Discover candidate findings in CLI and release/bootstrap surfaces
- [x] Validate candidates against concrete attacker control and impact
- [x] Analyze attack paths and severity for surviving findings
- [x] Document final audit results and verification commands

### Review

- Completed a focused Codex Security-style scoped audit for the protocol CLI,
  directly invoked runtime crates, and beta release/bootstrap scripts.
- Exhaustive multi-worker scan preflight could not prove worker capacity from
  runtime/config, so this result is intentionally scoped rather than claimed as
  exhaustive repository-wide coverage.
- Final report:
  `/var/folders/3b/5q3fv8gd5hjd0bxnv0fyc4r40000gn/T/codex-security-scans/protocol-rs/6d093da_20260629T235105Z/report.md`
- Reportable findings:
  - P1 High: bootstrap installers trust unsigned manifest-controlled binary URLs
    and checksums.
  - P2 Medium: auto-provider executes arbitrary signed mesh jobs without an
    authorization trust root.
  - P2 Medium: manifest-controlled asset filename can write downloads outside
    bootstrap temp directories.
- Verification run:
  - `cargo test -p osciris-node job_matches_provider_capability -- --nocapture`
  - `cargo test -p osciris-cli signed_verification_receipt_import_rejects_tampering -- --nocapture`
  - local path traversal simulation for bootstrap `asset.filename`

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
- [x] Verify Windows-target cargo check on a Windows/MSVC runner

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
- GitHub Actions release matrix verification completed on branch
  `codex/public-main-beta-promotion`:
  - PR run `28387903806` passed with `Build macos-aarch64`,
    `Build linux-x86_64`, and `Build windows-x86_64` all green
  - push run `28387900403` passed with `Build macos-aarch64`,
    `Build linux-x86_64`, and `Build windows-x86_64` all green
  - release publication remained correctly skipped on PR and branch pushes,
    preserving tag/manual gating for actual GitHub Release uploads
