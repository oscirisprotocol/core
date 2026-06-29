# Lessons

- When a public manifest controls installer update behavior, do not publish it
  automatically unless the referenced release assets have already been verified
  as reachable and consistent.
- When pushing workflow file changes, ensure the workspace is using credentials
  with GitHub `workflow` write scope or use the repo SSH remote up front; plain
  HTTPS OAuth tokens may allow normal pushes but reject workflow updates.
