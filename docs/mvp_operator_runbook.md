# OSCIRIS MVP Operator Runbook

Date: 2026-06-23

## MVP Goal

Run one private AI workload through the minimum useful OSCIRIS flow:

1. Enterprise creates a job.
2. DSP prepares controlled workload artifacts.
3. Provider declares capability and accepts assignment.
4. Provider executes the workload.
5. Verifier accepts or rejects the evidence bundle.
6. Enterprise exports receipt and settlement status.

The MVP proves buyer-visible accountability, not full permissionless mainnet
operation.

## Roles

| Role | Responsibility | MVP output |
| --- | --- | --- |
| Enterprise | Creates job and assigns provider | job spec, assignment, status export |
| Provider | Runs assigned workload | execution receipt and evidence bundle |
| Verifier | Checks evidence and policy result | signed verification receipt |
| Observer | Reviews proof artifacts | evidence index and receipt state |

## Key Handling

Use seed files for operator flows. Do not pass signing seeds directly in shell
arguments for real runs.

```bash
install -m 700 -d /run/osciris
printf "%s" "$PROVIDER_SEED_BASE64" > /run/osciris/provider-a.seed
chmod 600 /run/osciris/provider-a.seed
```

Provider capability signing:

```bash
osciris-node network create-provider-capability \
  --work-root /tmp/osciris-provider-a \
  --node-id provider-a \
  --signing-key-id provider-a-key \
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
```

## Enterprise Job

Create an inference economics job:

```bash
osciris-node submit-job \
  --job-type inference_economics \
  --dataset enterprise_policy_qa_fixtures \
  --model-id Qwen/Qwen2.5-7B-Instruct \
  --samples 24 \
  --seeds 11,22,33 \
  --backend transformers_causal_lm \
  --output /tmp/osciris-enterprise/job.json
```

Announce and assign:

```bash
osciris-node network create-job-announcement \
  --work-root /tmp/osciris-enterprise \
  --job-spec /tmp/osciris-enterprise/job.json \
  --submitter-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed

osciris-node network assign-job \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id> \
  --provider-id provider-a \
  --assigner-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

Provider claim after capability publish:

```bash
osciris-node network create-job-claim \
  --work-root /tmp/osciris-provider-a \
  --job-id <job-id> \
  --provider-id provider-a \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --claim-note "matched gpu>=24gb"
```

## Provider Execution

```bash
osciris-node network run-provider \
  --work-root /tmp/osciris-provider-a \
  --repo-root /absolute/path/to/OSCIRIS \
  --signing-key-id provider-a-key \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4102 \
  --bootstrap-peer <bootstrap-multiaddr>
```

Expected provider output:

- execution receipt
- evidence manifest
- model/runtime metrics
- signed provider identity
- evidence root hash

## Verifier Receipt

```bash
osciris-node network run-verifier \
  --work-root /tmp/osciris-verifier-1 \
  --verifier-id verifier-1 \
  --signing-key-id verifier-1-key \
  --signing-key-seed-file /run/osciris/verifier-1.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4103 \
  --bootstrap-peer <bootstrap-multiaddr>
```

Expected verifier output:

- signed verification receipt
- accepted or rejected result
- evidence hash checked against manifest
- quorum contribution

## Milestone Publication

After the verifier receipts are present, publish the communal milestone record:

```bash
osciris-node network publish-milestone \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id> \
  --title "Shared inference milestone" \
  --summary "The provider and verifier peers completed the first shared quality checkpoint." \
  --quality-metric-name quality_retention \
  --quality-metric-value 0.91 \
  --publisher-id enterprise-1 \
  --signing-key-id enterprise-key \
  --signing-key-seed-file /run/osciris/enterprise.seed

osciris-node network milestones --work-root /tmp/osciris-enterprise --job-id <job-id>
```

The milestone record is the public participant-facing proof that a workload
has moved from private execution into a shared, inspectable result.

## Buyer-Visible Status

```bash
osciris-node network job-status \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id>

osciris-node network participant-status \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id> \
  --output /tmp/osciris-participant-status.json

osciris-node network quorum-status \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id>

osciris-node network settlement-status \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id>
```

The MVP is successful when:

- assigned provider executed exactly once
- verifier receipt is present
- quorum status is accepted
- challenge status is closed or expired
- settlement status is `settlement_ready`
- evidence package is sanitized and reviewable

## Evidence Package

Pack sanitized reviewer evidence:

```bash
python3 tools/full_settlement_proof.py pack \
  --run-root /tmp/osciris-enterprise \
  --output /tmp/osciris-mvp-evidence.tar.gz
```

The package must include:

- `evidence/redaction_manifest.json` when redactions occur
- job receipt
- verifier receipt
- manifest hash
- benchmark summary
- status export

## MVP Boundary

This runbook does not claim:

- production SLA
- audited privacy guarantee
- permissionless provider admission
- mainnet economic security
- automatic confidential hardware attestation
