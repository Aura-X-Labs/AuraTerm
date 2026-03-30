# AuraTerm

<div align="center">
  <img src="src-tauri/icons/icon.png" alt="AuraTerm Logo" width="128">
</div>

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri-blue)](https://tauri.app/)
[![Vue 3](https://img.shields.io/badge/Vue-3.x-brightgreen)](https://vuejs.org/)

**[中文文档](README_CN.md)**

**AuraTerm** is a modern, cross-platform terminal emulator built with [Tauri](https://tauri.app/). It provides powerful connection capabilities (SSH, Serial, Telnet, Local Shell) with a modern UI/UX experience, tab management, split pane support, and a high-performance rendering engine.

---

## ✨ Features

- **🚀 Cross-Platform**: Full support for macOS (Apple Silicon/Intel), Windows, and Linux.
- **🛡️ Powerful Connection Protocols**:
  - **SSH2**: Password and key-based authentication, MFA support, built-in SFTP file manager.
  - **Serial Port**: Auto device enumeration, customizable baud rate, data bits, stop bits, flow control.
  - **Telnet**: Full Telnet protocol support.
  - **Local Shell**: Customizable local shell (PowerShell, Git Bash, zsh, etc.).
- **🎭 Modern UI/UX**:
  - **Tab Management**: Drag-and-drop reordering, custom renaming, auto-generated NATO alphabet suffixes.
  - **Bookmark Management**: Grouped connections with quick search and one-click connect.
  - **Split Panes**: Flexible terminal layout within a single window.
- **📈 High Performance**: Powered by Xterm.js with WebGL plugin for smooth rendering and low resource usage.
- **📋 Session Logging**: Flexible log saving with customizable filename templates.
- **🔄 Enhanced Reconnection**: SSH auto-reconnect and tmux/screen session persistence.

---

## 🛠️ Tech Stack

- **Frontend**: Vue 3, TypeScript, Xterm.js
- **Backend**: Rust, Tauri v2
- **Key Libraries**: `portable-pty`, `serialport-rs`, `russh`

---

## 🚀 Getting Started

### Prerequisites

1. Install [Rust](https://www.rust-lang.org/tools/install).
2. Install Node.js (v18+ recommended).

### Development

```bash
# Clone the repository
git clone https://github.com/Aura-X-Labs/AuraTerm.git
cd AuraTerm

# Install dependencies
npm install

# Start development server
npm run tauri dev
```

### Build

```bash
# Vue type check
npm run build

# Rust backend check
cd src-tauri
cargo check
```

---

## 📦 Build & Release

Generate installers for different platforms:

```bash
npm run tauri build
```

Build artifacts will be located in `src-tauri/target/release/bundle/`.

Windows release variants:

- Standard Windows installer: `npm run tauri build`
- Microsoft Store Windows installer: `npm run tauri:store`

Detailed Windows release and signing instructions are documented in [docs/Windows-Release.md](docs/Windows-Release.md).

---

## 📄 License

This project is licensed under the **MIT** License - see the [LICENSE](LICENSE) file for details.

---

## 🤝 Contributing

We welcome all forms of contribution! Whether it's bug reports, feature suggestions, or pull requests.

1. **Fork** the repository.
2. Create your feature branch (`git checkout -b feature/AmazingFeature`).
3. Commit your changes (`git commit -m 'Add some AmazingFeature'`).
4. Push to the branch (`git push origin feature/AmazingFeature`).
5. Open a **Pull Request**.

---

## 💡 Inspiration

- **Tauri**: Powerful cross-platform foundation.
