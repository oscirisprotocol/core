# OSCIRIS Node Desktop Architecture

## Boundary

OSCIRIS Desktop is a controller for a per-user `osciris-daemon`. It is not a
second protocol implementation and it does not centrally host inference.

```text
React webview
  -> typed Tauri commands
  -> Rust daemon client
  -> authenticated per-user IPC
  -> osciris-daemon
  -> protocol, peer, receipt, and provider-local runtime modules
```

The desktop bridge exposes:

- daemon launch;
- daemon status;
- pause participation;
- resume participation;
- persisted training and inference job drafts;
- explicit transition from draft to funding review;
- watch-only Horizen testnet wallet configuration;
- native and configured ERC-20 balance reads;
- unsigned ERC-20 withdrawal transaction preparation.

Identity, hardware discovery, provider matching, execution, receipts, readiness,
and inference remain visible as pending modules until their daemon endpoints
return measured data.

## Local IPC

The daemon uses:

- a mode `0600` Unix socket inside a mode `0700` state directory on macOS and
  Linux;
- a local-only named pipe on Windows;
- a random 256-bit per-user credential stored outside the webview;
- newline-framed JSON capped at 64 KiB;
- explicit API version and request/response IDs;
- a three-second client timeout.

The Tauri layer never returns the credential to React. The frontend can invoke
only the registered status, launch, and participation commands.

## State

Participation mode, job drafts, lifecycle records, and watch-only wallet
configuration are persisted atomically in `daemon-state.json`. Starting the
desktop does not automatically opt a machine into work. The default is paused.

Current status fields are deliberately bounded:

- daemon/API version;
- process uptime;
- participation mode;
- network state;
- active job count;
- operating system and architecture;
- optional readiness snapshot.

The daemon reports network state as `not_configured` and readiness as absent
until protocol integration exists. The GUI must not synthesize peer counts,
hardware claims, jobs, receipts, or rewards.

## Desktop security

- The app ships only bundled local frontend assets.
- Content Security Policy blocks remote scripts, frames, and objects.
- React has no shell, filesystem, network, or secret-storage permission.
- The Rust layer resolves a fixed daemon binary; the frontend cannot provide an
  executable path or arbitrary arguments.
- The wallet stores only public addresses and token metadata. It never accepts
  a private key or seed phrase.
- Horizen RPC access is fixed to the official HTTPS testnet endpoint.
- Withdrawals are unsigned ERC-20 payloads for review and signing in an external
  wallet.
- Future model installers must verify profile revision, artifact SHA-256, and
  license before execution.
- Application updates use signed Tauri updater artifacts from the latest
  GitHub Release. Checks run asynchronously; download and installation require
  explicit operator approval, and unsigned packages are rejected.

## Development

Build the daemon:

```bash
cargo build -p osciris-daemon
```

Install desktop dependencies:

```bash
cd apps/desktop
pnpm install
```

Run the desktop app:

```bash
pnpm tauri dev
```

The Tauri pre-build hook compiles the daemon and stages it under the
target-triple filename required by the Tauri sidecar bundler. Debug builds use
the workspace `target/debug` daemon; distributable builds compile the daemon in
release mode. `OSCIRIS_DAEMON_BIN` may override the resolved binary for local
developer testing; the webview cannot set it.

Build a native package:

```bash
pnpm tauri build
```

The package embeds the target-native `osciris-daemon`, Apache-2.0 license, and
NOTICE. The Rust bridge launches only that fixed daemon binary; no shell
capability is granted to the webview.

## Release boundary

The foundation does not register autostart or publish signed installers.
Public desktop release is complete only when:

- macOS and Windows artifacts are code signed;
- application updates are signed and release the GUI and daemon as one unit;
- app close leaves the per-user daemon lifecycle explicit;
- uninstall offers a clear choice to retain or remove node state.
