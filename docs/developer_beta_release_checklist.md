# OSCIRIS Developer Beta Release Checklist

Use this checklist before publishing a new public `osciris-node` beta build for
contributors.

Recommended end-to-end operator command:

```bash
bash scripts/run_beta_release_flow.sh \
  --version 0.1.1 \
  --release-notes "Beta collaboration release for colleague onboarding, published bundle sync, and release checks." \
  --asset macos-aarch64=/absolute/path/to/osciris-node \
  --asset linux-x86_64=/absolute/path/to/osciris-node \
  --asset windows-x86_64=/absolute/path/to/osciris-node.exe \
  --website-public-dir /absolute/path/to/OSCIRISLABS/public \
  --verify-base-url https://oscirislabs.com
```

Use `--publish-release` when you want the script to create or update the GitHub
prerelease after packaging.

When `--website-public-dir` is provided, the flow now verifies the generated
manifest against its referenced release asset URLs before copying
`beta-release-manifest.json` into the website public directory. This fails
closed by default so the website cannot advertise a broken binary-only install
path by accident.

Use `--allow-unverified-website-manifest` only when you are intentionally
publishing a fallback-oriented manifest before the GitHub assets exist and you
understand that binary-only onboarding will still fail until those assets are
uploaded.

Underlying packaging command:

```bash
bash scripts/package_beta_release.sh \
  --version 0.1.1 \
  --channel beta \
  --release-page-url https://github.com/oscirisprotocol/core/releases/tag/v0.1.1 \
  --release-notes "Beta collaboration release for colleague onboarding, published bundle sync, and release checks." \
  --base-download-url https://github.com/oscirisprotocol/core/releases/download/v0.1.1 \
  --asset macos-aarch64=/absolute/path/to/osciris-node \
  --asset linux-x86_64=/absolute/path/to/osciris-node \
  --asset windows-x86_64=/absolute/path/to/osciris-node.exe
```

The script emits release archives containing the binary, `LICENSE`, and
`NOTICE`, plus a `beta-release-manifest.json` with
per-asset SHA-256 checksums.

Default local output:

- `dist/beta-release/osciris-node-<platform>.tar.gz`
- `dist/beta-release/osciris-node-windows-x86_64.zip`
- `dist/beta-release/beta-release-manifest.json`

These are generated local release artifacts. They are used by the website
publisher when present, but they should not be committed to the repository.

Recommended release-surface verification command:

```bash
python3 scripts/verify_beta_release_surface.py \
  --base-url https://oscirislabs.com
```

The verifier checks:

- published JSON bundle endpoints
- GitHub release page reachability
- required public platform coverage
- release asset URL reachability
- release archive shape
- asset SHA-256 integrity against the manifest

Current required public beta platforms:

- `macos-aarch64`
- `linux-x86_64`
- `windows-x86_64`

Recommended publication sequence:

1. Build the release binary or binaries locally.
2. Run `bash scripts/run_beta_release_flow.sh ...` to generate the archives and
   manifest under `dist/beta-release/`.
3. Upload the archives to the matching GitHub prerelease.
4. Confirm `gh release view <tag>` shows the same asset names.
5. Republish the OSCIRIS Labs public bundle so `public/beta-release-manifest.json`
   is copied from the generated manifest rather than inferred from placeholder
   asset names.
6. Run `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com`.

If step 5 is attempted before step 3 is complete, `run_beta_release_flow.sh`
will now stop instead of copying a manifest that points to missing assets,
unless `--allow-unverified-website-manifest` is set explicitly.

## Scope

This beta should stay narrowly scoped to the current contributor workflow:

- install `osciris-node`
- run `doctor`
- run `demo local-settlement`
- run `demo contributor-flow`
- sync the published bundle feed
- check for newer beta builds
- inspect participant-visible job state
- follow the guided multi-host/testnet onboarding path

Do not expand the public message beyond the current repo boundary.

## Release Acceptance Checklist

- [ ] Workspace and crate metadata report `Apache-2.0`.
- [ ] The release version is `0.1.1` or later; historical `v0.1.0` remains MIT.
- [ ] GitHub Release assets exist for the intended beta version.
- [ ] Every release archive contains the platform binary, `LICENSE`, and `NOTICE`.
- [ ] Public beta manifest includes `macos-aarch64`, `linux-x86_64`, and `windows-x86_64`.
- [ ] Release notes describe the build as an early developer beta.
- [ ] Public beta manifest points to the same version and assets.
- [ ] `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com` passes.
- [ ] If release assets are still missing, the repo-checkout bootstrap fallback is verified explicitly.
- [ ] `osciris-node --version` reports the intended beta version.
- [ ] macOS install path is verified from the published release binary.
- [ ] Linux install path is verified from the published release binary.
- [ ] Windows install path is verified from the published release binary and `scripts/bootstrap_beta_collaboration.ps1`.
- [ ] Source fallback still works with `cargo install --path crates/osciris-cli`.
- [ ] `osciris-node doctor --repo-root /absolute/path/to/OSCIRIS` succeeds.
- [ ] `osciris-node demo local-settlement` succeeds.
- [ ] `osciris-node demo contributor-flow --work-root /tmp/osciris-demo --repo-root /absolute/path/to/OSCIRIS` succeeds.
- [ ] `osciris-node network sync-published --work-root /tmp/osciris-client --base-url https://oscirislabs.com` succeeds.
- [ ] `osciris-node network check-updates --work-root /tmp/osciris-client --base-url https://oscirislabs.com` succeeds.
- [ ] `osciris-node network participant-status --work-root /tmp/osciris-provider-a --job-id <job-id> --output /tmp/osciris-participant-status.json` is verified against a known-good job.
- [ ] `docs/beta_collaboration.md` matches the shipped install and sync path.
- [ ] `docs/multi_host_testnet_join_guide.md` matches the intended collaborator flow.
- [ ] `docs/mvp_operator_runbook.md` matches the intended MVP operator flow.
- [ ] Public messaging does not claim mainnet readiness, audited privacy, trustless attestation, or production SLA.

## Recommended Release Message

Use language close to the following:

> OSCIRIS Developer Beta is now open. Install `osciris-node`, generate your
> contributor identity, run the local protocol demo, sync the public proof
> bundles, and join the early provider and verifier workflow.

## Boundary Reminder

This beta is for developers and early contributors. It is not a mainnet
release, not an audited privacy product, and not a production inference SLA.
