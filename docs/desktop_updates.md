# Desktop Updates

OSCIRIS Node checks the latest GitHub Release asynchronously after startup and
also exposes a manual check in **Local node > Desktop updates**.

The application does not silently install software. When a newer release is
available, the operator starts the download. Tauri verifies the downloaded
package against the updater public key compiled into the installed application
before installation. The app then relaunches; Windows may close the running
process as part of the installer flow.

## Publication Boundary

Desktop update publication is restricted to `v*` tag builds:

1. GitHub Actions builds each platform with
   `bundle.createUpdaterArtifacts=true`.
2. Tauri signs each update bundle using the
   `TAURI_SIGNING_PRIVATE_KEY` repository secret. The generated key has no
   passphrase, so the workflow explicitly supplies an empty
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. The release job requires one signed updater artifact for each supported
   platform.
4. `scripts/generate_desktop_update_manifest.mjs` fails unless all three
   bundles and signatures are present and the Git tag matches the desktop
   version compiled from `tauri.conf.json`.
5. Only then does the workflow publish `latest.json` and its referenced assets.

Normal branch and pull-request builds do not receive the private key and do not
produce updater artifacts.

## Supported Update Targets

| Tauri target | Release asset |
| --- | --- |
| `darwin-aarch64` | `osciris-node-darwin-aarch64.app.tar.gz` |
| `linux-x86_64` | `osciris-node-linux-x86_64.AppImage` |
| `windows-x86_64` | `osciris-node-windows-x86_64-setup.exe` |

The static update endpoint is:

`https://github.com/oscirisprotocol/core/releases/latest/download/latest.json`

Signed `v0.x` releases are published as the repository's latest release so the
GitHub redirect resolves. Their pre-1.0 version remains the beta maturity
signal.

## Signing Key Operations

- GitHub Actions receives the private key through the
  `TAURI_SIGNING_PRIVATE_KEY` secret.
- A permission-restricted offline copy is stored outside the repository at
  `~/.tauri/osciris-node-updater.key`; it must be included in the operator's
  encrypted backup process.
- The public key is embedded in `apps/desktop/src-tauri/tauri.conf.json`.
- Losing the private key prevents existing installations from accepting future
  updates.
- Replacing the embedded public key alone does not update existing clients.
  Key rotation requires a transition release signed by the old key, containing
  application logic/configuration for the new key, before new-key-only releases
  are published.

Do not print the private key in logs, commit it, place it in an artifact, or
send it through the desktop application.
