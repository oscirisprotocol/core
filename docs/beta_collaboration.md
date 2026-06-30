# OSCIRIS Beta Collaboration

This path is for colleagues who want to install the CLI, sync the public
proof bundles, and stay aligned with the latest beta release without needing
direct access to the core team’s local state.

The fastest path is:

```bash
bash scripts/bootstrap_beta_collaboration.sh
```

On Windows PowerShell, use:

```powershell
.\scripts\bootstrap_beta_collaboration.ps1
```

The script prefers the published GitHub release asset referenced by
`beta-release-manifest.json`. If that asset is temporarily missing but the
script is run from a `protocol-rs` checkout with Cargo installed, it falls back
to a local release build and still runs the public sync and update checks.

## 1. Install a binary

Download the latest release binary from GitHub Releases or build it locally:

```bash
cargo install --path crates/osciris-cli
```

The binary is installed as `osciris-node`.

## 2. Sync the public bundle surface

```bash
osciris-node network sync-published \
  --work-root /tmp/osciris-client \
  --base-url https://oscirislabs.com
```

This pulls the read-only participant snapshot, proof feed, contributor
manifest, and beta release manifest into the local `.osciris/published`
cache.

## 3. Check whether the binary is current

```bash
osciris-node network check-updates \
  --work-root /tmp/osciris-client \
  --base-url https://oscirislabs.com
```

The command compares the installed CLI version to the public beta manifest
and reports the matching release asset when an update exists.

On Windows, use a normal writable path for `--work-root`, for example:

```powershell
.\osciris-node.exe network sync-published `
  --work-root "$env:TEMP\osciris-client" `
  --base-url https://oscirislabs.com

.\osciris-node.exe network check-updates `
  --work-root "$env:TEMP\osciris-client" `
  --base-url https://oscirislabs.com
```

## 4. Join the collaborative workflow

Use the contributor flow to inspect install, identity, capability, claim,
receipt, verifier, and milestone state:

```bash
osciris-node demo contributor-flow \
  --work-root /tmp/osciris-demo \
  --repo-root /absolute/path/to/OSCIRIS
```

For GPU peers, the follow-up commands are:

```bash
osciris-node network create-provider-capability ...
osciris-node network create-job-claim ...
osciris-node network run-provider ... --trusted-assigner-public-key-base64 <enterprise-ed25519-public-key>
osciris-node network publish-milestone ...
```

The collaboration boundary is read-only on the public side. The core team
publishes proof bundles and update metadata; contributors pull them locally
and participate through the CLI.

## Release notes for maintainers

- [developer_beta_release_checklist.md](developer_beta_release_checklist.md)
- [developer_beta_launch_copy.md](developer_beta_launch_copy.md)
