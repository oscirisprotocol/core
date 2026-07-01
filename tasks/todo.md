# Task Plan

## Desktop Protocol Publication Bridge

### Objective

Expose the first real backend protocol flow through the desktop daemon: a
desktop job can move beyond local funding review into a signed local protocol
announcement that provider matching can consume.

### Spec

- Keep the desktop non-custodial: no EVM private keys or seed phrases enter the
  UI or daemon.
- Generate and persist a daemon-local Ed25519 protocol identity for signing
  job announcements.
- Convert desktop training/inference jobs into `osciris-core` `JobSpec` and
  signed `JobAnnouncement` records.
- Store protocol announcements in the daemon-owned local `ProtocolStore`.
- Expose a Tauri/Desktop action that publishes a funding-review job into the
  protocol queue.
- Do not claim provider execution is desktop-driven until assignment,
  execution, and receipt ingestion are all exposed through daemon APIs.

### Checklist

- [x] Add daemon protocol-store and signing dependencies
- [x] Add daemon-local protocol identity persistence
- [x] Convert desktop jobs into signed protocol announcements
- [x] Add daemon and Tauri `publish_job` command path
- [x] Add desktop UI action for funding-review jobs
- [x] Verify daemon tests, desktop build, and native Tauri bundle
- [ ] Commit and push

### Review

- Added a daemon-local Ed25519 identity stored under the daemon state directory
  as `protocol-ed25519-seed`.
- Added `publish_job`, which records a signed `JobAnnouncement` in the daemon's
  local `ProtocolStore` and advances the desktop job to `queued`.
- Added Tauri and TypeScript bindings for `publish_job`.
- Updated the desktop detail view so draft jobs move to funding review, then
  funding-review jobs can publish a protocol announcement.
- Verified:
  - `cargo test -p osciris-daemon publish_job_records_protocol_announcement --locked`
  - `cargo test -p osciris-daemon --locked`
  - `cargo clippy -p osciris-daemon --locked --all-targets -- -D warnings`
  - `pnpm --dir apps/desktop build`
  - `pnpm --dir apps/desktop tauri build`

## Real Receipt Ingestion

### Objective

Make fetched or discovered provider evidence durable as real local receipt
state, not just downloaded files plus bundle metadata. Existing commands verify
receipt availability signatures and bundle hashes, but they do not record the
execution receipt itself after fetching evidence.

### Spec

- Reuse existing signed `ReceiptAvailability` and evidence directory format.
- Validate execution receipt metadata, file hash, bundle metadata, recomputed
  bundle hash, and provider signature.
- After validation, record both the execution receipt and receipt bundle in the
  protocol store.
- Update existing fetch/verify-discovered commands to use the ingestion helper.
- Keep remote HTTP/S3 bundle ingestion out of scope until transfer support is
  implemented.

### Checklist

- [x] Add reusable evidence ingestion helper
- [x] Record execution receipt during local bundle fetch
- [x] Record execution receipt before discovered-receipt verification
- [x] Add tests proving fetched evidence updates durable job state
- [ ] Verify and push

### Review

- Added `ingest_fetched_evidence`, which validates job spec, execution receipt,
  bundle, and provider signature, then records job state, execution receipt,
  and receipt bundle in the local protocol store.
- Updated `network fetch-receipt-bundle` and `network verify-discovered-receipt`
  to call the ingestion helper before reporting success.
- Added a regression test proving fetched evidence updates durable job state in
  a fresh consumer store.
- Verification passed:
  - `cargo test -p osciris-cli ingested_fetched_evidence --locked`
  - `cargo fmt --check`
  - `cargo clippy -p osciris-cli --locked --all-targets -- -D warnings`
  - `cargo test -p osciris-cli --locked`
  - `cargo test --workspace --locked`

## Provider Matching and Execution Protocol Slice

### Objective

Add the missing backend assignment step between provider claims and provider
execution. The protocol already supports signed job announcements, provider
capabilities, provider claims, signed assignments, and auto-provider execution
of assigned jobs. The gap is deterministic assignment from stored signed
claims/capabilities without manual provider selection.

### Spec

- Add a CLI/backend command that loads one announced job and all stored claims
  for that job.
- Verify the job announcement signature against the submitter public key.
- For each claim, require a stored provider capability, matching provider public
  key, valid provider capability signature, valid claim signature, online/idle
  or online/busy provider status, and capability fit.
- Select the best provider deterministically by lowest current load, lowest
  active job count, earliest claim timestamp, then provider node ID.
