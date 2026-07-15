# OSCIRIS Relay / Seed Node Setup

This runbook stands up the public node behind `relay.oscirislabs.com`.

That node does two jobs:

1. **Seed node** — the entry point every fresh `osciris-node` dials automatically,
   so a new contributor joins the network with no configuration at all.
2. **Circuit relay** — NAT'd peers (home WiFi, mobile) reserve a `/p2p-circuit`
   slot on it so other peers can reach them, then hole-punch a direct connection
   via DCUtR.

Clients ship with `/dnsaddr/relay.oscirislabs.com` as their default bootstrap
peer. They discover the relay's **peer id from DNS**, not from compiled-in
config, which means you can rebuild, move, or scale out the relay by editing DNS
alone — no client ever changes its configuration.

## 1. Host Requirements

- A box with a **public IP** (any small VPS is fine — 1 vCPU / 1 GB is plenty;
  the relay forwards bytes, it does not train).
- Ports open on **both TCP and UDP** for the listen port (`4101` below). QUIC runs
  over UDP and is what makes hole punching succeed; TCP-only still works but
  degrades NAT traversal.
- Rust toolchain to build, or copy a prebuilt binary.

```bash
# Ubuntu example
sudo ufw allow 4101/tcp
sudo ufw allow 4101/udp
```

If you are on AWS/GCP, open the same ports in the security group, and note the
`--external-address` caveat in step 4.

## 2. Install

```bash
git clone https://github.com/oscirisprotocol/core.git
cd core
cargo install --path crates/osciris-cli --force
osciris-node --version
```

## 3. Create the Relay Identity (do this once, then guard the seed)

```bash
osciris-node identity generate \
  --node-id osciris-relay-1 \
  --role enterprise \
  --display-name "OSCIRIS Relay 1" \
  --work-root ~/.osciris/relay
```

Record the `peer_id` — you will publish it in DNS — and store the seed:

```bash
sudo install -m 700 -d /run/osciris
printf "%s" "<relay-seed>" | sudo tee /run/osciris/relay.seed > /dev/null
sudo chmod 600 /run/osciris/relay.seed
```

> **The relay seed is critical infrastructure.** The `peer_id` is derived from it
> and is published in DNS and cached by every node. Back the seed up somewhere
> safe and offline. If you lose it you must generate a new identity and update
> the DNS TXT record; peers that cached the old peer id will fail to verify the
> relay until they re-resolve.
>
> When you rebuild the host, **restore the seed** and re-run `identity generate`
> with the same `--node-id` / `--role` / `--display-name` plus
> `--signing-key-seed-file`. That reproduces the identical `peer_id`, so nothing
> downstream changes. See §1a of the multi-host join guide.

Note `/run` is a tmpfs on most Linux distros and is cleared on reboot. For a
long-lived relay, keep the seed somewhere persistent with tight permissions
(for example `/etc/osciris/relay.seed`, mode `600`, owned by the service user)
and point `--signing-key-seed-file` at that.

## 4. DNS Records

Two records. Replace `203.0.113.7` with your public IP and `12D3KooW...` with
the relay's `peer_id`.

**A record** — where the relay lives:

```
relay.oscirislabs.com.        A      203.0.113.7
```

**TXT record** — what `/dnsaddr` resolves. This is what carries the peer id to
clients:

```
_dnsaddr.relay.oscirislabs.com.  TXT  "dnsaddr=/dns4/relay.oscirislabs.com/tcp/4101/p2p/12D3KooW...<relay-peer-id>"
```

The `_dnsaddr.` prefix and the `dnsaddr=` value prefix are both required — that
is the libp2p convention.

To run more than one seed node later, add **one TXT record per node** at the same
name. Clients try them all. That is how you scale or rotate seeds without
touching a single client.

Verify:

```bash
dig +short relay.oscirislabs.com A
dig +short _dnsaddr.relay.oscirislabs.com TXT
```

## 5. Run the Relay

`--relay-server` turns on circuit hosting. `--external-address` tells the relay
the address to hand out in reservations — **without it, clients fail with
`NoAddressesInReservation` and the relay is useless.**

