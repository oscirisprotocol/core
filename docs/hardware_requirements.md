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

## Platform minimums and benchmark status

All three platforms may join the network and publish signed capability. The
24 GB values describe the current 7B workload class; they are not a minimum for
joining, and lower-memory nodes may be targeted by smaller compatible jobs.

| Platform | Minimum to join | Current 7B profile baseline | Runtime declaration | OSCIRIS benchmark status | Job targeting |
| --- | --- | --- | --- | --- | --- |
| NVIDIA on Linux | Supported binary and truthful capability declaration; no GPU-memory minimum to join | CUDA-capable NVIDIA GPU with at least 24 GB VRAM; 8 CPU cores, 32 GB RAM, 160 GB SSD | `cuda_available=true`; runtimes include `python3` and `cuda` | Verified on A10G 24 GB for bounded 3B inference and 7B QLoRA; verified on L40S 48 GB for bounded 7B inference | Target CUDA jobs by job type and declared VRAM after a local smoke test |
| AMD on Linux | Supported binary and truthful capability declaration; no GPU-memory minimum to join | ROCm-supported AMD GPU with at least 24 GB VRAM as the first 7B profile baseline; 8 CPU cores, 32 GB RAM, 160 GB SSD | runtimes include `python3` and `rocm`; do not set CUDA or MPS | No published OSCIRIS ROCm performance benchmark | Accepted; target only ROCm-compatible jobs that fit the declared model and VRAM |
| Apple Silicon MacBook | Apple Silicon, supported macOS binary, and truthful capability declaration; no unified-memory minimum to join | At least 24 GB unified memory as the first 7B profile baseline; 8 CPU cores, 80 GB free SSD space | `mps_available=true`; runtimes include `python3` plus `mps` or `mlx` | No published OSCIRIS MPS or MLX performance benchmark | Accepted; target only MPS/MLX-compatible jobs that fit declared unified memory |

Capability declarations make AMD and Apple providers addressable before
OSCIRIS publishes comparative performance benchmarks. The current matcher
checks job type and declared memory, but does not yet enforce runtime labels;
operators must therefore target CUDA, ROCm, MPS, and MLX job profiles
explicitly. The AMD and Apple 24 GB baselines may change after measured model
fit, peak memory, throughput, thermals, and receipt reproducibility are
published.

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
  providers are accepted through declared capability and can be targeted by
  compatible jobs. Comparative performance minimums are not yet established.
- AMD ROCm providers are accepted through declared runtime and hardware
  capability. Comparative performance minimums are not yet established.

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