- Sign and persist a `JobAssignment` with the assigner key.
- Preserve the existing auto-provider execution boundary: providers still
  execute only assignments signed by a configured trusted assigner.
- Do not add desktop UI until this backend surface is proven.

### Checklist

- [x] Add deterministic provider selection helpers
- [x] Add `network auto-assign-job` CLI command
- [x] Add unit coverage for selection and signature validation
- [x] Verify targeted node/CLI tests
- [ ] Update review notes and push

### Review

- Added `osciris-node network auto-assign-job` to select a provider from stored
  signed claims and persist a signed assignment.
- The selector verifies the job announcement signature, provider claim
  signature, provider capability signature, public-key consistency, provider
  online status, and capability fit before assignment.
- Deterministic ranking is lowest current load, lowest active job count,
  earliest claim timestamp, then provider node ID.
- Existing assignments are returned idempotently instead of being replaced.
- The existing auto-provider execution path remains unchanged: providers still
  execute only assignments signed by a configured trusted assigner.
- Added tests for lowest-load valid claimant selection and tampered-claim
  rejection.
- Verification passed:
  - `cargo test -p osciris-cli auto_assign --locked`
  - `cargo test -p osciris-node job_matching --locked`
  - `cargo fmt --check`
  - `cargo clippy -p osciris-cli --locked --all-targets -- -D warnings`
  - `cargo test -p osciris-cli --locked`
  - `cargo test -p osciris-node --locked`
  - `cargo test --workspace --locked` (74 passed, 1 ignored live-RPC test)

## Desktop OSCIRIS Branding

### Objective

Replace the default or mismatched desktop branding with the existing OSCIRIS
mark already present in the workspace, both for packaged app icons and in-app
navigation branding.

### Spec

- Use the canonical `apps/desktop/src-tauri/icons/osciris.svg` mark.
- Configure Tauri packaging to use the existing generated icon set instead of
  relying on implicit defaults.
- Replace the simplified in-app hex/leaf glyph with the same OSCIRIS
  three-sweep mark.
- Verify frontend and native desktop builds after the branding change.

### Checklist

- [x] Add explicit Tauri bundle icon configuration
- [x] Replace the in-app custom mark with the canonical OSCIRIS mark
- [x] Build/test the desktop app
- [ ] Commit and push the branding update

### Review

- Tauri packaging now explicitly uses the existing desktop icon set, including
  `icon.icns`, `icon.ico`, `icon.png`, and standard PNG sizes.
- The in-app sidebar brand mark now uses the same three-sweep OSCIRIS vector
  shape as `apps/desktop/src-tauri/icons/osciris.svg`.
- Verified the generated macOS bundle declares `CFBundleIconFile => icon.icns`
  and includes `Contents/Resources/icon.icns`.
- Desktop verification passed:
  - `pnpm --dir apps/desktop build`
  - `pnpm --dir apps/desktop tauri build`

## Protocol Settlement and Execution Backlog

### Objective

Continue from the desktop workspace PR by turning the next protocol gaps into
real, verifiable backend behavior: ERC-20 job escrow, provider
matching/execution, and receipt ingestion. Start with the smallest protocol
slice that removes a known code-level blocker without overstating deployed
contract capability.

### Spec

- Keep native-token escrow behavior unchanged.
- Allow configured ERC-20 payment tokens to be passed to the deployed escrow
  contract without attaching native value.
- Validate that an existing on-chain escrow matches the requested amount,
  verifier count, and payment token before treating creation as idempotent.
- Keep transaction journaling, signer locking, and replay-safe resume behavior.
- Do not invent token allowance, custody, or final settlement semantics that are
  not exposed by the current escrow ABI.
- Treat provider matching/execution and receipt ingestion as follow-on slices
  after escrow creation no longer rejects ERC-20 payment tokens.

### Checklist

- [x] Remove the hard ERC-20 escrow rejection in `osciris-chain`
- [x] Add payment-token validation for idempotent existing escrow checks
- [x] Ensure native escrow attaches value and ERC-20 escrow attaches zero value
- [x] Add targeted tests for native/ERC-20 escrow preparation logic
- [x] Update protocol documentation and review notes
- [x] Run targeted chain tests and broader workspace verification

### Review

- `osciris-chain` no longer rejects nonzero configured payment-token addresses
  before escrow creation.
- Native-token escrow creation still attaches the escrow amount as transaction
  value.
- ERC-20-token escrow creation passes the configured payment token to the
  escrow contract and attaches zero native value.
