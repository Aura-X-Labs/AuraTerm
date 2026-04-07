# AuraTerm Workspace Instructions

## Scope

- Apply these instructions to the entire repository.
- Prefer linking to existing docs instead of restating detailed workflows.

## Build and Verify

- Use `npm run build` to validate Vue and TypeScript changes.
- Use `cd src-tauri && cargo check` to validate Rust and Tauri changes.
- Use `npm run tauri dev` for interactive integration testing.
- Use `npm run tauri build` for production builds; use the package scripts only for Windows release work.
- Prefer the npm Tauri wrappers over bare `tauri` commands so version sync runs first.

## Code Organization

- Frontend code lives in `src/`.
- `src/App.vue` coordinates the main layout, tabs, dialogs, and pane orchestration.
- `src/TerminalComponent.vue` owns Xterm rendering and terminal event handling.
- `src/composables/` holds shared frontend behavior.
- Backend code lives in `src-tauri/src/`.
- Keep protocol-specific logic in dedicated Rust modules such as `ssh.rs`, `serial.rs`, `telnet.rs`, `connections.rs`, and `settings.rs`.
- Register Tauri commands in `src-tauri/src/main.rs`.

## Conventions

- Use pointer or mouse-driven DOM hit testing for tab reordering instead of native HTML5 drag and drop.
- Batch SSH keystrokes on the frontend instead of invoking per character.
- Use Rust Tauri commands for custom titlebar minimize, maximize, and close actions.
- Derive UI theme through `settings.uiThemeMode` and `deriveUiTheme(theme, uiThemeMode)`.
- Keep screen reconnect TERM selection dynamic; do not assume the remote host has a specific terminfo entry.
- Be careful when changing Xterm addon versions; search functionality is currently tied to the Xterm 5.x line.

## Documentation

- Read [README.md](README.md), [CONTRIBUTING.md](CONTRIBUTING.md), and [README_CN.md](README_CN.md) for project overview and contribution expectations.
- Read [docs/Windows-Release.md](docs/Windows-Release.md) before changing Windows packaging or release behavior.
- If a workflow changes, update the linked documentation rather than duplicating the details here.