```bash
osciris-node network serve \
  --work-root ~/.osciris/relay \
  --signing-key-seed-file /etc/osciris/relay.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --relay-server \
  --external-address /dns4/relay.oscirislabs.com/tcp/4101
```

Notes:

- `--listen-addr` takes the **TCP** multiaddr; the QUIC listener
  (`/udp/4101/quic-v1`) is derived automatically. Do not pass a QUIC multiaddr.
- A relay does **not** dial the default seed nodes (it *is* one), so
  `--no-default-bootstrap` is unnecessary — it is applied for you.
- On a cloud host whose public IP is NATed onto a private interface (AWS elastic
  IP, GCP external IP), `--external-address` is **mandatory**, because the box
  cannot see its public IP on any local interface.

Expected startup output:

```
local peer id: 12D3KooW...<relay-peer-id>
advertising external address /dns4/relay.oscirislabs.com/tcp/4101
listening on /ip4/0.0.0.0/tcp/4101
listening on /ip4/0.0.0.0/udp/4101/quic-v1
```

Confirm the `local peer id` matches the one in your TXT record.

## 6. Run It As A Service

`/etc/systemd/system/osciris-relay.service`:

```ini
[Unit]
Description=OSCIRIS Relay / Seed Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=osciris
Environment=RUST_LOG=warn,osciris_node=info
ExecStart=/usr/local/bin/osciris-node network serve \
  --work-root /var/lib/osciris/relay \
  --signing-key-seed-file /etc/osciris/relay.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --relay-server \
  --external-address /dns4/relay.oscirislabs.com/tcp/4101
Restart=always
RestartSec=5
# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/osciris

[Install]
WantedBy=multi-user.target
```

```bash
sudo useradd -r -s /usr/sbin/nologin osciris
sudo install -d -o osciris -g osciris /var/lib/osciris/relay
sudo install -m 755 ~/.cargo/bin/osciris-node /usr/local/bin/osciris-node
sudo chown -R osciris:osciris /etc/osciris   # let the service user read the seed
sudo systemctl daemon-reload
sudo systemctl enable --now osciris-relay
sudo journalctl -u osciris-relay -f
```

The node reconnects and re-reserves circuits automatically after a restart, so
`Restart=always` is safe.

## 7. Verify From Another Machine

From any other host — no flags, since the seed is the default:

```bash
osciris-node network serve \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101
```

You should see it resolve the seed, connect, and (if it is behind NAT) reserve a
circuit:

```
connection established with 12D3KooW...<relay-peer-id>
relay reservation accepted by 12D3KooW...; now reachable over /p2p-circuit
listening on /dns4/relay.oscirislabs.com/tcp/4101/p2p/<relay>/p2p-circuit/p2p/<self>
```

Then confirm the protocol layer, not just the socket:

```bash
osciris-node network peers --work-root ~/.osciris/enterprise
```

The relay's `node_id` should be listed.

To prove NAT traversal properly, bring up a **second** NAT'd peer on a different
network (a phone hotspot works) and watch for:

```
outbound relayed circuit established via <relay>
dcutr hole punch succeeded with <peer>: direct connection established
```

If the hole punch fails, the peers keep working over the relayed circuit — the
relay stays in the data path, which costs you bandwidth but does not break
connectivity.

## 8. Operating Notes

- **Bandwidth**: every relayed connection that fails to hole-punch transits your
  relay. Watch egress. Successful DCUtR upgrades drop the relay out of the path,
  which is why UDP must be open.
- **The relay sees traffic it forwards.** Circuit relay is not an anonymity layer;
  it forwards an encrypted stream, but it knows who is talking to whom. Do not
  describe it as private.
- **The relay is a trust anchor for discovery, not for the protocol.** It cannot
  forge presence, claims, receipts, or assignments — those are Ed25519-signed and
  verified by each node. A malicious relay can withhold or delay traffic, not
  fabricate it.
- **Losing the relay does not partition an established LAN.** Peers already
  connected stay connected, and mDNS discovery on a LAN does not need it. New
  internet peers cannot find each other without a seed, so keep it up.

## 9. Opting Out

For isolated, offline, or CI networks that must not touch the public seed:

```bash
osciris-node network serve ... --no-default-bootstrap
```

Available on `network serve`, `network run-provider`, and `network run-verifier`.