- Existing escrow idempotency now validates the payment token as well as amount
  and verifier count before returning `already_created`.
- The desktop product docs now distinguish chain-client ERC-20 support from
  deployed contract, allowance, and verified-token requirements.
- Targeted chain verification passed: `cargo test -p osciris-chain --locked`
  (15 tests).
- Formatting and strict lint verification passed: `cargo fmt --check` and
  `cargo clippy -p osciris-chain --locked --all-targets -- -D warnings`.
- Full Rust workspace verification passed: `cargo test --workspace --locked`
  (72 passed, 1 ignored live-RPC test).

## Investor-Ready Compute Workspace

### Objective

Turn OSCIRIS Node Desktop into a holistic buyer and operator workspace that
communicates the complete product: create training and inference jobs, track
their lifecycle, inspect results and verification evidence, and manage the
testnet treasury boundary without inventing network activity or taking custody
of private keys.

### Product Surface

- Overview: spend, active jobs, verified completions, node/network readiness.
- Jobs: training and inference filters with draft, funding, queue, execution,
  verification, completed, and failed states.
- New job: model, workload, privacy mode, hardware profile, verifier quorum,
  budget, and challenge-window inputs.
- Job detail: timeline, economics, provider assignment, artifacts, execution
  receipt, verifier result, and chain anchor.
- Wallet: watch-only Horizen testnet address, native/test-token balances,
  deposit coordinates, committed funds, spend history, and withdrawal
  preparation for external signing.
- Evidence: receipt and anchor surfaces derived only from daemon records.

### Trust Boundary

- Persist job drafts and wallet configuration in the per-user daemon state.
- Keep private keys and seed phrases outside OSCIRIS Desktop.
- Read balances over the official Horizen testnet RPC.
- Label configurable ERC-20 balances as test tokens, never official testnet
  USDC.
- Do not enable funded job submission or ERC-20 withdrawal while protocol
  escrow rejects non-native payment tokens.
- Allow externally signed transaction preparation only after a settlement-token
  contract is explicitly configured.

### Checklist

- [x] Add daemon job, evidence, wallet, and transaction-preparation types
- [x] Add versioned IPC commands and atomic persistence
- [x] Add Horizen testnet balance reads and fail-closed address validation
- [x] Add Overview, Jobs, Job Detail, Evidence, and Wallet navigation
- [x] Add training and inference job creation flows
- [x] Add lifecycle, economics, receipt, and verifier components
- [x] Add watch-only deposit and external-signing withdrawal flows
- [x] Add responsive investor-demo visual states and honest empty states
- [x] Add daemon, bridge, and frontend tests
- [x] Update architecture, security, and product-boundary documentation
- [x] Run full cross-platform-quality verification

### Review

- Added persisted training and inference drafts with privacy, hardware, quorum,
  challenge-window, and stable-value budget controls.
- Added an explicit draft-to-funding-review transition. Later lifecycle states
  remain protocol-owned and cannot be fabricated by Desktop.
- Added Overview, Compute Jobs, Job Detail, Evidence, Wallet, and Local Node
  product surfaces with pending, running, completed, and failed filtering.
- Added watch-only Horizen testnet wallet configuration, official RPC chain-ID
  validation, native balance reads, configurable test-token reads, deposit
  coordinates, committed-budget reporting, and unsigned ERC-20 transfer
  preparation.
- Private keys and seed phrases remain outside Desktop. Zero addresses are
  rejected and withdrawal preparation remains disabled until a nonzero
  test-token contract is configured.
- Added an explicit stablecoin boundary: Horizen documents mainnet USDC but no
  official Horizen-testnet USDC contract, so the UI uses `USDC_TEST` and does
  not present it as Circle-issued USDC.
- Native smoke testing passed in an isolated state directory:
  - bundled sidecar started from the `.app`
  - inference draft persisted
  - funding-review transition persisted
  - Horizen RPC balance synchronized
  - committed budget updated to match the submitted job
- Verification passed:
  - 70 workspace tests passed; one live-RPC test remains ignored by default
  - live Horizen RPC test passed separately
  - strict Clippy passed for protocol and Tauri workspaces
  - production frontend build passed without compatibility warnings
  - production dependency audit found no known vulnerabilities
  - macOS arm64 `.app` bundled GUI, daemon, `LICENSE`, and `NOTICE`
  - responsive `860x620` review found no horizontal overflow

## Cross-Platform OSCIRIS Node Desktop Foundation

### Objective

