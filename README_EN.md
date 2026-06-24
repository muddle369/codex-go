# CodexGO

<p align="center">
  <img src="assets/images/codex-go.png" alt="CodexGO icon" width="160">
</p>

<p align="center">
  <a href="README.md">中文</a> | English
</p>

<p align="center">
  <img alt="Release" src="https://img.shields.io/github/v/release/muddle369/codex-go">
  <img alt="Stars" src="https://img.shields.io/github/stars/muddle369/codex-go">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange">
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.x-24C8DB">
</p>

CodexGO is an external launcher and management console for Codex App. It does not modify the original Codex installation. Instead, it launches Codex externally and injects enhancements through Chromium DevTools Protocol.

## Quick Start

Download the latest installer from [GitHub Releases](https://github.com/muddle369/codex-go/releases):

- Windows: `CodexGO-*-windows-x64-setup.exe`
- macOS Apple Silicon: `CodexGO-*-macos-arm64.dmg`
- macOS Intel: `CodexGO-*-macos-x64.dmg`

After installation, open `CodexGO`:

- If no configuration exists, the quick launch card helps you enter a token and configure `SCD_Ai`.
- If configuration exists, you can choose a provider profile, pure API / mixed API mode, and launch Codex.
- In the management console, use `Quick Launch` in the sidebar to return to the launch card at any time.

## Features

- Single App entry for quick launch and advanced configuration.
- One-click `SCD_Ai` setup with `https://007.007ai.cc/v1`.
- Pure API mode and mixed API mode.
- Provider configuration, model fetching, profile switching, and injection launch.
- Script Lab for installing user scripts from the public repository index.
- Codex page enhancements, session management, tools/plugins management, and Zed remote project support.
- macOS menu bar / Windows tray hide and restore.
- GitHub Release update checks.

## Script Lab

Script Lab reads the static index from the public repository:

```text
https://raw.githubusercontent.com/muddle369/codex-go/main/index.json
```

Scripts live in the `scripts/` directory. To update scripts, sync `index.json` and `scripts/*.js` to the public repository's `main` branch. Users can refresh Script Lab to see updates.

## Development

Requirements:

- Rust / Cargo
- Node.js / npm
- Tauri 2 dependencies
- macOS packaging: `iconutil`, `hdiutil`, `codesign`
- Windows packaging: Windows or cross-compilation environment, NSIS, WebView2Loader

Common commands:

```bash
cd apps/codexx-manager
npm install
npm run check
npm run vite:build

cd ../..
cargo check --workspace
```

macOS packaging example:

```bash
cd apps/codexx-manager
npm run vite:build
cargo build --release -p codexx-launcher -p codexx-manager --manifest-path ../../Cargo.toml
VERSION=1.0.0 ../../scripts/installer/macos/package-dmg.sh 1.0.0 $(uname -m)
```

## Buy Me a Coffee

If CodexGO helps you, you can buy me a coffee or support ongoing maintenance.

<p align="center">
  <img src="assets/images/feng-alipay.JPG" alt="Alipay sponsor QR code" width="220">
  <img src="assets/images/feng-wechat.JPG" alt="WeChat sponsor QR code" width="220">
</p>
