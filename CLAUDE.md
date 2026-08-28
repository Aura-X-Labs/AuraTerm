# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

AuraTerm is a modern, cross-platform terminal emulator built with **Tauri v2** (Rust backend) and **Vue 3 + TypeScript** (frontend). It supports SSH2 (with SFTP), Serial Port, Telnet, and Local Shell connections, with tab management, split-pane layouts, session logging, and a WebGL-accelerated Xterm.js renderer.

---

## Build, Lint & Type-Check Commands

### Frontend (Vue / TypeScript)
```bash
npm install            # Install all dependencies
npm run build          # Vue TSC type-check + Vite production build (use to validate TS/Vue changes)
```

### Rust / Tauri backend
```bash
cd src-tauri && cargo check    # Validate Rust changes without a full build
cd src-tauri && cargo build    # Compile the Rust backend
```

### Full App (Development & Production)
```bash
npm run tauri dev          # Start dev server (runs `npm run dev` + Tauri in watch mode)
npm run tauri build        # Production build + platform installer bundle
make build                 # Equivalent to `npm run tauri build` with elapsed-time reporting
```

> **Important:** Always use the `npm run tauri …` wrappers instead of the bare `tauri` CLI — the wrapper runs `scripts/sync_version.py` first, which keeps version numbers in sync across `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `package-lock.json`.

### Windows-specific release variants
```bash
npm run tauri:store                # Microsoft Store build (no-bundle + microsoftstore config)
npm run package:msix               # Package MSIX installer
npm run package:msixupload         # Package MSIX upload bundle
```

### Utility
```bash
make clean             # Remove src-tauri/target and dist directories
make release           # Update releases/auraterm-releases.json metadata from built artifacts
                       # and sync Changelog.md into ../AuraXLabs (site changelog page);
                       # override the site checkout location with AURAXLABS_DIR
make upload            # release + upload artifacts
npm run sync:version   # Manually sync version across all config files
```

---

## Testing

There is **no automated test suite** in this project. Verification is done through:

1. **TypeScript type-check:** `npm run build` — runs `vue-tsc --noEmit && vite build`
2. **Rust compile check:** `cd src-tauri && cargo check`
3. **Interactive integration testing:** `npm run tauri dev`

The CI pipeline (`.github/workflows/ci.yml`) runs `npm run build` as the lint/type-check step and then performs full Tauri builds across all platforms (Ubuntu, macOS arm64, Windows — macOS x64/Intel is no longer supported).

---

## Architecture Overview

AuraTerm follows the standard Tauri architecture: a **Rust backend** that manages system-level operations and emits events, and a **Vue 3 frontend** that consumes those events to render terminal UI.

```
┌──────────────────────────────────────────────────────┐
│  Frontend (src/)  — Vue 3 + TypeScript + Xterm.js    │
│                                                       │
│  App.vue ── orchestrates tabs, dialogs, pane layout  │
│  TerminalComponent.vue ── Xterm.js render + events   │
│  composables/ ── shared reusable logic               │
│  usePaneLayout.ts ── split-pane tree management      │
│  settings.ts ── AppSettings type + defaults          │
│  types.ts ── shared TypeScript types                 │
│  logging.ts ── log filename template rendering       │
└─────────────────────┬────────────────────────────────┘
                      │  Tauri IPC (invoke / events)
