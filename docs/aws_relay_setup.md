# Setting Up the OSCIRIS Relay / Seed Node on AWS

This runbook stands up the public node behind `relay.oscirislabs.com` on a single
AWS EC2 instance. It is the AWS-specific companion to
[relay_node_setup.md](relay_node_setup.md); read that first for the conceptual
model (why `/dnsaddr`, what the seed protects, the trust boundary).

The node plays two roles:

1. **Seed node** — the default entry point every `osciris-node` dials on startup,
   so a fresh install joins with no configuration.
2. **Circuit relay** — NAT'd peers reserve a `/p2p-circuit` slot on it and
   hole-punch direct connections via DCUtR.

## Where To Run Each Step

Every section is tagged with **where** its commands run. This matters — running a
server command on your laptop is the most common mistake.

- 💻 **[LOCAL]** — on your own machine, using the `aws` CLI and `ssh`.
- ☁️ **[INSTANCE]** — on the EC2 relay itself, **after** you `ssh` into it (step 4).

On the EC2 instance the `ubuntu` user has **passwordless sudo**, so `sudo`
commands there never prompt. On your Mac, `sudo` asks for your Mac login password
— if you see that prompt while following an ☁️ [INSTANCE] step, you are on the
wrong machine. Steps 0–3 and 7 are 💻 [LOCAL]; steps 4–6 and 8 are ☁️ [INSTANCE].

## What Makes AWS Different

Two things bite specifically on AWS, and both are handled below:

- **The public IP is NATed onto a private interface.** The instance sees only its
  private `10.x`/`172.x` address on `eth0`; the public/Elastic IP lives at the VPC
  edge. libp2p therefore cannot auto-detect its public address, so
  `--external-address` is **mandatory** — without it the relay hands out empty
  reservations (`NoAddressesInReservation`) and is useless.
- **Two firewalls.** A Security Group at the VPC edge *and* (optionally) a host
  firewall. Both must allow **TCP and UDP** on the P2P port.

## 0. 💻 [LOCAL] Conventions (align with existing OSCIRIS AWS runs)

| Item        | Value                                                                                    |
| ----------- | ---------------------------------------------------------------------------------------- |
| Region      | `us-east-1`                                                                            |
| AWS profile | `osciris-benchmark`                                                                    |
| Base OS     | Ubuntu 22.04 LTS                                                                         |
| Instance    | `t3.small` to start (2 vCPU / 2 GB); `c7i.large` if you expect heavy relayed traffic |
| P2P port    | `4101` (TCP **and** UDP)                                                         |
| Tags        | `Project=OSCIRISProtocol`, `Owner=OSCIRISLabs`, `Role=relay`                       |

A relay forwards bytes; it does not train, so it needs no GPU. The cost driver is
**egress bandwidth** from connections that fail to hole-punch and keep relaying,
not compute.

```bash
export AWS_PROFILE=osciris-benchmark
export AWS_REGION=us-east-1
```

## 1. 💻 [LOCAL] Security Group

Allow SSH from your IP only, and the P2P port on both protocols from anywhere.

```bash
VPC_ID=$(aws ec2 describe-vpcs --filters Name=isDefault,Values=true \
  --query 'Vpcs[0].VpcId' --output text)

SG_ID=$(aws ec2 create-security-group \
  --group-name osciris-relay \
  --description "OSCIRIS relay/seed node" \
  --vpc-id "$VPC_ID" \
  --query GroupId --output text)

MYIP=$(curl -s https://checkip.amazonaws.com)

# SSH, your IP only
aws ec2 authorize-security-group-ingress --group-id "$SG_ID" \
  --ip-permissions IpProtocol=tcp,FromPort=22,ToPort=22,IpRanges="[{CidrIp=${MYIP}/32}]"

# P2P over TCP, from anywhere
aws ec2 authorize-security-group-ingress --group-id "$SG_ID" \
  --ip-permissions IpProtocol=tcp,FromPort=4101,ToPort=4101,IpRanges="[{CidrIp=0.0.0.0/0}]"

# P2P over UDP (QUIC + hole punching), from anywhere
aws ec2 authorize-security-group-ingress --group-id "$SG_ID" \
  --ip-permissions IpProtocol=udp,FromPort=4101,ToPort=4101,IpRanges="[{CidrIp=0.0.0.0/0}]"
```

