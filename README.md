# AuraTerm

<div align="center">
  <img src="src-tauri/icons/icon.png" alt="AuraTerm Logo" width="128">
</div>

[![CI](https://github.com/Aura-X-Labs/AuraTerm/actions/workflows/ci.yml/badge.svg)](https://github.com/Aura-X-Labs/AuraTerm/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Aura-X-Labs/AuraTerm)](https://github.com/Aura-X-Labs/AuraTerm/releases/latest)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg)](LICENSE)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri%202-blue)](https://tauri.app/)
[![Vue 3](https://img.shields.io/badge/Vue-3.x-brightgreen)](https://vuejs.org/)

**[中文文档](README_CN.md)** · [Website](https://auraxlab.com) · [Download](https://auraxlab.com/download/auraterm/latest) · [User Manual](https://auraxlab.com/docs) · [Changelog](Changelog.md)

**AuraTerm** is a modern, cross-platform terminal for people who live in SSH sessions, serial consoles and lab equipment. It combines a fast Xterm.js renderer with a Rust backend, first-class SSH/Serial/Telnet support, encrypted credential storage, and an optional cloud layer (**Live Sync**) that keeps your bookmarks in sync and lets you reach your terminals from a browser or another machine.

Runs on **macOS (Apple Silicon)**, **Windows (x64)** and **Linux (x64)**.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Development](#development)
- [Testing](#testing)
- [Project Layout](#project-layout)
- [Releasing](#releasing)
- [Where Your Data Lives](#where-your-data-lives)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

---

## Features

### Connections

| Protocol | Highlights |
| --- | --- |
| **SSH** | Password, key and keyboard-interactive (MFA) auth · jump hosts · known_hosts management · auto-reconnect with `tmux`/`screen` session persistence · local (`-L`), remote (`-R`) and dynamic SOCKS5 (`-D`) tunnels |
| **SFTP / SCP** | Built-in remote file manager with drag-and-drop transfers and progress tracking |
| **Serial** | Auto device enumeration · baud rate, data bits, parity, stop bits, flow control · presets · live line-parameter changes, BREAK, DTR/RTS and modem-status lines without reconnecting |
| **Network serial (RFC 2217)** | Talk to serial device servers (ser2net, Moxa NPort, Digi, Lantronix); line settings are actually pushed to the device server, with graceful fallback to a plain byte pipe |
| **Raw TCP** | A bare byte pipe to `host:port` for device servers that offer nothing else |
| **Telnet** | Stateful IAC negotiation with terminal type, window size and correct IAC escaping |
| **Local shell** | Any local shell (zsh, bash, PowerShell, Git Bash, cmd, custom path) |

Inline **Zmodem** (`rz`/`sz`) works across local, SSH, Telnet and serial sessions.

### Terminal

- **Tabs and split panes** — drag-to-reorder tabs, rename, NATO-alphabet suffixes for duplicates, binary-tree split layouts, workspace restore on relaunch.
- **Shell integration** — OSC 133 command markers with exit status, jump between commands, rerun or copy a command.
- **Input toolbar** — quick buttons grouped per toolbar and scoped to hosts or bookmark groups, a multi-line input box with history, and a command palette (`Ctrl/Cmd+Shift+P`).
- **Session logging** — per-session logs with filename templates (host, date, time, etc.).
- **Themes** — built-in presets, terminal and UI theme derived together (follow-terminal / light / dark).
- **Rendering** — Xterm.js with the WebGL addon, Unicode 11, clickable links, search.
- **Localization** — English and Simplified Chinese UI.

### Bookmarks

- Nested groups, quick search, right-click management.
- Import from OpenSSH `config` and PuTTY sessions.
- **Bookmark sharing** — export a group as a credential-free share file or a short **share code** (end-to-end encrypted, expiring, revocable; recipients need no account).
- Import preview with per-entry add / update / skip and a trust gate that strips post-login commands, auto-login answers and jump-host credentials from untrusted files by default.

### Security

- Secrets are stored separately from bookmark metadata and encrypted with **AES-256-GCM + Argon2id**, optionally behind a master password (with opt-in OS keychain unlock on macOS and Windows).
- AI API keys never enter `settings.json`, exports or cloud sync.
- Credentials never leave the machine in share files or share codes.

### Live Sync (optional, needs an AuraXLab account)

One menu, four peers, all end-to-end encrypted where a remote party is involved:

| Member | Who is on the other side |
| --- | --- |
| **Sync** | Your other AuraTerm installs — bookmarks, settings and known_hosts follow your account; saved credentials are wrapped under your master password before upload so the server cannot read them. Automatic sync runs on launch, after bookmark edits and every 30 minutes. |
| **Live Console** | Your browser — watch or type into a session from [auraxlab.com/console](https://auraxlab.com/console). |
| **Live Share** | Someone else — hand out a one-time share code, choose read-only or read-write, approve control requests. |
| **Live Relay** | Your own other machine — mirror a session from another device on your account, request control, or (opt-in) open a new local shell / serial / SSH session on it using its own bookmarks. Off by default; the target machine always has the final say. |

### AI Assistant (optional)

A side panel that streams answers from the Anthropic Messages API or any OpenAI-compatible endpoint (DeepSeek, Kimi, Ollama, …). Suggested commands can be copied or filled into the prompt — nothing runs without you pressing Enter.

### Platform integration

- Custom titlebar with native window controls.
- Windows: "Open in AuraTerm" Explorer context menu, NSIS installer and Microsoft Store (MSIX) packages.
- Window state, tabs and pane layout are restored on launch.

---

## Installation

Pick whichever channel suits you:

- **Website** — [auraxlab.com/download/auraterm/latest](https://auraxlab.com/download/auraterm/latest) (macOS `.dmg`, Windows `.exe`, Linux packages).
- **GitHub Releases** — [github.com/Aura-X-Labs/AuraTerm/releases](https://github.com/Aura-X-Labs/AuraTerm/releases).
- **Microsoft Store** — [apps.microsoft.com/detail/9P6B6G5QGGWT](https://apps.microsoft.com/detail/9P6B6G5QGGWT).

Platform notes:

- **macOS**: Apple Silicon only. Intel builds are no longer produced.
- **Linux**: needs WebKitGTK 4.1 at runtime (`libwebkit2gtk-4.1`), which most current distributions ship.

---

## Development

### Prerequisites

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | stable | via [rustup](https://rustup.rs/) |
| Node.js | 20 (18+ works) | matches CI |
| Python 3 | any recent | used by the version-sync and release scripts |

On Ubuntu/Debian also install the Tauri system libraries:

```bash
sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libudev-dev
```

### Run it

```bash
git clone https://github.com/Aura-X-Labs/AuraTerm.git
cd AuraTerm
npm install
npm run tauri dev        # hot-reloading dev build (or: make run)
```

The Vite dev server binds **port 1420** and fails if it is taken.

> Always go through the `npm run tauri …` wrappers rather than the bare `tauri` CLI. The wrapper runs `scripts/sync_version.py` first, which propagates the version in `package.json` to `Cargo.toml`, `tauri.conf.json` and the lockfile.

### Build

```bash
npm run tauri build      # installers land in src-tauri/target/release/bundle/
make build               # same, with timing and platform-appropriate bundle targets
```

### Everyday checks

```bash
npm run build                          # vue-tsc type-check + Vite build
cd src-tauri && cargo check            # Rust compile check
make update                            # bump npm + cargo deps within current ranges
make clean                             # remove src-tauri/target and dist
```

---

## Testing

| Suite | Command | Covers |
| --- | --- | --- |
| Frontend (Vitest) | `npm test` | composables, bookmarks, cloud sync, Live Sync status, terminal behaviour (`src/__tests__/`) |
| Rust | `cd src-tauri && cargo test --bin auraterm` | settings round-trips, SSH reconnect state machine, known_hosts parsing, RFC 2217, E2EE, relay admission — in `mod tests` next to the code |
| Release scripts | `make test-scripts` | the Python scripts that write into the website checkout |

CI (`.github/workflows/ci.yml`) runs all three plus `cargo check` and full Tauri builds on Ubuntu, macOS arm64 and Windows for every push to `main`/`dev` and every PR.

Anything that touches a live PTY, real SSH host or physical serial port is still verified by hand with `npm run tauri dev`. A tiny RFC 2217 server for local testing is included at `scripts/rfc2217_test_server.py`.

---

## Project Layout

```
src/                      Vue 3 + TypeScript frontend
  App.vue                 tabs, dialogs, pane orchestration (coordinator, not logic dump)
  TerminalComponent.vue   Xterm.js instance, keyboard/mouse handling
  composables/            shared behaviour (tabs, search, menus, session IPC, auto-sync, tunnels)
  usePaneLayout.ts        split-pane tree
  settings.ts / types.ts  AppSettings, theme derivation, shared types
  i18n/locales/           en, zh-CN
  __tests__/              Vitest suites
src-tauri/                Rust + Tauri 2 backend
  src/main.rs             PTY, window management, command registration
  src/ssh/                sessions, SFTP/SCP transfer, port forwarding, known_hosts
  src/serial*.rs, rfc2217.rs, telnet.rs
  src/encryption.rs, keychain.rs, e2ee.rs, pake.rs
  src/cloud_sync.rs, cloud_bridge.rs, assist_*.rs, relay_*.rs, remote_tab.rs
  src/ai.rs               Anthropic / OpenAI-compatible streaming
  capabilities/           Tauri permission manifest
scripts/                  version sync, site sync, MSIX packaging, RFC 2217 test server
Changelog.md              release notes (Chinese), mirrored to the website on release
```

Frontend and backend talk over Tauri IPC: the frontend `invoke`s commands registered in `src-tauri/src/main.rs`, and Rust emits events such as `pty-output` and `pty-exit`.

---

## Releasing

1. Bump `version` in `package.json` only; the sync script propagates it.
2. Add a `## x.y.z` section to `Changelog.md`.
3. Merge to `main`, then push a `vX.Y.Z` tag. `.github/workflows/release.yml` builds Linux, macOS arm64 and Windows bundles (plus MSIX) and publishes the GitHub release.
4. `make release` copies the changelog and version pin into the neighbouring AuraXLabs website checkout (override the path with `AURAXLABS_DIR`).

Windows-specific variants:

```bash
npm run tauri:store             # Microsoft Store build
npm run package:msix            # MSIX package
npm run package:msixupload      # Store upload bundle
npm run release:windows         # full signed release pipeline
```

Code-signing and Store submission details live in `docs/Windows-Release.md` (see [Documentation](#documentation)).

---

## Where Your Data Lives

Everything is stored in the platform's app config directory (`~/Library/Application Support/com.auraxlab.auraterm`, `%APPDATA%\com.auraxlab.auraterm`, `~/.config/com.auraxlab.auraterm`):

- `settings.json` — settings, theme, workspace state (never contains secrets).
- `connections.json` — bookmarks without credentials.
- `credentials.enc` — passwords, keys and passphrases, encrypted separately from the above.

---

## Documentation

- **User manual** — [auraxlab.com/docs](https://auraxlab.com/docs).
- **Engineering docs** — the `docs/` entry in this checkout is a symlink into the shared Aura workspace documentation repository (feature specs, design notes such as Live Sync, RFC 2217 and bookmark sharing, and `Windows-Release.md`). It is not part of this repository's history.
- **Agent guidance** — `CLAUDE.md` and `.github/copilot-instructions.md` describe conventions and non-obvious gotchas for anyone (human or AI) editing the code.

---

## Contributing

Bug reports, feature ideas and pull requests are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

1. Fork and branch from `main`.
2. Make the change and add tests where the suites above apply.
3. Run `npm run build`, `npm test` and `cd src-tauri && cargo check && cargo test --bin auraterm`.
4. Write commit messages in English, imperative mood (`Fix: …`, `Feat: …`).
5. Open a pull request.

---

## License

AuraTerm is free software, released under the **GNU General Public License v3.0 or later** (`GPL-3.0-or-later`). See [LICENSE](LICENSE) for the full text.

Copyright (c) 2026 Aura-X-Labs.

Releases up to and including 0.3.5 were published under the MIT License; those versions remain available under MIT. Everything from 0.3.6 onward is GPL-3.0-or-later. Third-party components keep their own licenses (MIT, Apache-2.0, ISC, BSD, MPL-2.0), all of which are compatible with GPLv3.