┌─────────────────────▼────────────────────────────────┐
│  Backend (src-tauri/src/)  — Rust + Tauri v2         │
│                                                       │
│  main.rs ── PTY (local shell), window management,    │
│             Tauri command registration, app state     │
│  ssh.rs ── SSH2 sessions (russh), SFTP, MFA, reconnect│
│  serial.rs ── Serial port sessions (serialport-rs)   │
│  telnet.rs ── Raw TCP Telnet sessions (tokio)        │
│  connections.rs ── Saved connections CRUD (JSON)     │
│  settings.rs ── App settings persistence (JSON)      │
└──────────────────────────────────────────────────────┘
```

### Key Event Flow
- Rust emits `pty-output` and `pty-exit` events to the frontend via `AppHandle::emit()`
- Frontend invokes Tauri commands (e.g., `write_ssh_pty_input`, `start_pty`) via `@tauri-apps/api/core`'s `invoke()`
- Menu actions are dispatched as Tauri events (`menu-new-ssh`, `menu-split-right`, etc.) and handled in `useAppEventListeners.ts`

---

## Key Directories & Files

| Path | Purpose |
|------|---------|
| `src/` | All Vue 3 frontend source |
| `src/App.vue` | Root component: tab bar, dialogs, pane layout orchestration |
| `src/TerminalComponent.vue` | Xterm.js terminal instance, keyboard/mouse events |
| `src/composables/` | Shared Vue composables |
| `src/composables/useAppEventListeners.ts` | Tauri window/menu event listener registration |
| `src/composables/useTerminalSearch.ts` | Xterm search addon integration |
| `src/composables/useTerminalSessionCommands.ts` | IPC wrappers for all session types |
| `src/composables/useWorkspacePersistence.ts` | Debounced workspace state save |
| `src/usePaneLayout.ts` | Binary-tree split-pane layout engine |
| `src/settings.ts` | `AppSettings` interface, defaults, theme derivation |
| `src/types.ts` | Shared types: `SessionConfig`, `SshConfig`, `SavedConnection`, etc. |
| `src/logging.ts` | Log filename template renderer |
| `src/main.ts` | Vue app entry point; fetches startup dir via Tauri |
| `src-tauri/src/` | All Rust backend source |
| `src-tauri/src/main.rs` | PTY management, window handling, command registration |
| `src-tauri/src/ssh.rs` | Full SSH2 implementation (auth, pty, SFTP, reconnect) |
| `src-tauri/src/serial.rs` | Serial port connection handling |
| `src-tauri/src/telnet.rs` | Async Telnet session (tokio TCP) |
| `src-tauri/src/connections.rs` | Saved connections CRUD (stored in app config dir as `connections.json`) |
| `src-tauri/src/settings.rs` | App settings persistence (stored as `settings.json`) |
| `src-tauri/tauri.conf.json` | Tauri app config (window, bundle, build commands) |
| `src-tauri/capabilities/default.json` | Tauri permission capabilities |
| `scripts/sync_version.py` | Version sync utility (package.json → tauri.conf.json, Cargo.toml) |
| `scripts/release.py` | Update `releases/auraterm-releases.json` from build artifacts |
| `.github/workflows/ci.yml` | CI: type-check + cross-platform builds |
| `.github/workflows/release.yml` | Release: triggered on `v*` tags |
| `docs/` (symlinked) | Project documentation (Windows-Release.md, features, etc.) |

---

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Frontend framework | Vue 3 (Composition API, `<script setup>`) |
| Frontend language | TypeScript 5 |
| Terminal renderer | Xterm.js 5.x (`@xterm/xterm`) with WebGL addon |
| Xterm addons | `addon-fit`, `addon-search`, `addon-unicode11`, `addon-webgl` |
| Build tool | Vite 7 + `@vitejs/plugin-vue` |
| Desktop runtime | Tauri v2 |
| Backend language | Rust (edition 2021) |
| SSH library | `russh` 0.62 (keys via `russh::keys`) + `russh-sftp` |
| Serial port | `serialport` 4.8 |
| PTY (local shell) | `portable-pty` 0.8 |
| Async runtime | Tokio (multi-thread, full features) |
| Serialization | `serde` + `serde_json` |
| OS plugin | `tauri-plugin-os` |

---

## Conventions & Important Notes

### Commit Messages
All commit messages **must be in English** and follow imperative mood format:
- Short summary (≤72 chars): `Fix: resolve SSH timeout`, `Feat: add serial port auto-detect`
- Optionally followed by a blank line and detailed description

### Code Organization
- **Protocol-specific logic stays in dedicated Rust modules**: `ssh.rs`, `serial.rs`, `telnet.rs`
- **All Tauri commands are registered** in `src-tauri/src/main.rs` via `tauri::generate_handler![]`
- **Frontend composables** in `src/composables/` handle shared behavior; avoid duplicating logic in components
- **`App.vue`** is the orchestrator; keep it as a coordinator, not a logic dumping ground

### Specific Gotchas
- **Tab drag-and-drop** uses pointer/mouse-driven DOM hit testing — **not** native HTML5 drag-and-drop
- **SSH keyboard input** is batched on the frontend before invoking Tauri commands (not per-character)
- **Window minimize/maximize/close** are handled via Rust Tauri commands (custom titlebar, `decorations: false`)
- **UI theme** is derived via `settings.uiThemeMode` and `deriveUiTheme(theme, uiThemeMode)` — three modes: `follow-terminal`, `light`, `dark`
- **Screen/tmux reconnect**: keep TERM selection dynamic; do not assume a specific terminfo entry exists on the remote
- **Xterm addon versions**: search is tied to Xterm 5.x — exercise caution when upgrading addons
- **Never use `window.confirm` / `window.alert` / `window.prompt`** — they are silent no-ops in the macOS WebView (wry implements no WKUIDelegate JS-dialog handlers; `confirm` always returns `false`). Use `confirmDialog`/`alertDialog` from `src/nativeDialogs.ts` (tauri-plugin-dialog) and `promptText` from `src/promptDialog.ts` (in-app modal hosted by `PromptDialogHost.vue` in App.vue)
- **Version bumping**: only update `package.json` version; the sync script propagates it everywhere else. Run `npm run sync:version` (or just use `npm run tauri …` which runs it automatically)

### Data Persistence
App config and connections are stored in the **Tauri app config directory** (platform-specific):
- `settings.json` — all app settings (theme, font, scroll, shell, log paths, workspace state)
- `connections.json` — saved SSH/Serial/Telnet bookmarks

### Linux Build Dependencies
When building on Ubuntu/Debian, the following system packages are required:
```bash
sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libudev-dev
```

---

## Configuration & Environment Setup

### Prerequisites
1. **Rust** (stable toolchain) — install via [rustup](https://rustup.rs/)
2. **Node.js** v18+ (v20 recommended for CI parity)
3. **Python 3** — required for `scripts/sync_version.py` and `scripts/release.py`
4. **Tauri CLI** — installed as a dev dependency via npm (`@tauri-apps/cli`)

### Windows MSIX builds
MSIX packaging reads optional environment variables for store metadata:
- `AURATERM_MSIX_PACKAGE_NAME`
- `AURATERM_MSIX_PUBLISHER`
- `AURATERM_MSIX_PUBLISHER_DISPLAY_NAME`
- `AURATERM_MSIX_DISPLAY_NAME`
- `AURATERM_MSIX_DESCRIPTION`
- `AURATERM_MSIX_MIN_VERSION`
- `AURATERM_MSIX_MAX_VERSION_TESTED`

See `docs/Windows-Release.md` for full Windows release and code-signing instructions.

### Vite dev server
Vite is configured (`vite.config.ts`) to run on **port 1420** (strict — will fail if occupied). Tauri's `devUrl` points to `http://localhost:1420`.
