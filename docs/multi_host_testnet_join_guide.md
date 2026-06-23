# OSCIRIS Multi-Host Testnet Join Guide

This guide is for early contributors running the OSCIRIS developer MVP across
separate machines.

Target topology:

- 1 bootstrap or enterprise node
- 1 provider node
- 1 verifier node

This is an off-chain protocol join guide first. It does not require AWS or
Horizen transactions for the initial workflow proof.

## Prerequisites

- Rust with `cargo`
- `python3`
- `uv`
- network reachability between participating machines

Install the CLI:

```bash
cargo install --path crates/osciris-cli
```

Smoke check:

```bash
osciris-node --version
osciris-node doctor --repo-root /absolute/path/to/OSCIRIS
```

## Suggested Ports

- bootstrap or enterprise node: `4101`
- provider node: `4102`
- verifier node: `4103`

The `listen_addr` examples below use those ports. Adjust to match your network.

## 1. Generate Identities

Run this once per node.

Enterprise or bootstrap:

```bash
osciris-node identity generate \
  --node-id enterprise-1 \
  --role enterprise \
  --display-name "Enterprise 1" \
  --work-root /tmp/osciris-enterprise
```

Provider:

```bash
osciris-node identity generate \
  --node-id provider-a \
  --role provider \
  --display-name "Provider A" \
  --work-root /tmp/osciris-provider-a
```

Verifier:

```bash
osciris-node identity generate \
  --node-id verifier-1 \
  --role verifier \
  --display-name "Verifier 1" \
  --work-root /tmp/osciris-verifier-1
```

Record from the output:

- `signing_key_seed_base64`
- `ed25519_public_key_base64`
- `peer_id`

For MVP operations, store the seed in a private runtime file and pass
`--signing-key-seed-file`. Avoid passing seed values directly in shell history
or process arguments.

```bash
install -m 700 -d /run/osciris
printf "%s" "<enterprise-seed>" > /run/osciris/enterprise.seed
printf "%s" "<provider-seed>" > /run/osciris/provider-a.seed
printf "%s" "<verifier-seed>" > /run/osciris/verifier-1.seed
chmod 600 /run/osciris/*.seed
```

If the role will later submit on-chain actions, rerun with `--evm-private-key-hex`
or provide a separate EVM wallet during chain registration.

## 2. Start the Bootstrap Presence Node

On the enterprise machine:

```bash
osciris-node network serve \
  --work-root /tmp/osciris-enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101
```

Capture the printed listen address and `peer_id`. Other nodes use it as
`--bootstrap-peer`.

## 3. Start the Provider Node

On the provider machine:

```bash
osciris-node network run-provider \
  --work-root /tmp/osciris-provider-a \
  --repo-root /absolute/path/to/OSCIRIS \
  --signing-key-id provider-a-key \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4102 \
  --bootstrap-peer <bootstrap-multiaddr>
```

## 4. Start the Verifier Node

On the verifier machine:

```bash
osciris-node network run-verifier \
  --work-root /tmp/osciris-verifier-1 \
  --verifier-id verifier-1 \
  --signing-key-id verifier-1-key \
  --signing-key-seed-file /run/osciris/verifier-1.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4103 \
  --bootstrap-peer <bootstrap-multiaddr>
```

## 5. Create and Announce a Job

Create a mock job spec on the enterprise machine:

```bash
osciris-node submit-job --output /tmp/osciris-enterprise/job.json
```

Announce it:

```bash
osciris-node network create-job-announcement \
  --work-root /tmp/osciris-enterprise \
  --job-spec /tmp/osciris-enterprise/job.json \
  --submitter-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

## 6. Confirm Claims and Assign the Provider

Inspect claims:

```bash
osciris-node network claims --work-root /tmp/osciris-enterprise
```

Assign provider A:

```bash
osciris-node network assign-job \
  --work-root /tmp/osciris-enterprise \
  --job-id <job-id> \
  --provider-id provider-a \
  --assigner-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

## 7. Inspect Protocol State

Enterprise or observer:

```bash
osciris-node network job-status --work-root /tmp/osciris-enterprise --job-id <job-id>
osciris-node network quorum-status --work-root /tmp/osciris-enterprise --job-id <job-id>
osciris-node network settlement-status --work-root /tmp/osciris-enterprise --job-id <job-id>
osciris-node network provider-status --work-root /tmp/osciris-enterprise
```

Expected MVP outcome after successful provider execution and verifier acceptance:

- provider A executes once
- non-assigned providers do not execute
- quorum becomes `accepted`
- settlement becomes blocked during the challenge window or open challenge
- settlement becomes `settlement_ready` after challenge rejection or window expiry

## Current Boundary

This flow currently proves:

- multi-host off-chain presence and state sync
- provider claim and enterprise assignment
- provider execution receipt generation
- verifier receipt generation
- quorum and settlement-ready lifecycle

This guide does not yet provide:

- public bootstrap infrastructure
- automatic NAT traversal guarantees
- mainnet deployment
- production key custody policy