> If you skip the **UDP** rule everything still connects over TCP, but QUIC is
> blocked and NAT hole punching degrades — NAT'd peers stay stuck relaying
> through you instead of upgrading to direct connections. Open UDP.

## 2. 💻 [LOCAL] Launch the Instance

Fetch the latest Ubuntu 22.04 AMI from SSM (do not hard-code a stale AMI):

```bash
AMI_ID=$(aws ssm get-parameter \
  --name /aws/service/canonical/ubuntu/server/22.04/stable/current/amd64/hvm/ebs-gp2/ami-id \
  --query 'Parameter.Value' --output text)

# Use an existing key pair, or create one:
aws ec2 create-key-pair --key-name osciris-relay \
  --query KeyMaterial --output text > ~/.ssh/osciris-relay.pem
chmod 600 ~/.ssh/osciris-relay.pem

INSTANCE_ID=$(aws ec2 run-instances \
  --image-id "$AMI_ID" \
  --instance-type t3.small \
  --key-name osciris-relay \
  --security-group-ids "$SG_ID" \
  --metadata-options "HttpTokens=required,HttpEndpoint=enabled" \
  --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":20,"VolumeType":"gp3"}}]' \
  --tag-specifications 'ResourceType=instance,Tags=[{Key=Project,Value=OSCIRISProtocol},{Key=Owner,Value=OSCIRISLabs},{Key=Role,Value=relay},{Key=Name,Value=osciris-relay-1}]' \
  --query 'Instances[0].InstanceId' --output text)

aws ec2 wait instance-running --instance-ids "$INSTANCE_ID"
```

`HttpTokens=required` enforces IMDSv2 (used in step 5 to read the public IP).

## 3. 💻 [LOCAL] Elastic IP (stable address)

The relay's address is published in DNS and cached by every node, so it must not
change when the instance stops/starts. Allocate and attach an Elastic IP.

```bash
ALLOC_ID=$(aws ec2 allocate-address --domain vpc --query AllocationId --output text)
aws ec2 associate-address --instance-id "$INSTANCE_ID" --allocation-id "$ALLOC_ID"

PUBLIC_IP=$(aws ec2 describe-addresses --allocation-ids "$ALLOC_ID" \
  --query 'Addresses[0].PublicIp' --output text)
echo "Relay public IP: $PUBLIC_IP"
```

## 4. 💻→☁️ [LOCAL: ssh in, then INSTANCE] Install OSCIRIS

```bash
ssh -i ~/.ssh/osciris-relay.pem ubuntu@"$PUBLIC_IP"

# on the instance:
sudo apt-get update && sudo apt-get install -y build-essential pkg-config git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone https://github.com/oscirisprotocol/core.git
cd core
cargo install --path crates/osciris-cli --force
osciris-node --version
```

A relay does **not** need `uv`, the DSP repo, or a GPU. Those are only for
provider nodes that execute workloads.

## 5. ☁️ [INSTANCE] Create the Relay Identity

Do this once. The `peer_id` it prints goes into DNS; the seed becomes critical
infrastructure.

```bash
osciris-node identity generate \
  --node-id osciris-relay-1 \
  --role enterprise \
  --display-name "OSCIRIS Relay 1" \
  --work-root ~/.osciris/relay
```

Store the seed on a **persistent** path (`/run` is tmpfs and cleared on reboot):

```bash
sudo install -d -m 700 /etc/osciris
umask 077
osciris-node network peer-id --signing-key-seed-base64 "<relay-seed>"   # sanity: matches printed peer_id
printf "%s" "<relay-seed>" | sudo tee /etc/osciris/relay.seed > /dev/null
sudo chmod 600 /etc/osciris/relay.seed
# The directory and file are root-owned; give them to the user that will RUN the node.
# For the foreground smoke test in step 6 that is `ubuntu`; step 8 re-chowns to `osciris`.
sudo chown -R ubuntu:ubuntu /etc/osciris
```

