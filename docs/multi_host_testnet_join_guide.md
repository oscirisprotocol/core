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
- `uv` (only needed to execute real workloads on a provider)
- network reachability between participating machines

Install the CLI. Use `--force` when upgrading — every machine in the network
must run the same version, and older builds lack the QUIC and relay transports:

```bash
cargo install --path crates/osciris-cli --force
```

Smoke check:

```bash
osciris-node --version
osciris-node doctor --repo-root /absolute/path/to/OSCIRIS
```

`doctor` reports `work_root_writable`, `protocol_store_ready`, and whether
`python3` / `uv` / `forge` are present. Missing `uv` only blocks real workload
execution, not joining the network.

### Firewall

Nodes listen on **TCP and UDP** on the same port number (QUIC runs over UDP and
is what makes NAT hole punching work). Open both.

## Suggested Ports

- bootstrap / enterprise / relay node: `4101`
- provider node: `4102`
- verifier node: `4103`

## 1. Generate Identities

Run this once per node.

> **Use a persistent `--work-root`, not `/tmp`.** The work-root holds the node's
> local protocol store, including its saved identity. macOS clears `/tmp` on
> reboot, so a node whose work-root lives under `/tmp` will lose its identity and
> fail to start with `local node identity not found`. The examples below use
> `~/.osciris/<role>`.

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

Each node needs its **own** identity and seed. Never reuse one seed on two
nodes: the seed *is* the identity, so both would share a `peer_id` and conflict.

Store the seed in a private file and pass `--signing-key-seed-file`. Avoid
putting seed values directly in shell history or process arguments.

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
can be rebuilt from the seed at any time. If a work-root is wiped (for example a
`/tmp` work-root after a reboot), `network serve` fails with:

```
Error: local node identity not found; run `osciris-node node join` first
```

Restore the identity into a fresh work-root from the saved seed. Pass the **same
`--node-id`, `--role`, and `--display-name`** you used originally, plus the seed:

```bash
osciris-node identity generate \
  --node-id enterprise-1 \
  --role enterprise \
  --display-name "Enterprise 1" \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed
```

Because the seed is reused this reproduces the **exact same `peer_id`**, so any
`--bootstrap-peer` multiaddrs other operators already hold stay valid. Omitting
the seed flag would mint a *new* random identity with a different `peer_id` that
no longer matches your seed file — always pass the seed when recovering.

## 2. Start the Bootstrap Node

On the enterprise machine:

```bash
osciris-node network serve \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101
```

This process **runs until you stop it** (Ctrl-C). That is correct for a
bootstrap node — leave it in its own terminal. On startup it logs its peer id
and, for each interface it binds, a ready-to-share bootstrap address:

```
INFO osciris_node::network: local peer id: 12D3KooW...QgU5R
INFO osciris_node::network: listening on /ip4/192.168.0.2/tcp/4101
INFO osciris_node::network: bootstrap address (share with peers): /ip4/192.168.0.2/tcp/4101/p2p/12D3KooW...QgU5R
```

Give other nodes the address for your **LAN or public IP** — not the
`127.0.0.1` loopback line. Logs go to stderr and are on by default; use
`RUST_LOG=warn` to quiet them or `RUST_LOG=debug` for detail. JSON output from
data commands goes to stdout, so it is safe to pipe into `jq`.

To print the `peer_id` without starting the node:

```bash
osciris-node network peer-id --signing-key-seed-base64 "$(cat /run/osciris/enterprise.seed)"
```

## 2a. Connecting Peers

### Same LAN — automatic

Nodes on the same network discover each other over mDNS with **no
`--bootstrap-peer` at all**. Just start them; you will see:

```
mdns discovered peer 12D3KooW...
connection established with 12D3KooW...
peer 12D3KooW... subscribed to osciris/network/presence
```

### Explicit bootstrap — any reachable peer

Point a node at another node's bootstrap address:

```bash
osciris-node network serve \
  --work-root ~/.osciris/verifier-1 \
  --signing-key-seed-file /run/osciris/verifier-1.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4103 \
  --bootstrap-peer /ip4/192.168.0.2/tcp/4101/p2p/12D3KooW...QgU5R
```

### Across the internet — relay + hole punching

Most home and mobile connections sit behind NAT/CGNAT and **cannot accept
inbound connections**, so a peer there can never be dialed directly. At least
one node must be publicly reachable.

Run a **relay** on a public host (VPS, or a port-forwarded machine) with TCP and
UDP open:

```bash
osciris-node network serve \
  --work-root ~/.osciris/relay \
  --signing-key-seed-file /run/osciris/relay.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --relay-server
```

If the host cannot see its own public IP on a local interface (AWS elastic IP
and most cloud NAT setups), you **must** declare it, or the relay's reservations
carry no addresses and clients fail with `NoAddressesInReservation`:

```bash
  --external-address /ip4/203.0.113.7/tcp/4101
```

Every other node simply uses the relay as its `--bootstrap-peer`:

```bash
  --bootstrap-peer /ip4/203.0.113.7/tcp/4101/p2p/<relay-peer-id>
```

### Publishing the relay under a domain name

Hard-coding a bare IP means every node's config breaks when you replace the box.
Instead point a hostname at the relay (`A` record, e.g.
`relay.oscirislabs.com -> 203.0.113.7`) and use a `/dns4/` multiaddr:

```bash
  --bootstrap-peer /dns4/relay.oscirislabs.com/tcp/4101/p2p/<relay-peer-id>
```

Run the relay itself with the hostname as its external address, so the addresses
it hands out in circuit reservations are also stable:

```bash
osciris-node network serve \
  --work-root ~/.osciris/relay \
  --signing-key-seed-file /run/osciris/relay.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --relay-server \
  --external-address /dns4/relay.oscirislabs.com/tcp/4101
```

The `peer_id` is still required and is **not** replaced by DNS: the hostname says
*where* to connect, the peer id proves *who* answered. Keep the relay's seed safe
— it is what preserves the peer id across host rebuilds. If you lose the seed,
every node's `--bootstrap-peer` must be updated with the new peer id, so back it
up and restore it with the recovery flow in section 1a.

Use `/dns6/` for AAAA records. `/dnsaddr/` is also supported, which resolves
`TXT` records at `_dnsaddr.<host>` containing full multiaddrs — useful later for
publishing a rotating set of seed nodes under one name without changing client
config.

A NAT'd node detects it is not publicly reachable, reserves a `/p2p-circuit`
slot on the relay, and logs:

```
relay reservation accepted by <relay>; now reachable over /p2p-circuit
listening on /ip4/203.0.113.7/tcp/4101/p2p/<relay>/p2p-circuit/p2p/<self>
```

That circuit address is how other peers reach it. When two NAT'd peers meet
through the relay, DCUtR hole-punches a **direct** connection so traffic stops
flowing through the relay:

```
outbound relayed circuit established via <relay>
dcutr hole punch succeeded with <peer>: direct connection established
```

If a hole punch fails (some symmetric NATs defeat it), the peers keep working
over the relayed circuit — connectivity degrades, it does not break.

## 2b. Verify Peers Are Connected

The logs show transport-level connections. To confirm **signed presence** was
actually exchanged and stored, run on each node:

```bash
osciris-node network peers --work-root ~/.osciris/enterprise
```

Each node should list the other's `node_id`. This is the authoritative check
that the protocol layer — not just the socket — is working.

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

Confirm it propagated over gossipsub — on **another** node:

```bash
osciris-node network jobs --work-root ~/.osciris/verifier-1
```

The announcement should appear there without you copying any file.

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

## 8. One-Command End-to-End Check

To verify the whole off-chain lifecycle on a single machine without any
networking or GPU setup:

