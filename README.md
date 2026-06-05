# OSCIRIS Core

`protocol-rs` is the public developer entrypoint for OSCIRIS.

OSCIRIS is a privacy-first decentralized AI training protocol. This Rust workspace
contains the protocol runtime, the local node CLI, receipt verification, and the
Horizen testnet chain client used to anchor accepted receipt bundles.

This repo is for early developer MVP and testnet collaboration. It proves the
off-chain workflow and testnet settlement path. It does not claim mainnet
readiness, audited privacy, or public network bootstrap stability yet.

## Repo Map

- `protocol-rs`: Rust protocol runtime and public CLI
- `../`: DSP Python execution and benchmarking engine
- `../contracts`: Solidity contracts and Foundry tests for Horizen testnet

## Prerequisites

- Rust toolchain with `cargo`
- `python3` for the local mock settlement demo and Python-backed workloads
- `uv` to invoke the DSP repo from protocol workflows
- `forge` if you want to run or modify the Solidity contracts

## Install

From the `protocol-rs` repo root:

```bash
cargo install --path crates/osciris-cli
```

This installs the public binary as `osciris-node`.

## Smoke Test

```bash
osciris-node --version
osciris-node doctor
```

Optional DSP bridge check:

```bash
osciris-node doctor --repo-root /absolute/path/to/OSCIRIS
```

`doctor` is protocol-first. It checks CLI/runtime readiness and reports optional
tooling such as `uv`, `python3`, `forge`, and DSP health when a valid DSP repo is
supplied.

## One-Command Local Demo

Run the off-chain settlement lifecycle locally:

```bash
osciris-node demo local-settlement
```

The demo:

- creates an isolated local work root
- creates one enterprise job
- records two provider claims
- assigns provider A
- runs one mocked provider execution
- creates receipt availability
- verifies the receipt bundle
- accepts quorum
- opens a challenge
- resolves the challenge rejected
- finishes in `settlement_ready`

Inspect the printed `work_root` after the run. The demo writes:

- `job_status.json`
- `provider_status.json`
- `quorum_status.json`
- `settlement_status.json`
- signed job, claim, assignment, and challenge artifacts

Use a fixed work root if you want deterministic output paths:

```bash
osciris-node demo local-settlement --work-root /tmp/osciris-demo
```

## Multi-Host Join Guide

For separate enterprise, provider, and verifier machines, use
[docs/multi_host_testnet_join_guide.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/multi_host_testnet_join_guide.md).

The CLI now includes contributor identity generation:

```bash
osciris-node identity generate \
  --node-id provider-a \
  --role provider \
  --display-name "Provider A" \
  --work-root /tmp/osciris-provider-a
```

## Main Commands

- `osciris-node doctor`
- `osciris-node demo local-settlement`
- `osciris-node network serve`
- `osciris-node network run-provider`
- `osciris-node network run-verifier`
- `osciris-node network assign-job`
- `osciris-node network job-status`
- `osciris-node watch-chain`

## Development Checks

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Related Repos

- DSP engine and benchmarks: [../README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/README.md)
- Contracts and testnet deployment assets: [../contracts/README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/contracts/README.md)

## Current Boundary

The current developer MVP supports:

- signed provider claims and enterprise assignment
- provider execution receipts
- verifier receipts and quorum
- challenge gating
- settlement-ready off-chain lifecycle
- Horizen testnet contract/client integration

It does not yet provide:

- public mainnet bootstrap
- trustless hardware attestation
- audited privacy guarantees
- production-grade permissioning or economic policy

## Contribution Entry Points

- `crates/osciris-cli`: public developer commands and demos
- `crates/osciris-node`: execution/runtime and local protocol state
- `crates/osciris-verifier`: receipt verification logic
- `crates/osciris-chain`: Horizen testnet interaction
