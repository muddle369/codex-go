# CodexGO

<p align="center">
  <img src="assets/images/codex-go.png" alt="CodexGO 图标" width="160">
</p>

<p align="center">
  中文 | <a href="README_EN.md">English</a>
</p>

<p align="center">
  <img alt="Release" src="https://img.shields.io/github/v/release/muddle369/codex-go">
  <img alt="Stars" src="https://img.shields.io/github/stars/muddle369/codex-go">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.85%2B-orange">
  <img alt="Tauri" src="https://img.shields.io/badge/tauri-2.x-24C8DB">
</p>

CodexGO 是面向 Codex App 的外部增强启动器和管理工具。它不修改 Codex App 原始安装文件，而是通过外部启动器启动 Codex，并使用 Chromium DevTools Protocol 注入增强能力。

## 快速使用

从 [GitHub Releases](https://github.com/muddle369/codex-go/releases) 下载最新版安装包：

- Windows：`CodexGO-*-windows-x64-setup.exe`
- macOS Apple Silicon：`CodexGO-*-macos-arm64.dmg`
- macOS Intel：`CodexGO-*-macos-x64.dmg`

安装后双击 `CodexGO`：

- 未配置时，会显示快捷启动卡片，引导输入令牌并一键配置 `SCD_Ai`。
- 已配置时，可选择供应商配置、纯 API / 混合 API 模式并启动 Codex。
- 进入管理控制台后，可通过左侧 `快捷启动` 随时回到启动卡片。

## 主要功能

- 单 App 启动入口，兼顾快捷启动和高级配置。
- `SCD_Ai` 一键配置，默认使用 `https://007.007ai.cc/v1`。
- 支持纯 API 模式和混合 API 模式。
- 支持供应商配置、模型获取、配置切换和注入启动。
- 支持脚本实验室，可从公开仓库拉取脚本索引并安装用户脚本。
- 支持 Codex 页面增强、会话管理、工具与插件管理、Zed 远程项目等能力。
- 支持 macOS 菜单栏 / Windows 托盘隐藏与唤起。
- 支持 GitHub Release 检查更新。

## 脚本实验室

脚本实验室读取公开仓库中的静态索引：

```text
https://raw.githubusercontent.com/muddle369/codex-go/main/index.json
```

脚本文件放在仓库的 `scripts/` 目录中。更新脚本只需要同步 `index.json` 和 `scripts/*.js` 到公开仓库的 `main` 分支，用户刷新脚本实验室即可看到最新内容。

## 开发

环境依赖：

- Rust / Cargo
- Node.js / npm
- Tauri 2 相关依赖
- macOS 打包需要 `iconutil`、`hdiutil`、`codesign`
- Windows 打包需要 Windows 环境或交叉编译环境、NSIS、WebView2Loader

常用命令：

```bash
cd apps/codexx-manager
npm install
npm run check
npm run vite:build

cd ../..
cargo check --workspace
```

macOS 打包示例：

```bash
cd apps/codexx-manager
npm run vite:build
cargo build --release -p codexx-launcher -p codexx-manager --manifest-path ../../Cargo.toml
VERSION=1.0.0 ../../scripts/installer/macos/package-dmg.sh 1.0.0 $(uname -m)
```

## 请我喝杯咖啡

如果 CodexGO 帮到了你，可以请我喝杯咖啡，或者随手赞赏支持一下继续维护。

<p align="center">
  <img src="assets/images/feng-alipay.JPG" alt="支付宝赞赏码" width="220">
  <img src="assets/images/feng-wechat.JPG" alt="微信赞赏码" width="220">
</p>
