# Provider-Local Inference Round Trip

Status: implementation milestone

## User-visible outcome

A developer sends a prompt from their machine, OSCIRIS assigns the request to
an eligible peer, the peer runs the model locally, and the generated response
returns directly to the developer. OSCIRIS coordinates the exchange; it does
not centrally host the model or execute inference.

The first practical use case is a developer assistant for public code:

- explain a source file or error;
- generate unit-test suggestions;
- summarize an issue or pull request;
- answer documentation questions with supplied context.

Initial tests must use public or synthetic inputs. The selected provider sees
the plaintext prompt during execution. Encrypted transport does not make the
prompt confidential from the executing provider.

## Pinned inference profile

| Field | Value |
| --- | --- |
| Profile ID | `osciris-qwen3-4b-q4-v1` |
| Model | [`Qwen/Qwen3-4B-GGUF`](https://huggingface.co/Qwen/Qwen3-4B-GGUF) |
| Model revision | `bc640142c66e1fdd12af0bd68f40445458f3869b` |
| Artifact | `Qwen3-4B-Q4_K_M.gguf` |
| Artifact SHA-256 | `7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5` |
| Artifact size | 2,497,280,256 bytes |
| License | Apache-2.0 |
| Runtime | pinned [`llama.cpp`](https://github.com/ggml-org/llama.cpp) build with CUDA, HIP/ROCm, or Metal backend |
| Context limit | 8,192 tokens |
| Maximum generated tokens | 1,024 |
| Concurrency | one slot per provider for the first milestone |
| Default mode | non-thinking |

The profile intentionally caps context below the model maximum. One bounded
profile makes memory qualification, latency comparison, and receipt review
reproducible across heterogeneous providers.

## Provider envelope

| Provider | Counted minimum | Recommended |
| --- | --- | --- |
| NVIDIA CUDA | 8 GB physical VRAM and at least 6 GB free | 12 GB VRAM and 16 GB system RAM |
| AMD HIP/ROCm | 8 GB physical VRAM and at least 6 GB free | 12 GB VRAM and 16 GB system RAM |
| Apple Silicon Metal | 16 GB unified memory | 24 GB unified memory |
| Disk | 10 GB free | 20 GB free |

The 8K, one-slot estimate includes the 2.50 GB quantized model, approximately
1.1 GiB of BF16 KV cache, and runtime/compute headroom. The milestone benchmark
must replace estimates with observed peak memory and throughput for each
accepted backend.

## End-to-end path

```text
developer CLI
  -> signed inference request
  -> encrypted OSCIRIS peer transport
  -> capability and model-hash matching
  -> selected provider's local llama-server
  -> signed inference result
  -> encrypted OSCIRIS peer transport
  -> developer CLI output
  -> verifier receipts and quorum
  -> hash-only testnet anchor and public milestone status
```

The prompt and generated text return over the peer channel. They are not
published to GitHub, the public status bundle, or Horizen. Public records contain
request/result commitments, provider identity, model/profile commitment,
timings, verifier decisions, and anchor transaction references.

## What works after PR #16

The PR #16 implementation adds the interactive transport surface:

- `osciris-node inference serve`
- `osciris-node inference submit`
- `osciris-node inference wait`
- daemon/Desktop network start/stop controls
- Desktop `Test inference` prompt submission and signed response display

Interactive inference now returns a signed provider response and a
verifier-ready evidence bundle. The provider writes `job_spec.json`,
`inference_request.json`, `inference_response.json`, `execution_receipt.json`,
`receipt_bundle.json`, `bundle_index.json`, and
`python-output/inference_economics.json` under `.osciris/evidence/<request-id>`.
The requester unpacks and validates the provider bundle locally and records the
execution receipt and bundle hashes.

Local proof already covers:

- signed request/response verification;
- deterministic runtime response signing;
- `llama-cpp` `/completion` adapter response signing;
- requester-side evidence materialization; and
- successful local `osciris-verifier` review of interactive evidence.

## What works in the current beta

The current CLI already provides:

- signed identities and provider capability publication;
- job announcements, claims, and assignments;
- batch `inference_economics` provider execution;
- signed execution and verification receipts;
- P2P receipt-bundle retrieval;
- configurable verifier counts and quorum status;
- participant snapshots and milestone publication;
- hash and receipt anchoring through the Horizen integration path.

These primitives prove asynchronous accountable jobs. They do not provide
interactive prompt/result transport or a local model-server supervisor.

## Commands introduced by this milestone

PR #16 implements the base interactive command surface. The readiness command
exists now, and pinned-profile install plus managed local runtime startup are
partially implemented.

```bash
# Install and verify the pinned profile from a locally staged GGUF artifact.
osciris-node inference profile install \
  --work-root ~/.osciris \
  --profile osciris-qwen3-4b-q4-v1 \
  --source-model-path /models/Qwen3-4B-Q4_K_M.gguf

# Provider: expose an already-running deterministic or llama.cpp-compatible runtime.
osciris-node inference serve \
  --work-root /var/lib/osciris \
  --provider-id provider-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --runtime llama-cpp \
  --llama-cpp-endpoint http://127.0.0.1:8080 \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --bootstrap-peer <bootstrap-multiaddr>

# Provider: alternatively let OSCIRIS launch a managed local llama-server.
osciris-node inference serve \
  --work-root /var/lib/osciris \
  --provider-id provider-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --runtime llama-cpp-managed \
  --llama-server-path /usr/local/bin/llama-server \
  --model-path /var/lib/osciris/.osciris/profiles/osciris-qwen3-4b-q4-v1/Qwen3-4B-Q4_K_M.gguf \
  --managed-llama-host 127.0.0.1 \
  --managed-llama-port 8080 \
  --managed-llama-ctx-size 8192 \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --bootstrap-peer <bootstrap-multiaddr>

# Developer: submit a bounded request.
osciris-node inference submit \
  --work-root ~/.osciris \
  --signing-key-seed-base64 <developer-seed> \
  --requester-id developer-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --prompt-file ./prompt.txt \
  --max-output-tokens 512 \
  --provider-peer-id <provider-peer-id> \
  --bootstrap-peer <provider-multiaddr> \
  --output ./response.json

# Developer: wait for the signed result and reviewed receipt status.
osciris-node inference wait \
  --work-root ~/.osciris \
  --signing-key-seed-base64 <developer-seed> \
  --request-json ./request.json \
  --provider-peer-id <provider-peer-id> \
  --bootstrap-peer <provider-multiaddr> \
  --output ./response.json \
  --timeout-seconds 180

# Inspect capacity and verification gaps.
osciris-node inference readiness \
  --work-root ~/.osciris \
  --profile osciris-qwen3-4b-q4-v1
```

Until those commands are released, use
[the MVP operator runbook](../mvp_operator_runbook.md) for the existing
asynchronous inference-economics and receipt flow.

## Capability and routing requirements

The provider declaration must add:

- model profile ID, revision, artifact SHA-256, and license;
- inference backend and backend version;
- accelerator backend: CUDA, HIP/ROCm, Metal, Vulkan, or CPU;
- physical and currently free accelerator memory;
- maximum context, available inference slots, and measured tokens per second;
- health-check timestamp and profile benchmark commitment.

Routing must enforce model artifact hash, backend compatibility, free memory,
context, available slots, supported job type, and recent health. The current
generic `gpu>=Ngb` matcher is insufficient for interactive inference.

## Readiness and quorum gaps

Service capacity and verification quorum are separate:

| Measure | Initial target |
| --- | ---: |
| Healthy compatible providers | 3 |
| Fallback compatible providers | 1 |
| Total provider target | 4 |
| Available inference slots | 3 |
| Independent verifiers per completed request | 2 |

The public profile status must expose:

```text
provider_gap = max(0, 4 - healthy_compatible_providers)
slot_gap = max(0, 3 - available_slots)
verifier_gap = max(0, 2 - online_independent_verifiers)
```

It should also show provider counts by CUDA, HIP/ROCm, Metal, and CPU without
making backend diversity a hard readiness requirement.

One provider executes each request. Provider count is an availability target,
not a requirement to execute the same prompt four times. Two independent
verifiers review the result receipt and profile-policy checks.

## Practical multi-host run

The acceptance run uses at least:

1. one developer machine;
2. one remote provider machine running the pinned model locally;
3. two independent verifier identities, with at least one on a different host
   from the provider.

Test prompt:

```text
/no_think
Explain what the supplied public function does, identify one edge case, and
return a minimal pytest test for that edge case.
```

The developer attaches one public Python function as bounded context. The
returned JSON must include:

- request ID and request commitment;
- response text and result commitment;
- selected provider ID;
- exact model profile and artifact SHA-256;
- input/output token counts and latency;
- signed execution receipt;
- two accepted verifier receipt references;
- quorum state and optional Horizen testnet anchor.

## Current multi-host acceptance commands

The exact remote run should use public or synthetic prompt content only.

On the provider host, either start a local llama.cpp-compatible server first:

```bash
llama-server \
  --model /models/Qwen3-4B-Q4_K_M.gguf \
  --host 127.0.0.1 \
  --port 8080 \
  --ctx-size 8192
```

Then expose that provider over OSCIRIS peer transport:

```bash
osciris-node inference serve \
  --work-root /var/lib/osciris-provider \
  --signing-key-seed-base64 <provider-seed> \
  --provider-id provider-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --runtime llama-cpp \
  --llama-cpp-endpoint http://127.0.0.1:8080 \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --run-seconds 600
```

Or use the managed runtime path after installing the pinned artifact:

```bash
osciris-node inference profile install \
  --work-root /var/lib/osciris-provider \
  --profile osciris-qwen3-4b-q4-v1 \
  --source-model-path /models/Qwen3-4B-Q4_K_M.gguf

osciris-node inference serve \
  --work-root /var/lib/osciris-provider \
  --signing-key-seed-base64 <provider-seed> \
  --provider-id provider-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --runtime llama-cpp-managed \
  --llama-server-path /usr/local/bin/llama-server \
  --model-path /var/lib/osciris-provider/.osciris/profiles/osciris-qwen3-4b-q4-v1/Qwen3-4B-Q4_K_M.gguf \
  --managed-llama-host 127.0.0.1 \
  --managed-llama-port 8080 \
  --managed-llama-ctx-size 8192 \
  --listen-addr /ip4/0.0.0.0/tcp/4101 \
  --run-seconds 600
```

On the developer host, submit the prompt to the provider peer:

```bash
osciris-node inference submit \
  --work-root ~/.osciris-dev \
  --signing-key-seed-base64 <developer-seed> \
  --requester-id developer-1 \
  --profile-id osciris-qwen3-4b-q4-v1 \
  --prompt-file ./prompt.txt \
  --max-output-tokens 512 \
  --provider-peer-id <provider-peer-id> \
  --bootstrap-peer /ip4/<provider-ip>/tcp/4101/p2p/<provider-peer-id> \
  --timeout-seconds 180 \
  --output ./response.json
```

The response JSON includes `evidence_dir`, `execution_receipt_sha256`, and
`bundle_sha256`. Each verifier can review the materialized bundle:

```bash
osciris-node verify \
  --evidence-dir <evidence_dir_from_response> \
  --provider-public-key-base64 <provider-ed25519-public-key-base64> \
  --verifier-id verifier-1 \
  --signing-key-id verifier-1-key \
  --signing-key-seed-base64 <verifier-1-seed>
```

Run the same command with `verifier-2`. Acceptance requires the updated
`receipt_bundle.json` to include two accepted verification receipt hashes.

## Acceptance criteria

- The request originates on the developer machine and reaches a remote peer.
- The selected peer serves the pinned artifact from local storage.
- No central OSCIRIS inference server receives or executes the prompt.
- The response returns to the originating developer over the peer protocol.
- Provider signature, model hash, request commitment, and result commitment
  verify locally.
- Two distinct verifier identities accept the receipt.
- Removing the provider updates the profile's provider and slot gaps.
- Public status and testnet data contain no prompt or generated response.
- A sanitized evidence bundle records commands, versions, timings, peak memory,
  token throughput, receipts, quorum status, and anchor transaction.
- The reviewed evidence bundle is linked from a signed OSCIRIS milestone
  record.

## Completion evidence

Publish only:

- this runbook revision;
- sanitized machine and runtime descriptors;
- model/profile commitment;
- request and result hashes;
- benchmark summary;
- execution and verifier receipt hashes;
- readiness-gap before/after snapshots;
- Horizen testnet transaction link;
- evidence bundle SHA-256.

Do not publish prompts, generated responses, signing seeds, peer private keys,
IP addresses, access tokens, or unredacted logs.
