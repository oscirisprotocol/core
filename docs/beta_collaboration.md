# OSCIRIS Beta Collaboration

This path is for colleagues who want to install the CLI, sync the public
proof bundles, and stay aligned with the latest beta release without needing
direct access to the core team’s local state.

This is a developer beta path. It is not production mainnet, not an audited
privacy product, not a hardware-attested compute network, and not a production
inference SLA.

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

Supported public beta platforms are listed in the release manifest:

- macOS Apple Silicon: `macos-aarch64`
- Linux x86_64: `linux-x86_64`
- Windows x86_64: `windows-x86_64`

Verify SHA-256 checksums from the manifest before running downloaded assets.
The bootstrap scripts perform this check automatically.

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

## Provider hardware guidance

OSCIRIS supports heterogeneous participants. Joining the developer beta does
not require a GPU. The current evidence-backed 7B CUDA profile baseline is
24 GB VRAM on Linux/NVIDIA CUDA. NVIDIA, AMD, and Apple Silicon hosts may
publish capability at other memory sizes and receive compatible targeted jobs;
benchmark status limits performance claims, not network admission.

Read [hardware requirements](hardware_requirements.md) before publishing a
provider capability. It defines role minimums, GPU tiers, platform support, and
the current A10G/L40S evidence boundary.

CUDA is required only for CUDA-specific jobs. Apple Silicon providers should
advertise `mps_available=true`, `cuda_available=false`, and runtimes such as
`python3`, `mps`, or `mlx` when those paths are locally available.
AMD providers should advertise both CUDA and MPS as unavailable and include
`python3` and `rocm` only when the ROCm path passes a local smoke test.

## Participant warnings

- Do not process regulated, confidential, customer, health, financial, or
  classified data unless a separate written review authorizes that use.
- Provider capability claims are signed and checked, but they are not the same
  as full hardware attestation.
- Current automatic matching checks job type and declared VRAM. Operators must
  validate runtime packages, system memory, disk capacity, and model fit before
  accepting work.
- Windows NVIDIA GPU participation requires host smoke testing before stronger
  public production-readiness claims.
- Rewards, payments, escrow, settlement, and SLA terms are not final unless
  separately agreed.
- Operators are responsible for local security, firewalling, key storage,
  driver updates, workload legality, and data handling compliance.
- Never publish private signing seeds, API keys, datasets, enterprise material,
  or secrets in work roots, logs, screenshots, or issue reports.
- Workloads can fail, be rejected by verifiers, or be excluded if capability
  metadata is incomplete or inconsistent.

## Release notes for maintainers

- [developer_beta_release_checklist.md](developer_beta_release_checklist.md)
- [developer_beta_launch_copy.md](developer_beta_launch_copy.md)
