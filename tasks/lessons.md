# Lessons

- When a public manifest controls installer update behavior, do not publish it
  automatically unless the referenced release assets have already been verified
  as reachable and consistent.
- When pushing workflow file changes, ensure the workspace is using credentials
  with GitHub `workflow` write scope or use the repo SSH remote up front; plain
  HTTPS OAuth tokens may allow normal pushes but reject workflow updates.
- Treat GitHub repositories, releases, and raw repository artifacts as the
  OSCIRIS publication authority. Railway is only a website runtime or mirror
  and must not be described as blocking publication.
- Do not equate missing platform benchmarks with protocol exclusion. OSCIRIS
  accepts heterogeneous nodes through signed capability declarations; benchmark
  evidence limits performance claims, while job profiles determine targeting.
- Describe inference as provider-local execution, never as centrally hosted by
  OSCIRIS. Each participant stores and serves the pinned model on its own
  machine; OSCIRIS coordinates discovery, assignment, receipts, verification,
  and published network status.
- A desktop controller is not distributable when its daemon exists only in the
  developer workspace. Bundle the target-native daemon as a fixed sidecar and
  verify the installed artifact contains it before presenting launch controls
  as complete.
- Do not label a configurable Horizen testnet settlement token as official
  USDC. Horizen publishes mainnet USDC, but the official testnet token list does
  not include USDC; use an explicit `USDC_TEST` boundary until a test contract
  is deployed and verified.
