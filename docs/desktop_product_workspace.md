# OSCIRIS Desktop Compute Workspace

## Product Purpose

OSCIRIS Desktop presents the complete buyer and operator workflow without
moving AI execution into a central OSCIRIS server:

1. define a training or inference workload;
2. select privacy, hardware, verifier quorum, challenge window, and budget;
3. review funding before network broadcast;
4. track matching, provider-local execution, verification, and completion;
5. inspect evidence receipts and Horizen anchors;
6. manage the workspace payment address without giving OSCIRIS custody of keys.

## Job Lifecycle

Desktop records use these states:

| State | Meaning |
| --- | --- |
| Draft | Local terms only; no funds or network state |
| Awaiting funding | Approved for payment review; still not broadcast |
| Queued | Funded and accepted for network publication |
| Matching | Eligible provider selection is in progress |
| Running | The selected provider is executing locally |
| Verifying | Execution evidence is under independent review |
| Completed | Required verification and settlement conditions are satisfied |
| Failed | Execution or policy requirements were not satisfied |

Desktop now writes the local buyer-side protocol controls for this lifecycle:

- `Draft` → `Awaiting funding` through local job approval;
- `Awaiting funding` → `Queued` by publishing a signed protocol announcement;
- `Queued`/`Matching` → `Running` by triggering daemon-side provider matching
  against signed claims/capabilities already present in the daemon protocol
  store;
- `Running`/`Verifying` updates by importing signed provider evidence and
  signed verifier receipt announcements;
- `Completed` only after verifier quorum is met.

Desktop still does not run external provider or verifier machines. Provider
execution and verifier review are backend/node responsibilities; Desktop
observes and imports their signed protocol outputs.

## Remote Prompt Boundary

Question: can a developer send a prompt from Desktop to another machine for
test inference?

Current answer: yes, through the current desktop test inference surface and
signed peer transport. The remaining gap is verified multi-host provider and
verifier quorum at release quality.

What works today:

- A multi-host asynchronous inference-economics workflow can run through the
  CLI using `network serve`, `network run-provider`, `network run-verifier`,
  job announcement, provider claim/assignment, execution receipt, verifier
  receipt, and quorum status.
- Desktop can participate on the buyer side after signed protocol records are
  present: publish a job, match against stored provider claims/capabilities,
  import provider evidence, import verifier receipts, and show completion.

What is not yet implemented:

- Desktop does not start/join the P2P network from the UI.
- Desktop does not yet expose a full network bootstrap workflow from the UI.
- The current backend still needs a verified remote multi-host prompt round
  trip with verifier quorum before the release claim becomes public beta
  quality.
- Public beta release assets and manifest publication remain a separate
  release step from the desktop protocol work.

Use `docs/multi_host_testnet_join_guide.md` today for multi-host CLI testing
and the desktop `Test inference` surface for signed prompt transport and
verifier-ready evidence capture.

## Wallet Boundary

OSCIRIS Desktop uses a watch-only wallet model:

- stores a public EVM address;
- reads native ETH and an explicitly configured ERC-20 balance;
- uses the official Horizen testnet RPC and chain ID;
- exposes the public address as the deposit coordinate;
- prepares ERC-20 transfer calldata for external-wallet signing;
- never accepts, stores, or transmits private keys or seed phrases.

Official Horizen network parameters are published at
<https://docs.horizen.io/overview/rpc/>.

## Stablecoin Boundary

Horizen publishes a mainnet USDC contract, but its official token page does not
publish an official USDC contract for Horizen testnet:
<https://docs.horizen.io/overview/token/>.

Therefore:

- the production asset can be USDC after contract and legal review;
- the desktop testnet symbol defaults to `USDC_TEST`;
- a test-token contract must be explicitly configured;
- the UI must not describe `USDC_TEST` as Circle-issued USDC;
- `osciris-chain` can create escrow transactions for a configured ERC-20
  payment token without attaching native value;
- production job funding still depends on a deployed escrow contract, token
  allowance/funding semantics, and verified test-token or mainnet-token
  configuration.

## Investor Demo Path

1. Start the local daemon.
2. Create one inference draft and one training draft.
3. Open each job to show policy, economics, lifecycle, and proof surfaces.
4. Move a draft into funding review to show the explicit payment boundary.
5. Configure a public Horizen testnet address and refresh its native balance.
6. Configure the deployed OSCIRIS test-token contract when available.
7. Prepare a withdrawal payload and show that signing remains external.
8. Open Evidence to explain how real provider and verifier receipts populate the
   currently empty proof fields.

This is demo-worthy product behavior, not a simulated network. Drafts and wallet
configuration are real local state; provider execution, verifier receipts, and
chain anchors appear only when their integrations produce them.
