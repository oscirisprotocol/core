# OSCIRIS Release Workflow Handoff

This repo already has the beta collaboration branch and onboarding changes
reviewable in GitHub. The remaining missing piece is the GitHub Actions
release workflow.

It was prepared locally but could not be pushed with the current credentials
because GitHub rejected workflow-file updates without `workflow` scope.

## Maintainer action

Create `.github/workflows/release.yml` with the content below, or apply the
same file from a session that has permission to update workflow files.

```yaml
name: Release

on:
  push:
    tags:
      - "v*"
  workflow_dispatch:

permissions:
  contents: write

jobs:
  build:
    name: Build ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            archive_name: osciris-node-x86_64-unknown-linux-gnu.tar.gz
          - os: macos-14
            target: aarch64-apple-darwin
            archive_name: osciris-node-aarch64-apple-darwin.tar.gz
    steps:
      - name: Checkout
        uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Cache cargo
        uses: Swatinem/rust-cache@v2

      - name: Build release binary
        run: cargo build --locked --release -p osciris-cli --bin osciris-node --target ${{ matrix.target }}

      - name: Package binary
        run: |
          set -euo pipefail
          staging_dir="release-${{ matrix.target }}"
          mkdir -p "$staging_dir"
          cp "target/${{ matrix.target }}/release/osciris-node" "$staging_dir/osciris-node"
          tar -C "$staging_dir" -czf "${{ matrix.archive_name }}" osciris-node
          python3 - <<'PY'
          import hashlib
          from pathlib import Path

          archive = Path("${{ matrix.archive_name }}")
          digest = hashlib.sha256(archive.read_bytes()).hexdigest()
          Path("${{ matrix.archive_name }}.sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
          PY

      - name: Upload build artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.archive_name }}
          path: |
            ${{ matrix.archive_name }}
            ${{ matrix.archive_name }}.sha256

  release:
    name: Publish GitHub Release
    needs: build
    runs-on: ubuntu-latest
    steps:
      - name: Download build artifacts
        uses: actions/download-artifact@v4
        with:
          path: release-artifacts

      - name: Create release notes
        run: |
          set -euo pipefail
          find release-artifacts -type f | sort > release-artifacts/FILES.txt

      - name: Publish release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          files: |
            release-artifacts/**/*.tar.gz
            release-artifacts/**/*.sha256
```

## Verification after maintainer apply

- push a beta tag such as `v0.1.0`
- confirm the workflow builds Linux and macOS release artifacts
- confirm the GitHub Release includes `.tar.gz` and `.sha256` files
- confirm `OSCIRISLABS/public/beta-release-manifest.json` points to the same version
