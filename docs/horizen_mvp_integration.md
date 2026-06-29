# Horizen Testnet MVP Integration

Date: 2026-06-23

## Purpose

Horizen testnet is the coordination and accountability layer for the OSCIRIS MVP.
It does not run AI workloads. It records the state needed to make provider
participation, receipts, challenges, and settlement reviewable.

## Current Testnet Config

```json
{
  "rpc_url": "https://horizen-testnet.rpc.caldera.xyz/http",
  "chain_id": 2651420,
  "provider_registry": "0x44D592e063d683cC422d7C8432D853DB9B7Dd50e",
  "job_escrow": "0xe876030fE4cE267e0949c414eDE25e19FBcD8A09",
  "receipt_registry": "0xFf4471ef06ab10a5502E2Dc9ADdD3C428C24AB9b",
  "explorer_url": "https://horizen-testnet.explorer.caldera.xyz"
}
```

## MVP State Mapping

| OSCIRIS state | Horizen contract role | Why it matters |
| --- | --- | --- |
| Provider capability | Provider registry | Declares accountable compute capacity |
| Job terms | Job escrow | Defines assignment, terms, and challenge window |
| Execution receipt | Receipt registry | Anchors provider output and evidence root |
| Verification receipt | Receipt registry | Anchors verifier decision and quorum contribution |
| Challenge result | Job escrow / receipt state | Blocks or releases settlement readiness |
| Settlement-ready status | Job escrow | Shows buyer-visible final protocol state |

## Minimal MVP Transaction Path

1. Register provider capability.
2. Open or reference job terms.
3. Record assigned-provider execution receipt.
4. Record verifier receipt.
5. Check challenge window.
6. Export settlement-ready status.

The `SubmitReceipt` path anchors hashes and receipt commitments first. It does
not publish raw payloads, datasets, model artifacts, or full evidence bundles to
Horizen testnet.

## Customer Billing Boundary

For MVP, customer-facing billing should remain stable-value and off-chain unless
testnet stable assets are explicitly configured. Provider collateral can be
represented as testnet-native stake or registry state. This keeps the MVP focused
on proof of accountability rather than payment productionization.

## ZEN Boundary

ZEN is treated as provider collateral or bond logic where supported by the
Horizen deployment path. It is not required as a buyer payment rail for the MVP.

## Evidence Boundary

Only hashes, receipt commitments, policy checkpoints, and status transitions
belong onchain. Raw datasets, model outputs, private keys, full logs, and
unsanitized evidence bundles do not belong onchain.

## MVP Completion Criteria

- contract config loads from `config/horizen-testnet.json`
- provider capability can be represented against the provider registry
- execution and verifier receipts have deterministic hashes
- settlement status can be mapped to testnet state
- evidence package can be independently reviewed off-chain

## Production Gap

Before mainnet or real-customer launch, OSCIRIS still needs:

- audited smart contracts
- final staking/slashing policy
- stable-token billing path
- provider admission controls
- dispute process and legal terms
- operational key custody policy