Create the first real desktop vertical slice for macOS, Windows, and Linux:
a versioned per-user daemon API plus a Tauri GUI that starts the daemon, reads
real process state, and pauses or resumes participation without duplicating
protocol logic in the frontend.

### Architecture

- Add `osciris-daemon` as the long-running local owner of node state.
- Use a per-user Unix socket on macOS/Linux and named pipe on Windows.
- Use bounded, newline-framed JSON with an explicit API version.
- Keep signing material and filesystem access out of the webview.
- Let the Tauri Rust layer expose only typed daemon operations.
- Keep the frontend local-only with a restrictive CSP and no remote content.
- Treat the desktop app as a controller; closing its window must not define
  network participation state.

### Desktop MVP

- Node status: daemon version, uptime, participation mode, and network state.
- Controls: launch daemon, retry connection, pause, and resume.
- Honest pending states for identity, hardware, model profiles, jobs, receipts,
  and readiness until their APIs are implemented.
- Light technical visual system using OSCIRIS cyan, warm white surfaces,
  hairline borders, restrained motion, and mono status metadata.

### Checklist

- [x] Add versioned daemon request/response types
- [x] Add secure cross-platform per-user IPC server and client
- [x] Persist participation mode atomically
- [x] Add daemon protocol and transport tests
- [x] Scaffold Tauri 2, React, TypeScript, and Vite desktop app
- [x] Connect launch, status, pause, and resume controls
- [x] Add responsive desktop dashboard and offline/error states
- [x] Bundle the target-native daemon with the desktop app
- [x] Add desktop architecture and development documentation
- [x] Add macOS, Linux, and Windows desktop CI
- [x] Verify Rust workspace tests, frontend build, and native macOS bundle
- [x] Review security boundaries and changed-file scope

### Review

- Added `osciris-daemon` with API v1, bounded newline-framed JSON, request IDs,
  constant-time credential checks, and fail-closed participation defaults.
- Added mode `0600` Unix sockets under mode `0700` per-user state directories
  and local-only Windows named pipes.
- Persisted participation state atomically and kept identity, hardware,
  network, jobs, receipts, and readiness explicitly pending until measured APIs
  exist.
- Added a Tauri 2 desktop controller with only typed status, launch, pause, and
  resume commands. React has no shell, filesystem, network, or secret access.
- Added host-target sidecar preparation. Native packages embed the daemon,
  Apache-2.0 `LICENSE`, and `NOTICE`.
- Added a three-platform desktop workflow for daemon tests, frontend builds,
  sidecar preparation, and Tauri bridge compilation.
- Verification passed:
  - `cargo test --workspace --locked`: 67 tests passed
  - strict Clippy for the protocol workspace and Tauri workspace
  - production frontend build: 199.13 kB JavaScript, 62.65 kB gzip
  - `pnpm audit --prod`: no known vulnerabilities
  - macOS arm64 `.app` build with both native executables
  - packaged daemon reports `osciris-daemon 0.1.1`
  - packaged `LICENSE` and `NOTICE` match repository files byte-for-byte
  - responsive check at `860x620`: no horizontal overflow or browser errors

## Provider-Local Inference Round-Trip Milestone

### Objective

Publish a practical, testable milestone for sending an inference request from a
developer machine to an eligible OSCIRIS peer, executing Qwen3-4B locally on
that peer, and returning a signed result and verifiable receipt without a
central inference server.

### Checklist

- [x] Audit existing job, provider, P2P receipt, verifier, and milestone commands
- [x] Define the pinned model profile and hardware envelope
- [x] Document the end-to-end operator run and trust boundary
- [x] Separate currently working commands from commands that must be implemented
- [x] Define observable quorum and capacity-gap acceptance criteria
- [x] Link the runbook from README and milestone documentation
- [x] Create the GitHub milestone and scoped implementation issues
- [x] Verify links, command references, changed-file scope, and repository tests

### Review

- Added `docs/milestones/provider_local_inference_roundtrip.md` as the
  implementation contract for developer-to-peer-to-developer inference.
- Pinned the official Qwen3-4B Q4_K_M artifact by repository revision,
  filename, size, and SHA-256.
- Separated working v0.1.1 primitives from the proposed interactive inference
  CLI so future commands are not presented as released.
- Defined provider-local execution, prompt/result privacy boundaries,
  capability matching requirements, capacity gaps, verifier quorum, a
  multi-host test, and completion evidence.
- Verification passed: local Markdown links, upstream model metadata,
  `git diff --check`, and `cargo test --workspace --locked` (62 tests).

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
