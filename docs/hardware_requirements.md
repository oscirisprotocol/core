# Hardware Requirements

OSCIRIS records signed provider capability. Joining the developer beta does not
require a GPU. A provider should claim only hardware and runtimes that are
present and tested on that host.

## Participation requirements

| Role | Current requirement | GPU required |
| --- | --- | --- |
| CLI, enterprise client, relay, or proof review | supported macOS, Linux, or Windows binary | No |
| Verifier | supported binary, Python 3, `uv`, and enough local storage for reviewed evidence | No |
| Current GPU worker | Linux x86_64, NVIDIA CUDA, 24 GB VRAM, Python 3, and `uv` | Yes |

Provider signing keys must use local protected storage. Operators must also
maintain supported drivers, outbound HTTPS access, accurate capability
metadata, and enough disk space for each assigned model and evidence bundle.

No CPU-core, system-RAM, or disk minimum has been benchmarked across all
control-plane roles. For the current 7B GPU beta, use 8 CPU cores, 32 GB system
RAM, and a 160 GB SSD as the operational baseline provisioned for the completed
A10G adaptation runs.

## Evidence-backed GPU tiers

| Workload tier | Tested accelerator | What is established |
| --- | --- | --- |
| Bounded 3B inference | NVIDIA A10G, 24 GB VRAM | completed causal-LM inference benchmark |
| Bounded 7B QLoRA | NVIDIA A10G, 24 GB VRAM | completed Qwen and Mistral adaptation benchmarks |
| Bounded 7B inference | NVIDIA L40S, 48 GB VRAM | completed Qwen and Mistral causal-LM inference benchmarks |

The public minimum for a current GPU worker is therefore 24 GB VRAM. This is a
conservative evidence-backed floor, not a claim that smaller models cannot run
on smaller GPUs. A 16 GB floor for the measured 7B QLoRA configuration is
technically plausible because observed peak allocation stayed below that
capacity, but OSCIRIS has not completed the same workload on a 16 GB device.

Lower-VRAM hosts may install the CLI and publish capability. They should not be
promised GPU jobs until OSCIRIS tests and publishes job profiles for those
devices.

## Matching boundary

Each announcement carries a capability requirement such as `gpu>=24gb`; the
current matcher checks job type and declared VRAM. It does not yet enforce
CUDA/MPS availability, runtime packages, GPU architecture, system RAM, free
disk, load, or hardware attestation.

Until those checks are implemented, operators must validate the complete
runtime locally and assign only workloads that fit the provider.

## Platform support

- Linux x86_64 with NVIDIA CUDA is the validated GPU execution path.
- Windows x86_64 NVIDIA binaries are published for beta smoke testing; Windows
  GPU execution is not yet a production-readiness claim.
- macOS Apple Silicon can run the CLI and control-plane roles. MPS/MLX GPU
  workload minimums are not yet established by published OSCIRIS benchmarks.
- AMD ROCm is not a published beta execution target yet.

## Capability publication

Before accepting work, publish the host's real capability:

```bash
osciris-node network create-provider-capability \
  --provider-id <provider-id> \
  --gpu-model "NVIDIA RTX 4090" \
  --gpu-count 1 \
  --vram-gb 24 \
  --cuda-available \
  --supported-job-type inference_economics \
  --supported-runtime python3 \
  --supported-runtime cuda
```

Do not round VRAM upward or declare runtimes that have not passed a local smoke
test. Signed capability metadata is checked against execution receipts, but it
is not hardware attestation.
