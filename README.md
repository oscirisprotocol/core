# OSCIRIS Core

`protocol-rs` is the public developer entrypoint for OSCIRIS.

OSCIRIS is a privacy-first decentralized AI training and inference protocol. This workspace
contains the Rust node CLI, off-chain protocol runtime, verifier path, and
Horizen testnet chain client.

## Current Progress

- Early developer MVP is published.
- Contributors can install `osciris-node`, generate identities, run the local
  settlement demo, and follow the multi-host testnet join guide.
- The current protocol slice supports signed claims, enterprise assignment,
  provider execution receipts, verifier receipts, quorum, challenge gating, and
  settlement-ready local state.
- Contributors can publish signed milestone records for completed training or
  inference runs so peers can inspect shared progress from the same local store.
- Receipt-backed Python workloads now include both `llm_lora_economics` and
  `inference_economics`.

## Latest Metrics

Primary inference evidence on AWS `g6e.xlarge` NVIDIA L40S:

- `Qwen/Qwen2.5-7B-Instruct` cost-to-quality savings: `58.87%`
- `Qwen/Qwen2.5-3B-Instruct` cost-to-quality savings: `59.53%`
- `mistralai/Mistral-7B-Instruct-v0.3` cost-to-quality savings: `42.62%`
- fixture: `enterprise_policy_qa_fixtures`, `24` cases, seeds `11,22,33`

Primary adaptation evidence: `osciris-enterprise-eff-20260603-1920`

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

## Beta Collaboration Mode

Early contributors can download a release binary from GitHub Releases or
build the CLI locally, then sync the published bundle feed from OSCIRIS Labs:

```bash
osciris-node network sync-published \
  --work-root /tmp/osciris-client \
  --base-url https://oscirislabs.com
```

The command caches the reviewed participant snapshot, proof feed, and
contributor manifest under the local `.osciris/published` directory, along
with the public beta release manifest used for update checks. Running it with
`--watch` keeps the bundle cache updated from the core team’s public publish
surface.

Check whether the binary is current:

```bash
osciris-node network check-updates \
  --work-root /tmp/osciris-client \
  --base-url https://oscirislabs.com
```

The command compares the running binary version against the public beta
manifest and reports the matching release asset when a newer build is
available.

Colleague onboarding notes:

[docs/beta_collaboration.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/beta_collaboration.md)

One-command bootstrap:

```bash
bash scripts/bootstrap_beta_collaboration.sh
```

## Quick Start

```bash
osciris-node --version
osciris-node doctor --repo-root /absolute/path/to/OSCIRIS
osciris-node demo local-settlement
osciris-node demo contributor-flow --work-root /tmp/osciris-demo
```

The local-settlement demo writes local artifacts for job status, settlement
status, a signed milestone record, and a participant snapshot JSON for the
same job. The contributor-flow demo wraps the same settlement path with a
readable install, identity, capability, claim, receipt, verifier, and
milestone workflow manifest for GPU peers.

Generate a contributor identity:

```bash
osciris-node identity generate \
  --node-id provider-a \
  --role provider \
  --display-name "Provider A" \
  --work-root /tmp/osciris-provider-a
```

Generate an inference-economics job spec:

```bash
osciris-node submit-job \
  --job-type inference_economics \
  --dataset enterprise_policy_qa_fixtures \
  --model-id mistralai/Mistral-7B-Instruct-v0.3 \
  --samples 24 \
  --seeds 11,22,33 \
  --backend transformers_causal_lm \
  --output /tmp/osciris-inference-job.json
```

Run the job as a provider:

```bash
printf "%s" "$PROVIDER_SEED" > /run/osciris/provider-a.seed
chmod 600 /run/osciris/provider-a.seed

osciris-node run-provider \
  --job-spec /tmp/osciris-inference-job.json \
  --provider-id provider-a \
  --signing-key-id provider-a-key \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --repo-root /absolute/path/to/OSCIRIS \
  --work-root /tmp/osciris-provider-a
```

Publish the provider capability and signed job claim:

```bash
osciris-node network create-provider-capability \
  --work-root /tmp/osciris-provider-a \
  --node-id provider-a \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --host-class aws_g5_xlarge \
  --gpu-model "NVIDIA A10G" \
  --gpu-count 1 \
  --vram-gb 24 \
  --cuda-available true \
  --supported-job-type llm_lora_economics \
  --supported-runtime python3 \
  --pricing-hint "testnet-credits" \
  --current-load 0 \
  --active-job-count 0

osciris-node network create-job-claim \
  --work-root /tmp/osciris-provider-a \
  --job-id <job-id> \
  --provider-id provider-a \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --claim-note "matched gpu>=24gb"
```

Publish a milestone after the evidence bundle and verifier receipts exist:

```bash
osciris-node network publish-milestone \
  --work-root /tmp/osciris-provider-a \
  --job-id <job-id> \
  --title "Inference quality milestone" \
  --summary "Provider A completed the shared inference checkpoint." \
  --quality-metric-name quality_retention \
  --quality-metric-value 0.91 \
  --publisher-id enterprise-1 \
  --signing-key-id enterprise-key \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

Contributors can inspect the shared job, evidence, verifier, and milestone
state in one read-only snapshot with:

```bash
osciris-node network participant-status \
  --work-root /tmp/osciris-provider-a \
  --job-id <job-id> \
  --output /tmp/osciris-participant-status.json
```

Multi-machine onboarding:

[docs/multi_host_testnet_join_guide.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/multi_host_testnet_join_guide.md)

MVP operator path:

[docs/mvp_operator_runbook.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/mvp_operator_runbook.md)

Horizen testnet integration:

[docs/horizen_mvp_integration.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/protocol-rs/docs/horizen_mvp_integration.md)

## Related Repos

- DSP engine and benchmarks: [../README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/README.md)
- Horizen testnet contracts: [../contracts/README.md](/Users/meshachishaya/CascadeProjects/windsurf-project/OSCIRIS/contracts/README.md)

## Current Boundary

This repo does not claim:

- mainnet readiness
- audited privacy guarantees
- public permissionless bootstrap
- trustless hardware attestation
- production inference SLA
