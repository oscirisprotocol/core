# OSCIRIS Developer Beta Release Checklist

Use this checklist before publishing a new public `osciris-node` beta build for
contributors.

Recommended packaging command:

```bash
bash scripts/package_beta_release.sh \
  --version 0.1.0 \
  --channel beta \
  --release-page-url https://github.com/oscirisprotocol/core/releases/tag/v0.1.0 \
  --release-notes "Beta collaboration release for colleague onboarding, published bundle sync, and release checks." \
  --base-download-url https://github.com/oscirisprotocol/core/releases/download/v0.1.0 \
  --asset macos-aarch64=/absolute/path/to/osciris-node \
  --asset linux-x86_64=/absolute/path/to/osciris-node
```

The script emits the release tarballs and a `beta-release-manifest.json` with
per-asset SHA-256 checksums.

Default local output:

- `dist/beta-release/osciris-node-<platform>.tar.gz`
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
- release asset URL reachability
- tarball shape
- asset SHA-256 integrity against the manifest

Recommended publication sequence:

1. Build the release binary or binaries locally.
2. Run `bash scripts/package_beta_release.sh ...` to generate the tarball(s) and
   manifest under `dist/beta-release/`.
3. Upload the tarball(s) to the matching GitHub prerelease.
4. Confirm `gh release view <tag>` shows the same asset names.
5. Republish the OSCIRIS Labs public bundle so `public/beta-release-manifest.json`
   is copied from the generated manifest rather than inferred from placeholder
   asset names.
6. Run `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com`.

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

- [ ] GitHub Release assets exist for the intended beta version.
- [ ] Release notes describe the build as an early developer beta.
- [ ] Public beta manifest points to the same version and assets.
- [ ] `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com` passes.
- [ ] If release assets are still missing, the repo-checkout bootstrap fallback is verified explicitly.
- [ ] `osciris-node --version` reports the intended beta version.
- [ ] macOS install path is verified from the published release binary.
- [ ] Linux install path is verified from the published release binary.
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
