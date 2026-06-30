# OSCIRIS Release Workflow Handoff

The release workflow is now live in this repo and has been verified on GitHub
Actions across all three beta platforms:

- `macos-aarch64`
- `linux-x86_64`
- `windows-x86_64`

The public beta release surface has also been updated and verified against
`https://oscirislabs.com`.

## Current State

- Workflow file present: `.github/workflows/release.yml`
- Branch and PR validation:
  - PR run `28387903806` passed
  - push run `28387900403` passed
- GitHub prerelease:
  - tag `v0.1.0` (historical MIT-licensed release)
  - includes:
    - `osciris-node-macos-aarch64.tar.gz`
    - `osciris-node-linux-x86_64.tar.gz`
    - `osciris-node-windows-x86_64.zip`
- Public website manifest:
  - `https://oscirislabs.com/beta-release-manifest.json`
  - includes all three required platforms with matching SHA-256 hashes
- Public verifier:
  - `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com`
  - passes

## Workflow Behavior

The GitHub Actions workflow is intentionally split into two modes:

- PRs and branch pushes:
  - run the full macOS/Linux/Windows build matrix
  - upload build artifacts for inspection
  - do not publish a GitHub Release
- tag pushes `v*` and `workflow_dispatch`:
  - run the same build matrix
  - package deterministic archives through `scripts/package_beta_release.sh`
  - publish the prerelease assets

This keeps release publication gated while still proving Windows/MSVC build
health on ordinary review flows.

## Operator Notes

When publishing or repairing a beta release manually, prefer the repo script:

```bash
bash scripts/run_beta_release_flow.sh \
  --version 0.1.1 \
  --release-notes "Beta collaboration release for colleague onboarding, published bundle sync, and release checks." \
  --asset macos-aarch64=/absolute/path/to/osciris-node \
  --asset linux-x86_64=/absolute/path/to/osciris-node \
  --asset windows-x86_64=/absolute/path/to/osciris-node.exe \
  --publish-release \
  --website-public-dir /absolute/path/to/OSCIRISLABS/public
```

Important behavior:

- The script packages deterministic Unix `.tar.gz` archives and a deterministic
  Windows `.zip`.
- Every archive contains the platform binary, the Apache-2.0 `LICENSE`, and
  `NOTICE`.
- If `--website-public-dir` is set, it refuses to copy the public manifest
  until the referenced GitHub release assets verify cleanly.
- Right after `gh release upload --clobber`, GitHub may briefly serve stale
  asset bytes under existing tag URLs. If that happens, wait for propagation
  and rerun verification before publishing the website manifest.

## Verification Checklist

For a future beta refresh, confirm all of the following:

- the release version is `0.1.1` or later and Cargo metadata is `Apache-2.0`
- `gh release view <tag>` shows all three expected assets
- every release archive contains its binary, `LICENSE`, and `NOTICE`
- `python3 scripts/verify_beta_release_surface.py --base-url https://oscirislabs.com` passes
- the website manifest asset hashes match the actual GitHub release downloads
- Windows onboarding uses `scripts/bootstrap_beta_collaboration.ps1`
- release publication remains gated to tag/manual workflow runs