> The node reads the seed as whatever user runs it. If you see
> `failed to read /etc/osciris/relay.seed ... Permission denied (os error 13)`,
> the seed (or its directory) is owned by a different user — `chown` it to the
> user running `osciris-node` (`ubuntu` for the smoke test, `osciris` under
> systemd).

> **Back up the seed off the box.** The `peer_id` is derived from it and is published in DNS and cached by every node. If you lose it you must mint a new
> identity and update the DNS TXT record, and peers that cached the old peer id will reject the relay until they re-resolve. When you rebuild the instance, restore this seed and re-run `identity generate` with the same` --node-id`/`--role`/`--display-name` plus `--signing-key-seed-file` to reproduce the identical `peer_id` (see join guide §1a).

Determine the external address to advertise. If you are using a hostname
(recommended, step 7), use it:

```
--external-address /dns4/relay.oscirislabs.com/tcp/4101
```

If you have only the IP for now, read it from IMDSv2 rather than guessing (the
instance cannot see the Elastic IP on any local interface):

```bash
TOKEN=$(curl -sX PUT "http://169.254.169.254/latest/api/token" \
  -H "X-aws-ec2-metadata-token-ttl-seconds: 300")
curl -s -H "X-aws-ec2-metadata-token: $TOKEN" \
  http://169.254.169.254/latest/meta-data/public-ipv4
# -> use as: --external-address /ip4/<that-ip>/tcp/4101
```

## 6. ☁️ [INSTANCE] Run It (foreground smoke test first)

```bash
osciris-node network serve \
  --work-root ~/.osciris/relay \
  --signing-key-seed-file /etc/osciris/relay.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --relay-server \
  --external-address /ip4/${PUBLIC_IP}/tcp/4101
```

Expected startup:

```
local peer id: 12D3KooW...<relay-peer-id>
advertising external address /ip4/<public-ip>/tcp/4101
listening on /ip4/0.0.0.0/tcp/4101
listening on /ip4/0.0.0.0/udp/4101/quic-v1
```

Confirm `local peer id` matches what you will publish in DNS. A relay is itself a
seed node, so it does **not** dial the default seeds — that is applied for you, no
`--no-default-bootstrap` needed. Ctrl-C once you have seen the lines above.

## 7. 💻 [LOCAL] DNS (Route 53)

Two records point `relay.oscirislabs.com` at the box. The A record says where; the
`_dnsaddr` TXT record carries the multiaddr **including the peer id**, which is
what lets clients ship without a compiled-in peer id.

```bash
ZONE_ID=$(aws route53 list-hosted-zones-by-name --dns-name oscirislabs.com \
  --query 'HostedZones[0].Id' --output text | cut -d/ -f3)
RELAY_PEER_ID=<paste from step 5/6>

aws route53 change-resource-record-sets --hosted-zone-id "$ZONE_ID" \
  --change-batch '{
    "Changes": [
      { "Action": "UPSERT", "ResourceRecordSet": {
          "Name": "relay.oscirislabs.com", "Type": "A", "TTL": 300,
          "ResourceRecords": [{ "Value": "'"$PUBLIC_IP"'" }] } },
      { "Action": "UPSERT", "ResourceRecordSet": {
          "Name": "_dnsaddr.relay.oscirislabs.com", "Type": "TXT", "TTL": 300,
          "ResourceRecords": [{ "Value": "\"dnsaddr=/dns4/relay.oscirislabs.com/tcp/4101/p2p/'"$RELAY_PEER_ID"'\"" }] } }
    ]
  }'
```

Verify:

```bash
dig +short relay.oscirislabs.com A
dig +short _dnsaddr.relay.oscirislabs.com TXT
```

Once DNS is live, switch `--external-address` to the hostname so the addresses the
relay hands out survive an IP change:

```
--external-address /dns4/relay.oscirislabs.com/tcp/4101
```

To scale later, add **one TXT record per relay** at `_dnsaddr.relay.oscirislabs.com`.
Clients try them all — no client config changes.

