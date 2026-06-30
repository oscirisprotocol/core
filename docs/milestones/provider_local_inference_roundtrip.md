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

## What works in v0.1.1

The current CLI already provides:

- signed identities and provider capability publication;
- job announcements, claims, and assignments;
- batch `inference_economics` provider execution;
- signed execution and verification receipts;
- P2P receipt-bundle retrieval;
- configurable verifier counts and quorum status;
- participant snapshots and milestone publication;
- hash and receipt anchoring through the Horizen integration path.

These primitives prove asynchronous accountable jobs. They do not yet provide
interactive prompt/result transport or a local model-server supervisor.

## Commands introduced by this milestone

The following command surface is the implementation target and is **not
available in v0.1.1**:

```bash
# Provider: install and verify the pinned profile.
osciris-node inference profile install \
  --profile osciris-qwen3-4b-q4-v1

# Provider: expose the local runtime only through the OSCIRIS peer.
osciris-node inference serve \
  --work-root /var/lib/osciris \
  --profile osciris-qwen3-4b-q4-v1 \
  --backend auto \
  --slots 1 \
  --bootstrap-peer <bootstrap-multiaddr>

# Developer: submit a bounded request.
osciris-node inference submit \
  --work-root ~/.osciris \
  --profile osciris-qwen3-4b-q4-v1 \
  --prompt-file ./prompt.txt \
  --max-output-tokens 512 \
  --required-verifier-count 2 \
  --output ./request.json

# Developer: wait for the signed result and reviewed receipt status.
osciris-node inference wait \
  --work-root ~/.osciris \
  --request-file ./request.json \
  --response-output ./response.json \
  --timeout-seconds 180

# Any participant: inspect capacity and verification gaps.
osciris-node inference readiness \
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
