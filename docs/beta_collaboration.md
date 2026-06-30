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

OSCIRIS supports heterogeneous providers. CPU and Apple Silicon hosts can
participate in control, validation, MPS/MLX-compatible workloads, and developer
beta capacity. NVIDIA CUDA hosts are recommended for high-throughput enterprise
AI workloads, with 24 GB VRAM as the practical default tier and 48 GB+ for
enterprise pilots.

Recommended public beta tiers:

| Tier | Minimum hardware | Intended use |
| --- | --- | --- |
| `cpu-control` | 4 CPU cores, 8-16 GB RAM, 50 GB SSD | relay, verifier, enterprise client, proof review |
| `apple-silicon-dev` | Apple M1/M2/M3/M4, 16 GB unified memory | developer beta, validation, light MPS/MLX workloads |
| `apple-silicon-pro` | Apple M-series Pro/Max/Ultra, 24-64 GB unified memory | non-CUDA provider tier, MPS/MLX-compatible workloads |
| `cuda-standard` | NVIDIA CUDA GPU, 24 GB VRAM, 64 GB RAM | default high-value AI provider tier |
| `cuda-enterprise` | NVIDIA CUDA GPU, 48 GB+ VRAM, ECC preferred, 128 GB RAM | enterprise pilots and regulated workload review |
| `frontier` | A100/H100/B200-class or similar, 80 GB+ VRAM | premium large-model workloads |

CUDA is required only for CUDA-specific jobs. Apple Silicon providers should
advertise `mps_available=true`, `cuda_available=false`, and runtimes such as
`python3`, `mps`, or `mlx` when those paths are locally available.

## Participant warnings

- Do not process regulated, confidential, customer, health, financial, or
  classified data unless a separate written review authorizes that use.
- Provider capability claims are signed and checked, but they are not the same
  as full hardware attestation.
- Jobs must match the provider runtime and memory capability.
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
