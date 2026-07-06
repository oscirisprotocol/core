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

> **Use a persistent `--work-root`, not `/tmp`.** The work-root holds the node's
> local protocol store, including its saved identity. macOS clears `/tmp` on
> reboot, so a node whose work-root lives under `/tmp` will lose its identity and
> fail to start with `local node identity not found`. The examples below use
> `~/.osciris/<role>`. If you must use a temporary work-root, keep the seed file
> (below) safe and restore with the recovery step in section 1a.

Enterprise or bootstrap:

```bash
osciris-node identity generate \
  --node-id enterprise-1 \
  --role enterprise \
  --display-name "Enterprise 1" \
  --work-root ~/.osciris/enterprise
```

Provider:

```bash
osciris-node identity generate \
  --node-id provider-a \
  --role provider \
  --display-name "Provider A" \
  --work-root ~/.osciris/provider-a
```

Verifier:

```bash
osciris-node identity generate \
  --node-id verifier-1 \
  --role verifier \
  --display-name "Verifier 1" \
  --work-root ~/.osciris/verifier-1
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

## 1a. Recover an Identity from a Seed

The **seed is the durable secret** — the node id, public key, and `peer_id` are
all derived deterministically from it. The work-root store is a local cache that
can be rebuilt at any time from the seed. So if a work-root is wiped (for
example a `/tmp` work-root after a reboot) and `network serve` fails with:

```
Error: local node identity not found; run `osciris-node node join` first
```

restore the identity into a fresh work-root from the saved seed. Pass the **same
`--node-id`, `--role`, and `--display-name`** you used originally, plus the seed:

```bash
osciris-node identity generate \
  --node-id enterprise-1 \
  --role enterprise \
  --display-name "Enterprise 1" \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

Because the seed is reused, this reproduces the **exact same `peer_id`**, so any
`--bootstrap-peer` multiaddrs other operators already hold stay valid. (Omitting
the seed flag would mint a *new* random identity with a different `peer_id` that
no longer matches your seed file — always pass the seed when recovering.) You can
also read the seed inline with `--signing-key-seed-base64 "$(cat /run/osciris/enterprise.seed)"`.

Verify the restored `peer_id` matches what you recorded, then start the node as
usual. To sidestep this entirely, prefer a persistent `--work-root` such as
`~/.osciris/<role>` from the start.

## 2. Start the Bootstrap Presence Node

On the enterprise machine:

```bash
osciris-node network serve \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101
```

On startup the node logs its `local peer id` and, for each interface it binds, a
ready-to-share `bootstrap address` line, for example:

```
INFO osciris_node::network: local peer id: 12D3KooW...QgU5R
INFO osciris_node::network: bootstrap address (share with peers): /ip4/192.168.0.2/tcp/4101/p2p/12D3KooW...QgU5R
```

Give the other nodes the `bootstrap address` for your LAN or public IP (not the
`127.0.0.1` loopback line) as their `--bootstrap-peer`. Logs are written to
stderr, so this is visible by default; set `RUST_LOG=warn` to quiet it or
`RUST_LOG=debug` for more detail.

You can also print the `peer_id` without starting the node:

```bash
osciris-node network peer-id --signing-key-seed-base64 "$(cat /run/osciris/enterprise.seed)"
```

## 3. Start the Provider Node

On the provider machine:

```bash
osciris-node network run-provider \
  --work-root ~/.osciris/provider-a \
  --repo-root /absolute/path/to/OSCIRIS \
  --signing-key-id provider-a-key \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --trusted-assigner-public-key-base64 <enterprise-ed25519-public-key> \
  --listen-addr /ip4/0.0.0.0/tcp/4102 \
  --bootstrap-peer <bootstrap-multiaddr>
```

## 4. Start the Verifier Node

On the verifier machine:

```bash
osciris-node network run-verifier \
  --work-root ~/.osciris/verifier-1 \
  --verifier-id verifier-1 \
  --signing-key-id verifier-1-key \
  --signing-key-seed-file /run/osciris/verifier-1.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4103 \
  --bootstrap-peer <bootstrap-multiaddr>
```

## 5. Create and Announce a Job

Create a mock job spec on the enterprise machine:

```bash
osciris-node submit-job --output ~/.osciris/enterprise/job.json
```

Announce it:

```bash
osciris-node network create-job-announcement \
  --work-root ~/.osciris/enterprise \
  --job-spec ~/.osciris/enterprise/job.json \
  --submitter-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

## 6. Confirm Claims and Assign the Provider

Inspect claims:

```bash
osciris-node network claims --work-root ~/.osciris/enterprise

osciris-node network create-job-claim \
  --work-root ~/.osciris/provider-a \
  --job-id <job-id> \
  --provider-id provider-a \
  --signing-key-seed-file /run/osciris/provider-a.seed \
  --claim-note "matched gpu>=24gb"
```

Assign provider A:

```bash
osciris-node network assign-job \
  --work-root ~/.osciris/enterprise \
  --job-id <job-id> \
  --provider-id provider-a \
  --assigner-id enterprise-1 \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

## 7. Inspect Protocol State

Enterprise or observer:

```bash
osciris-node network job-status --work-root ~/.osciris/enterprise --job-id <job-id>
osciris-node network participant-status --work-root ~/.osciris/enterprise --job-id <job-id> --output ~/.osciris/participant-status.json
osciris-node network quorum-status --work-root ~/.osciris/enterprise --job-id <job-id>
osciris-node network settlement-status --work-root ~/.osciris/enterprise --job-id <job-id>
osciris-node network provider-status --work-root ~/.osciris/enterprise
```

Publish a milestone from the completed evidence bundle:

```bash
osciris-node network publish-milestone \
  --work-root ~/.osciris/enterprise \
  --job-id <job-id> \
  --title "Shared inference milestone" \
  --summary "Provider and verifier peers completed the first communal checkpoint." \
  --quality-metric-name quality_retention \
  --quality-metric-value 0.91 \
  --publisher-id enterprise-1 \
  --signing-key-id enterprise-key \
  --signing-key-seed-file /run/osciris/enterprise.seed

osciris-node network milestones --work-root ~/.osciris/enterprise --job-id <job-id>
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