## 8. ☁️ [INSTANCE] Run as a systemd Service

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
# let the service user read the seed
sudo chown -R osciris:osciris /etc/osciris
# install the binary to a standard system path (the osciris system user has no home dir)
sudo install -m 755 ~/.cargo/bin/osciris-node /usr/local/bin/osciris-node

# The service uses a DIFFERENT work-root than the step-5 smoke test, so it has no
# identity yet. Recreate it from the seed AS THE SERVICE USER — same seed reproduces
# the same peer id, so nothing in DNS changes.
sudo -u osciris /usr/local/bin/osciris-node identity generate \
  --node-id osciris-relay-1 \
  --role enterprise \
  --display-name "OSCIRIS Relay 1" \
  --work-root /var/lib/osciris/relay \
  --signing-key-seed-file /etc/osciris/relay.seed

sudo systemctl daemon-reload
sudo systemctl enable --now osciris-relay
sudo journalctl -u osciris-relay -f
```

> `local node identity not found` in the journal means the service's `--work-root`
> has no identity — usually because it differs from the work-root you ran
> `identity generate` against. The `sudo -u osciris ... identity generate` step
> above populates the service work-root from the seed and fixes it.

The node reconnects and re-reserves circuits automatically after a restart, so
`Restart=always` is safe.

## 9. 💻 [LOCAL / any other machine] Verify

From any laptop — no flags, because the relay is now the default seed:

```bash
osciris-node network serve \
  --work-root ~/.osciris/enterprise \
  --signing-key-seed-file /run/osciris/enterprise.seed \
  --listen-addr /ip4/0.0.0.0/tcp/4101
```

Expect:

```
connection established with 12D3KooW...<relay-peer-id>
relay reservation accepted by 12D3KooW...; now reachable over /p2p-circuit
```

Then confirm the protocol layer, not just the socket:

```bash
osciris-node network peers --work-root ~/.osciris/enterprise
```

The relay's `node_id` (`osciris-relay-1`) should be listed. To prove real NAT
traversal, bring up a second peer on a **different** network (phone hotspot) and
watch for `dcutr hole punch succeeded ... direct connection established`.

## 10. 💻 [LOCAL] Cost, Monitoring, Teardown

- `t3.small` on-demand in `us-east-1` is roughly a few USD/month; the Elastic IP
  is free **while attached to a running instance** and billed if left unattached.
- The real variable cost is **egress**: relayed (non-hole-punched) connections
  transit the instance. Watch `NetworkOut` in CloudWatch. If it climbs,
  investigate why hole punching is failing (usually a blocked UDP rule).
- Budget guardrail, matching existing runs: reuse or clone
  `OSCIRISProtocolMultiHost75`.

Teardown:

```bash
aws ec2 terminate-instances --instance-ids "$INSTANCE_ID"
aws ec2 wait instance-terminated --instance-ids "$INSTANCE_ID"
aws ec2 release-address --allocation-id "$ALLOC_ID"   # avoid idle-EIP charge
```

Keep the seed backup even after teardown if you intend to rebuild under the same
`peer_id`.

## Troubleshooting (AWS-specific)

**Clients connect but reservations fail with `NoAddressesInReservation`.**
The relay does not know its public address. On AWS this is expected without
`--external-address` — set it to `/dns4/relay.oscirislabs.com/tcp/4101` or the
Elastic IP.

**`Timeout has been reached` dialing the relay from outside.**
Security Group is not open. Confirm both the TCP and UDP ingress rules on 4101,
and that you attached `$SG_ID` to the instance. Test raw reachability:
`nc -z -v $PUBLIC_IP 4101`.

**Works over TCP, hole punching never succeeds.**
The UDP rule is missing, or a client-side network blocks UDP. Add the SG UDP rule;
peers on UDP-blocked networks fall back to relayed circuits (still functional).

**`local node identity not found` after a stop/start.**
The work-root was on ephemeral/instance storage, or the seed path was tmpfs.
Restore from the backed-up seed (join guide §1a) and keep the work-root on the EBS
volume (`/var/lib/osciris`).

**Relay's advertised address is a private `10.x` IP.**
It auto-confirmed a private interface. Always pass `--external-address` on AWS so
the public address wins.