```bash
osciris-node demo local-settlement --work-root ~/.osciris/demo
```

It runs enterprise → provider → verifier → settlement and should report:

```json
{
  "provider_a_executed": true,
  "provider_b_executed": false,
  "quorum_status": "Accepted",
  "settlement_ready": true,
  "lifecycle_state": "SettlementReady"
}
```

Use this to isolate faults: if the demo passes but a multi-host run does not,
the problem is networking, not the protocol.

## Verification Checklist

| # | Check | Command | Pass signal |
|---|-------|---------|-------------|
| 1 | CLI installed | `osciris-node --version` | prints version; same on every host |
| 2 | Environment sane | `osciris-node doctor --repo-root <repo>` | `work_root_writable`, `protocol_store_ready` true |
| 3 | Identity created | `osciris-node identity generate ...` | JSON with `peer_id`, seed |
| 4 | Node serves | `osciris-node network serve ...` | `listening on ...` + `local peer id:` |
| 5 | LAN auto-discovery | start 2 nodes, no `--bootstrap-peer` | `mdns discovered peer ...` |
| 6 | Peers connect | any two nodes | `connection established with ...` |
| 7 | Gossip mesh formed | either node's log | `subscribed to osciris/network/presence` (7 topics) |
| 8 | Presence stored | `network peers --work-root ...` | each lists the other's `node_id` |
| 9 | Job propagates | announce on A, `network jobs` on B | announcement appears on B |
| 10 | NAT traversal | relay + 2 NAT'd peers | `relay reservation accepted`, `dcutr hole punch succeeded` |
| 11 | Full lifecycle | `demo local-settlement` | `settlement_ready: true` |

## Troubleshooting

**`network serve` prints nothing and seems to hang.**
It is working. `serve` runs until Ctrl-C and only prints a summary on exit. Logs
are on by default in current builds; if you see nothing, you are on an old build
— reinstall with `--force`, or run with `RUST_LOG=info`.

**`Error: local node identity not found; run osciris-node node join first`.**
The work-root has no identity — usually a `/tmp` work-root wiped by a reboot.
Restore it from your seed: see section 1a.

**`Multiaddr is not supported: /ip4/.../udp/.../quic-v1/...` or `.../p2p-circuit/...`.**
Your binary is older than its peers and lacks the QUIC/relay transports. Upgrade
every host: `cargo install --path crates/osciris-cli --force`.

**`Timeout has been reached` dialing a peer.**
That peer is not reachable — almost always NAT/CGNAT or a closed port. Confirm
with `nc -z -v <ip> <port>`. Fix by making the peer publicly reachable, or route
through a relay (section 2a). No client-side setting can dial an unreachable host.

**`Connection refused`.**
Nothing is listening on that address/port — the peer is stopped, or you have the
wrong port. Stale mDNS entries for exited nodes also produce this briefly; it is
harmless.

**`kademlia bootstrap ... No known peers`.**
The node has no peers yet — expected when it was started with no
`--bootstrap-peer` and no LAN peers are present. Give it a bootstrap peer.

**`NoAddressesInReservation` when reserving on a relay.**
The relay does not know its own external address. Start it with
`--external-address /ip4/<public-ip>/tcp/<port>`.

## Current Boundary

This flow currently proves:

- multi-host off-chain presence and state sync
- LAN peer auto-discovery (mDNS) and DHT-based discovery (Kademlia)
- NAT traversal via circuit relay and DCUtR hole punching
- provider claim and enterprise assignment
- provider execution receipt generation
- verifier receipt generation
- quorum and settlement-ready lifecycle

This guide does not yet provide:

- public bootstrap infrastructure (you must run your own relay/seed node)
- guaranteed hole-punch success (symmetric NATs may fall back to relayed circuits)
- long-running / resumable training jobs
- correctness verification of the work itself (receipts prove provenance, not that
  the computation was performed honestly)
- mainnet deployment
- production key custody policy
