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
