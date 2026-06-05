# OSCIRIS Core

`protocol-rs` is the public developer entrypoint for OSCIRIS.

OSCIRIS is a privacy-first decentralized AI training protocol. This workspace
contains the Rust node CLI, off-chain protocol runtime, verifier path, and
Horizen testnet chain client.

## Current Progress

- Early developer MVP is published.
- Contributors can install `osciris-node`, generate identities, run the local
  settlement demo, and follow the multi-host testnet join guide.
- The current protocol slice supports signed claims, enterprise assignment,
  provider execution receipts, verifier receipts, quorum, challenge gating, and
  settlement-ready local state.

## Latest Metrics

Primary bounded evidence: `osciris-enterprise-eff-20260603-1920`

- `Qwen/Qwen2.5-7B-Instruct` mean cost-to-quality savings: `16.08%`
- `mistralai/Mistral-7B-v0.1` mean cost-to-quality savings: `12.55%`
- completed benchmark rows: `6/6`

Latest protocol proof shape:

- multi-host off-chain workflow across enterprise, provider, and verifier roles
- accepted local settlement demo ending in `settlement_ready`
- Horizen testnet contract package published separately in `contracts`

## Install

```bash
cargo install --path crates/osciris-cli
```

This installs the public binary as `osciris-node`.

## Quick Start

```bash
osciris-node --version
osciris-node doctor --repo-root /absolute/path/to/OSCIRIS
osciris-node demo local-settlement
```

Generate a contributor identity:

```bash
osciris-node identity generate \
  --node-id provider-a \
  --role provider \
  --display-name "Provider A" \
  --work-root /tmp/osciris-provider-a
```

Multi-machine onboarding:

[docs/multi_host_testnet_join_guide.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/multi_host_testnet_join_guide.md)

## Related Repos

- DSP engine and benchmarks: [../README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/README.md)
- Horizen testnet contracts: [../contracts/README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/contracts/README.md)

## Current Boundary

This repo does not claim:

- mainnet readiness
- audited privacy guarantees
- public permissionless bootstrap
- trustless hardware attestation
