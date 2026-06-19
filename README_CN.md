# AuraTerm 星环终端

<div align="center">
  <img src="src-tauri/icons/icon.png" alt="AuraTerm Logo" width="128">
</div>

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri-blue)](https://tauri.app/)
[![Vue 3](https://img.shields.io/badge/Vue-3.x-brightgreen)](https://vuejs.org/)

**[English](README.md)**

**AuraTerm** 是一款基于 [Tauri](https://tauri.app/) 开发的现代化、跨平台终端模拟器。它提供强大的连接能力（SSH, Serial, Telnet, Local Shell），拥有现代化的 UI/UX 体验、标签页管理、分屏支持以及高性能的渲染引擎。

---

## ✨ 核心特性

- **🚀 跨平台支持**: 完美支持 macOS (Apple Silicon/Intel), Windows 和 Linux。
- **🛡️ 强大的连接协议**:
  - **SSH2**: 支持密码、私钥认证，集成 MFA 多因素认证，内置 SFTP 文件管理器。
  - **Serial (串口)**: 自动枚举设备，支持自定义波特率、数据位、停止位、流控等完整配置。
  - **Telnet**: 支持终端类型、窗口尺寸等有状态 IAC 协商。
  - **Local Shell**: 可自定义本地 Shell (PowerShell, Git Bash, zsh 等)。
- **🎭 现代化的 UI/UX**:
  - **Tab 标签页管理**: 支持拖拽排序、自定义重命名、自动生成北约字母表后缀。
  - **书签管理**: 任意层级文件夹、快速搜索，并支持导入 OpenSSH/PuTTY 会话。
  - **内联 Zmodem**: Local/SSH/Telnet/Serial 会话自动识别 `rz`/`sz` 上传下载。
  - **Shell 集成**: OSC 133 命令标记、退出码、导航、重跑与复制。
  - **分屏支持 (Split Panes)**: 在同一窗口内灵活排列多个终端。
- **📈 高性能渲染**: 基于 Xterm.js 配合 WebGL 插件实现极致的渲染速度与低资源占用。
- **📋 会话日志**: 灵活的日志保存机制，支持高度自定义的文件名占位符模版。
- **🔄 增强重连**: 支持 SSH 自动重连及 tmux/screen 会话持久化管理。

---

## 🛠️ 技术栈

- **Frontend**: Vue 3, TypeScript, Xterm.js
- **Backend**: Rust, Tauri v2
- **Key Libraries**: `portable-pty`, `serialport-rs`, `russh`

---

## 🚀 快速上手

### 开发环境配置

1. 确保已安装 [Rust](https://www.rust-lang.org/tools/install) 环境。
2. 安装 Node.js (推荐 v18+)。

### 运行开发版本

```bash
# 克隆仓库
git clone https://github.com/Aura-X-Labs/AuraTerm.git
cd AuraTerm

# 安装前端依赖
npm install

# 启动开发环境
npm run tauri dev
```

### 编译检查

```bash
# Vue 类型检查
npm run build # 包含 vue-tsc 检查

# Rust 后端检查
cd src-tauri
cargo check
```

---

## 📦 构建与发布

针对不同平台生成安装包：

```bash
npm run tauri build
```

构建结果将位于 `src-tauri/target/release/bundle/` 目录下。

---

## 📄 许可证

本项目采用 **MIT** 许可证，详情请参阅 [LICENSE](LICENSE) 文件。

---

## 🤝 贡献规范

我们欢迎所有形式的贡献！无论是提交 Bug 报告、功能建议还是直接提交 Pull Request。

1. **Fork** 本仓库。
2. 创建您的特性分支 (`git checkout -b feature/AmazingFeature`)。
3. 提交您的修改 (`git commit -m 'Add some AmazingFeature'`)。
4. 推送到分支 (`git push origin feature/AmazingFeature`)。
5. 开启一个 **Pull Request**。

---

## 💡 灵感来源

- **Tauri**: 强力支持的跨平台底座。
