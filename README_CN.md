# AuraTerm 星环终端

<div align="center">
  <img src="src-tauri/icons/icon.png" alt="AuraTerm Logo" width="128">
</div>

[![CI](https://github.com/Aura-X-Labs/AuraTerm/actions/workflows/ci.yml/badge.svg)](https://github.com/Aura-X-Labs/AuraTerm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Aura-X-Labs/AuraTerm)](https://github.com/Aura-X-Labs/AuraTerm/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri%202-blue)](https://tauri.app/)
[![Vue 3](https://img.shields.io/badge/Vue-3.x-brightgreen)](https://vuejs.org/)

**[English](README.md)** · [官网](https://auraxlab.com) · [下载](https://auraxlab.com/download/auraterm/latest) · [用户手册](https://auraxlab.com/docs) · [更新日志](Changelog.md)

**AuraTerm** 是一款现代化的跨平台终端，面向整天泡在 SSH 会话、串口控制台和实验室设备里的人。它把高速的 Xterm.js 渲染与 Rust 后端结合在一起，原生支持 SSH / 串口 / Telnet，凭据加密存储，并提供可选的云能力 **Live Sync**：书签跨设备同步，还能从浏览器或另一台机器接入你的终端。

支持 **macOS（Apple Silicon）**、**Windows（x64）** 与 **Linux（x64）**。

---

## 目录

- [功能特性](#功能特性)
- [安装](#安装)
- [开发](#开发)
- [测试](#测试)
- [项目结构](#项目结构)
- [发布](#发布)
- [数据存放位置](#数据存放位置)
- [文档](#文档)
- [参与贡献](#参与贡献)
- [许可证](#许可证)

---

## 功能特性

### 连接

| 协议 | 亮点 |
| --- | --- |
| **SSH** | 密码 / 私钥 / 交互式（MFA）认证 · 跳板机 · known_hosts 管理 · 自动重连并配合 `tmux` / `screen` 保持会话 · 本地（`-L`）、远程（`-R`）与动态 SOCKS5（`-D`）隧道 |
| **SFTP / SCP** | 内置远程文件管理器，支持拖拽传输与进度显示 |
| **串口** | 自动枚举设备 · 波特率、数据位、校验、停止位、流控 · 预设 · 不断开即可修改线路参数、发送 BREAK、驱动 DTR/RTS、读取调制解调器状态线 |
| **网络串口（RFC 2217）** | 连接串口设备服务器（ser2net、Moxa NPort、Digi、Lantronix），线路参数真正下发到对端；对端不支持时降级为纯字节管道 |
| **裸 TCP** | 到 `host:port` 的纯字节管道，给只提供这一种接法的设备服务器 |
| **Telnet** | 有状态 IAC 协商，支持终端类型、窗口尺寸，正确转义 IAC |
| **本地 Shell** | 任意本地 Shell（zsh、bash、PowerShell、Git Bash、cmd、自定义路径） |

内联 **Zmodem**（`rz` / `sz`）在本地、SSH、Telnet 与串口会话中均可用。

### 终端

- **标签页与分屏** —— 拖拽排序、重命名、重名自动追加北约字母后缀、二叉树分屏布局、重启后恢复工作区。
- **Shell 集成** —— OSC 133 命令标记与退出码，命令间跳转、重跑、复制。
- **输入工具栏** —— 按工具栏与分组组织的快捷按钮（可限定只在某些主机或书签分组显示）、带历史的多行输入框、命令面板（`Ctrl/Cmd+Shift+P`）。
- **会话日志** —— 每个会话独立记录，文件名支持占位符模板（主机、日期、时间等）。
- **主题** —— 内置预设，终端主题与界面明暗联动（跟随终端 / 浅色 / 深色）。
- **渲染** —— Xterm.js + WebGL 插件、Unicode 11、可点击链接、搜索。
- **多语言** —— 英文与简体中文界面。

### 书签

- 任意层级分组、快速搜索、右键管理。
- 从 OpenSSH `config` 与 PuTTY 会话导入。
- **书签分享** —— 把分组导出为不含凭据的分享文件，或生成短**分享码**（端到端加密、可设有效期、可撤销，接收方无需账号）。
- 导入前预览，逐条选择新增 / 更新 / 跳过；信任闸门默认剥离外部文件中的登录后命令、自动登录响应与跳板机凭据。

### 安全

- 密码与私钥同书签元数据分开存放，使用 **AES-256-GCM + Argon2id** 加密，可选主密码保护（macOS / Windows 可选用系统钥匙串自动解锁）。
- AI API 密钥不进入 `settings.json`、不随设置导出、不参与云同步。
- 分享文件与分享码永远不包含凭据。

### Live Sync（可选，需要 AuraXLab 账号）

一个菜单，四类对端，凡涉及远端的链路均为端到端加密：

| 成员 | 对端是谁 |
| --- | --- |
| **同步** | 你的其它 AuraTerm 安装 —— 书签、设置与 known_hosts 跟随账号；已保存的凭据上传前先用主密码封装，服务器无法读取。启动后、书签变更后与每 30 分钟自动同步。 |
| **Live Console** | 你的浏览器 —— 在 [auraxlab.com/console](https://auraxlab.com/console) 观看或输入。 |
| **Live Share** | 外人 —— 发一个一次性分享码，选择只读或可写，审批控制申请。 |
| **Live Relay** | 你自己的另一台机器 —— 镜像账号下其它设备的会话、申请控制权，或（需显式开启）用对方自己的书签在对方机器上新建本地 / 串口 / SSH 会话。默认关闭，最终裁决永远在被接入的设备上。 |

### AI 助手（可选）

侧栏面板，流式接入 Anthropic Messages API 或任意 OpenAI 兼容接口（DeepSeek、Kimi、Ollama 等）。建议的命令可复制或填入提示符，不按回车不会执行。

### 系统集成

- 自定义标题栏与原生窗口控制。
- Windows：资源管理器右键「在 AuraTerm 中打开」，NSIS 安装包与 Microsoft Store（MSIX）包。
- 启动时恢复窗口状态、标签页与分屏布局。

---

## 安装

任选一个渠道：

- **官网** —— [auraxlab.com/download/auraterm/latest](https://auraxlab.com/download/auraterm/latest)（macOS `.dmg`、Windows `.exe`、Linux 包）。
- **GitHub Releases** —— [github.com/Aura-X-Labs/AuraTerm/releases](https://github.com/Aura-X-Labs/AuraTerm/releases)。
- **Microsoft Store** —— [apps.microsoft.com/detail/9P6B6G5QGGWT](https://apps.microsoft.com/detail/9P6B6G5QGGWT)。

平台说明：

- **macOS**：仅提供 Apple Silicon 版本，不再构建 Intel 版本。
- **Linux**：运行时需要 WebKitGTK 4.1（`libwebkit2gtk-4.1`），主流发行版均已自带。

---

## 开发

### 环境要求

| 工具 | 版本 | 说明 |
| --- | --- | --- |
| Rust | stable | 通过 [rustup](https://rustup.rs/) 安装 |
| Node.js | 20（18+ 可用） | 与 CI 一致 |
| Python 3 | 任意近期版本 | 版本同步与发布脚本使用 |

Ubuntu / Debian 还需安装 Tauri 依赖的系统库：

```bash
sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libudev-dev
```

### 运行

```bash
git clone https://github.com/Aura-X-Labs/AuraTerm.git
cd AuraTerm
npm install
npm run tauri dev        # 热重载的开发版本（或 make run）
```

Vite 开发服务器固定使用 **1420** 端口，被占用时会直接失败。

> 请始终使用 `npm run tauri …` 包装命令，而不是裸的 `tauri` CLI。包装命令会先运行 `scripts/sync_version.py`，把 `package.json` 里的版本号同步到 `Cargo.toml`、`tauri.conf.json` 与 lockfile。

### 构建

```bash
npm run tauri build      # 安装包输出到 src-tauri/target/release/bundle/
make build               # 同上，附带计时并按平台选择打包目标
```

### 日常检查

```bash
npm run build                          # vue-tsc 类型检查 + Vite 构建
cd src-tauri && cargo check            # Rust 编译检查
make update                            # 在现有版本范围内更新 npm 与 cargo 依赖
make clean                             # 删除 src-tauri/target 与 dist
```

---

## 测试

| 套件 | 命令 | 覆盖范围 |
| --- | --- | --- |
| 前端（Vitest） | `npm test` | composables、书签、云同步、Live Sync 状态、终端行为（`src/__tests__/`） |
| Rust | `cd src-tauri && cargo test --bin auraterm` | 设置往返、SSH 重连状态机、known_hosts 解析、RFC 2217、E2EE、Relay 准入 —— 以 `mod tests` 形式紧邻代码 |
| 发布脚本 | `make test-scripts` | 写入官网检出的 Python 脚本 |

CI（`.github/workflows/ci.yml`）在每次推送到 `main` / `dev` 与每个 PR 上运行以上三套测试、`cargo check`，以及 Ubuntu、macOS arm64、Windows 的完整 Tauri 构建。

凡涉及真实 PTY、真实 SSH 主机或物理串口的部分仍需用 `npm run tauri dev` 手工验证。仓库内附带一个用于本地测试的迷你 RFC 2217 服务器：`scripts/rfc2217_test_server.py`。

---

## 项目结构

```
src/                      Vue 3 + TypeScript 前端
  App.vue                 标签页、对话框、分屏编排（只做协调，不堆逻辑）
  TerminalComponent.vue   Xterm.js 实例、键鼠事件
  composables/            共享行为（标签、搜索、菜单、会话 IPC、自动同步、隧道）
  usePaneLayout.ts        分屏树
  settings.ts / types.ts  AppSettings、主题推导、共享类型
  i18n/locales/           en、zh-CN
  __tests__/              Vitest 套件
src-tauri/                Rust + Tauri 2 后端
  src/main.rs             PTY、窗口管理、命令注册
  src/ssh/                会话、SFTP/SCP 传输、端口转发、known_hosts
  src/serial*.rs, rfc2217.rs, telnet.rs
  src/encryption.rs, keychain.rs, e2ee.rs, pake.rs
  src/cloud_sync.rs, cloud_bridge.rs, assist_*.rs, relay_*.rs, remote_tab.rs
  src/ai.rs               Anthropic / OpenAI 兼容流式接口
  capabilities/           Tauri 权限清单
scripts/                  版本同步、官网同步、MSIX 打包、RFC 2217 测试服务器
Changelog.md              更新日志（中文），发布时同步到官网
```

前后端通过 Tauri IPC 通信：前端 `invoke` 在 `src-tauri/src/main.rs` 注册的命令，Rust 侧发出 `pty-output`、`pty-exit` 等事件。

---

## 发布

1. 只修改 `package.json` 的 `version`，同步脚本会传播到其它文件。
2. 在 `Changelog.md` 中新增 `## x.y.z` 小节。
3. 合并到 `main` 后推送 `vX.Y.Z` 标签。`.github/workflows/release.yml` 会构建 Linux、macOS arm64 与 Windows 包（含 MSIX）并发布 GitHub Release。
4. `make release` 把更新日志与版本号写入旁边的 AuraXLabs 官网检出（可用 `AURAXLABS_DIR` 覆盖路径）。

Windows 专属变体：

```bash
npm run tauri:store             # Microsoft Store 构建
npm run package:msix            # MSIX 包
npm run package:msixupload      # 商店上传包
npm run release:windows         # 完整签名发布流程
```

代码签名与商店提交细节见 `docs/Windows-Release.md`（参见[文档](#文档)）。

---

## 数据存放位置

所有数据都在平台的应用配置目录下（`~/Library/Application Support/com.auraxlab.auraterm`、`%APPDATA%\com.auraxlab.auraterm`、`~/.config/com.auraxlab.auraterm`）：

- `settings.json` —— 设置、主题、工作区状态（不含任何密钥）。
- `connections.json` —— 不含凭据的书签。
- `credentials.enc` —— 密码、私钥与口令，与上面两者分开加密存放。

---

## 文档

- **用户手册** —— [auraxlab.com/docs](https://auraxlab.com/docs)。
- **工程文档** —— 本检出里的 `docs/` 是指向 Aura 工作区共享文档仓库的符号链接（功能规格、Live Sync / RFC 2217 / 书签分享等设计说明，以及 `Windows-Release.md`），不属于本仓库的历史。
- **Agent 指引** —— `CLAUDE.md` 与 `.github/copilot-instructions.md` 记录了约定与不显眼的坑，供人和 AI 修改代码时参考。

---

## 参与贡献

欢迎 Bug 报告、功能建议与 Pull Request。请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

1. Fork 并从 `main` 建分支。
2. 完成修改，并在上述测试套件适用处补充测试。
3. 运行 `npm run build`、`npm test` 与 `cd src-tauri && cargo check && cargo test --bin auraterm`。
4. 提交信息使用英文祈使句（`Fix: …`、`Feat: …`）。
5. 发起 Pull Request。

---

## 许可证

AuraTerm 是自由软件，采用 **GNU 通用公共许可证 v3.0 或更高版本**（`GPL-3.0-or-later`）发布，完整文本见 [LICENSE](LICENSE)。

Copyright (c) 2026 Aura-X-Labs。

0.3.5 及之前的版本以 MIT 许可证发布，这些版本仍可按 MIT 使用；自 0.3.6 起为 GPL-3.0-or-later。第三方组件保留各自的许可证（MIT、Apache-2.0、ISC、BSD、MPL-2.0），均与 GPLv3 兼容。
