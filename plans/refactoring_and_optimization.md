# AuraTerm 重构与优化建议

> 分析日期：2026-06-17
> 范围：前端（`src/`）+ Rust 后端（`src-tauri/src/`）核心模块通读
> 用途：记录待办的重构/优化项，避免遗忘。每项含 严重度 / 位置 / 问题 / 建议方案。

---

## 整体评价

架构基础良好，无需推倒重来：

- 协议分模块清晰（`ssh.rs` / `serial.rs` / `telnet.rs` + 本地 PTY）
- 前端用 composable 拆分共享逻辑（`useTerminalSessionCommands`、`useWorkspacePersistence` 等）
- pane 布局树是纯函数实现（`usePaneLayout.ts`），易测试
- 已有原子写（`connections.rs` 的 `write_file_atomic`）、前后端 settings schema 一致性测试
- 已有 Vitest 套件（pane 布局 / logging / settings / TerminalComponent）

主要债务集中在：**几个跨多文件的正确性隐患** 和 **两个超大文件的可维护性问题**。

---

## 一、正确性缺陷（最高优先级）

### 1. ⚠️ UTF-8 跨缓冲区边界被截断（影响中文 / emoji / 制表符）

**严重度：高**

四个传输模块都对**任意读取边界**的字节直接 `String::from_utf8_lossy`：

- `src-tauri/src/main.rs:409`（本地 PTY，4096 字节缓冲）
- `src-tauri/src/ssh/mod.rs:1343`（ChannelMsg::Data）
- `src-tauri/src/ssh/mod.rs:1358`（ChannelMsg::ExtendedData）
- `src-tauri/src/serial.rs:159`
- `src-tauri/src/telnet.rs:54`

**问题**：一个 3 字节汉字若正好跨在 4096 边界（或两个 SSH data 包）上，尾部不完整字节会被 lossy 替换成 `�`。中文环境下高频触发（`cat` 大文件、`ls` 中文目录、tmux 重绘等）。

**建议方案**：维护 per-stream 的"残余字节" carry buffer——用 `std::str::from_utf8`，失败时取 `valid_up_to()` 的合法前缀发送，尾部 1–3 个不完整字节留到下次读取拼接。建议抽成共享 helper（见 §6），一处修复四处受益。

---

### 2. settings.json 非原子写入，存在损坏风险

**严重度：中高**

- 问题位置：`src-tauri/src/settings.rs:222` 用 `fs::write`（先截断再写）
- 已有方案：`src-tauri/src/connections.rs:165` 的 `write_file_atomic`（temp + fsync + rename）

**问题**：工作区状态是防抖高频持久化的（`src/composables/useWorkspacePersistence.ts`）。写入中途崩溃/断电会把 `settings.json` 截断损坏；`merge_json` 只能容忍缺字段，无法容忍半截 JSON。

**建议方案**：把 `write_file_atomic` 提到共享工具模块，`save_settings` 复用它。改动小、风险低。

---

## 二、安全相关

### 3. `zeroize` 依赖声明了却完全没用

**严重度：中**

- 位置：`Cargo.toml` 引入 `zeroize = "1.6"`，全代码库零引用
- 主密码存于 `src-tauri/src/encryption.rs:25` 的 `Mutex<Option<String>>`，每次 `get()` 都 `.clone()`
- 派生的 `[u8;32]` 密钥从不擦除

**建议方案**：用 `Zeroizing<String>` 包裹明文主密码，给派生密钥数组实现 `Drop` 擦除。这是"主密码加密凭据"特性的纵深防御缺口，应趁特性刚落地时补上。

---

### 4. KDF 实现非标准（能用但脆弱）

**严重度：低中**

- 位置：`src-tauri/src/encryption.rs:127` `derive_key_from_password`

**问题**：先跑 Argon2id，再对 **PHC 文本字符串**（`password_hash.to_string()`）做 SHA-256 取 32 字节。用哈希的文本编码而非原始输出派生密钥，迁移/兼容性脆弱。

**建议方案**：改用 `argon2.hash_password_into(pwd, salt, &mut key)` 直接填充 32 字节原始密钥，去掉多余的 SHA-256 层。
> 注意：这是破坏性变更，会使已存在的 `credentials.enc` 无法解密——需配合文件版本号升级 + 迁移逻辑。

---

## 三、架构与可维护性

### 5. `App.vue` 是 2214 行的"上帝组件"

**严重度：中**

- 位置：`src/App.vue`（1538 行脚本 + 676 行模板，约 120 个顶层函数）

**问题**：同时承担 tab 管理、菜单、终端搜索、主密码流程、字号调整、全屏、自定义标题栏、串口状态、工作区持久化协调。CLAUDE.md 明确"App.vue 应是协调者，不是逻辑堆放处"——已偏离。

**建议方案**：抽离 composable，保持 App.vue 薄：
- `useTabManager`（`src/App.vue:78-160` 标题生成/去重/重命名/关闭）
- `useAppMenus`（菜单/子菜单/右键菜单开关与 outside-click watch）
- `useTitlebarControls`（最小化/最大化/关闭/全屏）
- `useTerminalFontSize`（`src/App.vue:1301` 一组）

---

### 6. 四个传输模块的读取循环高度重复

**严重度：中**

**问题**：`main / ssh / serial / telnet` 都是同一套 `read → from_utf8_lossy → emit "pty-output"` + 退出 emit `"pty-exit"`，各自维护 `Arc<Mutex<HashMap<String, Session>>>` 和 start/write/close 三件套。

**建议方案**：抽共享 `stream_pump` helper（把 §1 的 UTF-8 carry 修复内建进去），统一 session 注册表抽象。既消除重复，又保证 UTF-8 修复一次到位不漏改。

---

### 7. 每个终端都注册全局事件监听器，按 id 过滤

**严重度：中**

- 位置：`src/TerminalComponent.vue:689` 每个实例 `listen("pty-output")` 后 `if (id !== ptyId) return`

**问题**：开 N 个 pane/tab 时，每条 `pty-output` 触发 N 个回调做字符串比较。高吞吐输出（`yes`、`cat bigfile`）下是 O(N) 浪费。

**建议方案**：改用 per-session 事件名，或 **Tauri v2 Channel**（`ipc::Channel`）——官方为高吞吐流式数据设计，只唤醒归属组件，并减少 JSON 事件封送开销。

---

## 四、性能

### 8. `logBuffer` 无限增长且是响应式的

**严重度：中**

- 位置：`src/TerminalComponent.vue:150` 声明 `ref("")`，`:694` 处 `logBuffer.value += data`

**问题**：把整段会话输出累加进**响应式 ref 字符串**，无论是否开启日志，仅为支持"保存完整日志"。长/大输出会话内存无上限增长；作为 ref 还白走 Vue 响应式 setter。

**建议方案**：(a) 改成普通 `let` 非响应式变量；(b) 上限裁剪到 scrollback 配置，或基于已落盘日志文件/xterm buffer 导出，避免内存里再存全量副本。

---

### 9. `get_connections` 每次都跑一次 16 MiB Argon2

**严重度：低**

- 位置：`src-tauri/src/connections.rs:220`

**问题**：每次调用都解密凭据库（一次 Argon2id @16MiB，约几十毫秒），在连接/侧栏刷新/密码重试等路径反复调用；`save_connection` 会 load+save = 两次 Argon2。

**建议方案**：内存缓存已解密的 store（保存时失效），避免重复 KDF。优化项，非紧急。

---

## 五、代码卫生

| # | 问题 | 位置 | 建议 | 严重度 |
|---|---|---|---|---|
| 10 | 16 处常驻 `eprintln!("[ssh-debug]…")` 在 release 也打到 stderr | `src-tauri/src/ssh/mod.rs` | 换成 `log`/`tracing` 带级别，或编译开关门控 | 低 |
| 11 | 最复杂的两个模块零单测 | `src/App.vue`、`src-tauri/src/ssh/mod.rs`（认证/重连状态机风险最高） | 给纯函数加 Rust 单测：`shell_escape`、`build_screen_attach_command`、`is_auth_error`、重连会话名解析、未来的 UTF-8 carry 解码器 | 低中 |
| 12 | `App.css` 单文件 2708 行 | `src/App.css` | 按功能拆分或迁移到组件 scoped 样式 | 低 |
| 13 | Telnet 无 IAC 协商 | `src-tauri/src/telnet.rs` | 文档标了 "raw" 属已知限制，但真实 telnetd 的 `0xFF` 选项字节会显示乱码并破坏 UTF-8 解码，UI 应注明 | 低 |
| 14 | `panic = "abort"` + 72 处 `unwrap/expect` | 全局；尤其主密码 mutex 的 `.expect("poisoned")` | abort 下会整个应用崩溃；高频路径上的 unwrap 应改 `?` | 低中 |

---

## 优先级行动清单

1. **修 UTF-8 边界截断**（§1）—— 抽共享 `stream_pump` 一次性覆盖四协议；对中文用户体感最强。
2. **settings.json 原子写**（§2）—— 复用现成 `write_file_atomic`，几乎零风险。
3. **启用 zeroize + KDF 收口**（§3 / §4）—— 加密特性刚落地，趁早补内存擦除（KDF 变更需迁移）。
4. **logBuffer 改非响应式 + 加上限**（§8）—— 小改动，防内存泄漏。
5. **拆 App.vue**（§5）+ **改 Channel/按 id 事件**（§7）—— 中期重构，提升可维护性与多 pane 吞吐。

---

## 进度跟踪

| 编号 | 项目 | 状态 |
|---|---|---|
| 1 | UTF-8 边界截断 | ✅ 已完成（2026-06-17，PR #23） |
| 2 | settings 原子写 | ✅ 已完成（2026-06-17，PR #23） |
| 3 | zeroize 擦除 | ✅ 已完成（2026-06-17） |
| 4 | KDF 收口 | ✅ 已完成（2026-06-17，含 v1→v2 透明迁移） |
| 5 | 拆分 App.vue | ✅ 已完成（2026-06-18） |
| 6 | 统一 stream_pump | ✅ 已完成（2026-06-19）：decode+emit 抽成共享 `util::emit_pty_output`/`emit_pty_exit`；session 注册表统一经评估不做（见说明） |
| 7 | per-session 事件/Channel | ✅ 已完成（2026-06-18，按 id 事件名方案） |
| 8 | logBuffer 优化 | ✅ 已完成（2026-06-17，PR #24） |
| 9 | get_connections 缓存 | ✅ 已完成（2026-06-19，KDF 结果记忆化） |
| 10 | ssh-debug 日志门控 | ✅ 已完成（2026-06-19，`debug_log!`/`warn_log!` 按 build profile 门控） |
| 11 | 补单测 | ✅ 已完成（2026-06-19）：累计 +12 单测（UTF-8 6 + KDF/迁移 4 + Telnet IAC 7 + tmux 2 + KDF 缓存 1） |
| 12 | App.css 拆分 | ✅ 已完成（2026-06-19，按功能拆成 `src/styles/` 7 文件，产物字节一致） |
| 13 | Telnet IAC | ✅ 已完成（2026-06-19，实现 IAC 过滤 + 协商拒绝，非仅文档说明） |
| 14 | unwrap/expect 收敛 | ✅ 已完成（2026-06-19）：主密码 mutex poison 恢复 + 时钟 unwrap 加固；其余均在测试或顶层 `run().expect()` |

### 已完成项实现说明（2026-06-17）

- 新增共享模块 `src-tauri/src/util.rs`：
  - `Utf8StreamDecoder`：增量 UTF-8 解码器，缓存跨读取边界的不完整多字节序列；真正非法字节仍替换为 `U+FFFD`（保持原 lossy 行为）。
  - `write_atomic`：从 `connections.rs` 提取的原子写（temp + fsync + rename）。
- §1 接入四处读取循环：`main.rs`（本地 PTY）、`serial.rs`、`telnet.rs`、`ssh/mod.rs`（Data / ExtendedData 各用独立 decoder，避免 stdout/stderr 串扰）。空字符串结果跳过 emit。
- §2 `settings.rs::save_settings` 改用 `util::write_atomic`；`connections.rs` 删除本地副本改调共享实现（移除已无用的 `std::io::Write` 导入）。
- 验证：`cargo check` 通过；`cargo test` 全部 51 项通过（含 6 项新解码测试）。
- 提交：分支 `fix/utf8-stream-decode-and-atomic-settings`，PR https://github.com/Aura-X-Labs/AuraTerm/pull/23 （fix + docs 两个 commit）。

### §3 / §4 实现说明（2026-06-17）

均在 `src-tauri/src/encryption.rs`（+ `Cargo.toml` 启用 `zeroize` 的 `derive` 特性）：

- **§3 zeroize**：
  - `MasterPasswordState` 改为 `Mutex<Option<Zeroizing<String>>>`，`get()` 返回 `Zeroizing<String>`，缓存清除/覆盖时自动擦除明文。
  - `derive_key_*` 返回 `Zeroizing<[u8;32]>`；解密明文、序列化 JSON 用 `Zeroizing<Vec<u8>>` 包裹；v1 路径的 PHC 串用后 `.zeroize()`。
  - `CredentialStore` / `StoredCredential` 派生 `ZeroizeOnDrop`（导入合并循环改用 `drain` 以避免 move out of Drop 类型）。
  - 边界：主密码经 Tauri IPC / 前端 JS 的副本不在 zeroize 覆盖范围内（纵深防御，非端到端）。
- **§4 KDF 收口 + 迁移**：
  - 新 `derive_key_v2` 用 `Argon2id::hash_password_into` 直接派生 32 字节，去掉 PHC 串 + SHA-256。
  - 保留 `derive_key_v1`（逐字节等价旧实现）仅用于解密旧文件；`derive_key(version)` 按文件头版本调度。
  - 文件头版本：`1`=legacy，`2`=current；`from_bytes` 同时接受两者。
  - **透明迁移**：`load_encrypted_credentials` 读到 v1 文件后用 v2 重新加密回写（尽力而为，不阻断读取）。
  - 备份格式加 1 字节版本前缀（`BACKUP_VERSION=2`）；旧备份串需重新导出（特性刚落地，影响面可忽略）。
  - 主密码**验证哈希**（settings.json 的 PHC 串）不受影响，无需迁移。
  - 注意：`credentials.enc` 仍用 `fs::write`（非原子），未在本次范围内——可后续做二进制版 `write_atomic`。
- 验证：`cargo check` 通过；`cargo test` 全部 54 项通过（新增 `test_derive_key_v1_and_v2_differ`、`test_derive_key_dispatch_matches_explicit_versions`、`test_v1_blob_decrypts_only_via_version_dispatch`）。

### §7 实现说明（2026-06-18，按 id 事件名方案）

采用「per-session 事件名」而非 Tauri Channel——改动面小、风险低，且直接消除 O(N) 热路径，无需改 `invoke` 签名或跨线程持有 Channel 句柄。

- 新增 `src-tauri/src/util.rs::session_event(base, id) -> "<base>:<id>"`（Tauri 2.10.3 的事件名校验允许 `:`，tab id 为 `tab-N` 全合法；新增 1 个单测）。
- 后端 25 处 per-session emit 全部改为 `app.emit(&util::session_event("pty-output", &id), payload)`，覆盖 `main.rs`(本地 PTY)/`serial.rs`/`telnet.rs`/`ssh/mod.rs`/`ssh/known_hosts.rs` 的 `pty-output`、`pty-exit`、`ssh-connected`、`serial-connected`、`ssh-mfa-prompt`、`ssh-reconnect-session-prompt`、`ssh-host-key-mismatch-prompt`。payload 仍保留 `id` 字段（信息冗余但零改动，避免动 payload 结构体）。
- 前端 `TerminalComponent.vue`：监听名改为 `` `<base>:${props.sessionId}` ``，删除原 `event.payload.id !== ptyId.value` 过滤（事件名已保证归属），保留 `!terminal` 守卫。每条高吞吐输出只唤醒归属组件，不再 O(N) 全广播 + 字符串比较。
- `ssh-transfer-progress`（SFTP 面板，`RemoteFileManager.vue`）不在范围内——非每终端扇出，保持全局。
- 测试：`TerminalComponent.test.ts` 改为向 `pty-exit:<id>` / `ssh-connected:<id>` 派发。
- 验证：`cargo check` + `cargo test`（55 项，+1 `session_event`）通过；`npm run build`（vue-tsc + vite）+ `npm test`（48 项）通过。

### §5 实现说明（2026-06-18，拆分 App.vue）

`App.vue` 2215 → 1829 行（脚本 ~1538 → ~1152 行；模板未动）。抽出四个 composable，`App.vue` 退回「协调者」角色——仅声明状态、按依赖顺序装配 composable、保留编排型 handler（如 `handleConnectResult`、`handleCloseTab`、context-menu 动作）。

- `src/composables/useTitlebarControls.ts`：独占 `appWindow` 句柄 + `isFullscreen`，含最小化/最大化/关闭/退出/全屏/拖拽/`stopDragPropagation`/`syncFullscreenState`。依赖注入 `closeOpenMenus`。
- `src/composables/useTerminalFontSize.ts`：字号增/减/重置（含 clamp + 静默持久化）。注入 `settingsRef`、`persistSettingsSilently`、`closeOpenMenus`。
- `src/composables/useAppMenus.ts`：菜单/子菜单/右键菜单/布局下拉的开关状态 + `closeOpenMenus`/`toggleMenu`/`toggleFileSubmenu`/`toggleLayoutMenu`/`handleOpenNewTabMenu` + 三个 outside-click/Escape watch。DOM 模板 ref（`menuBarRef`/`layoutMenuRef`/`tabContextMenuRef`）由组件持有并注入（`ref="…"` 绑定属组件；避免 vue-tsc `noUnusedLocals` 误报）。导出 `AppMenuId`/`FileSubmenuId`/`TabContextMenuState` 类型。
- `src/composables/useTabManager.ts`：NATO 后缀去重标题生成、内联重命名状态机、单调 tab id 计数器（`mintTabId`/`syncTabIdCounter`）。注入 `tabs`、`activeTabId`（来自 usePaneLayout）、`closeTabContextMenu`。
- 装配顺序（避免 TDZ + 满足依赖）：plain refs → `useAppMenus` → `usePaneLayout` → `useWorkspacePersistence` → `useTitlebarControls` → `useTerminalFontSize` → `useTabManager` → `useAppEventListeners`（后者构造选项对象时即读取 `syncFullscreenState`/字号 handler，故必须在其之前初始化）。
- 验证：`npm run build`（vue-tsc + vite）通过；`npm test`（48 项）通过。

### §8 实现说明（2026-06-17，随 PR #24 一起提交）

- `src/TerminalComponent.vue`：`logBuffer` 由响应式 `ref("")` 改为普通 `let` 字符串（无人渲染它，去掉每个 chunk 的响应式 setter 开销）。
- 加上限：保留最近 `SAVED_LOG_MAX_CHARS`（~4M 字符），超过 cap + 1M slack 时裁掉最旧部分（slack 避免每个 chunk 都 slice 多 MB 字符串）。
- 权衡：超长会话的"保存日志"只含最近窗口；持续落盘日志（配置了 logPath）走独立的 `pendingLogBuffer`/`appendToLog` 路径，不受影响。
- 验证：`npm run build`（vue-tsc + vite）通过；`npm test`（Vitest）48 项全过。

### §6 / §9 / §10 / §11 / §12 / §13 / §14 实现说明（2026-06-19，收尾批次）

剩余 7 项一并完成。验证：`cargo test`（68 项全过）+ `cargo check --release`（0 warning）+ `npm run build`（vue-tsc + vite）+ `npm test`（48 项）。

- **§6 stream_pump**：把"解码 + 跳过空串 + per-session emit"抽成共享 `util::emit_pty_output(app, id, decoder, chunk)` 与 `util::emit_pty_exit(app, id, msg)`，接入本地 PTY（`main.rs`）、`serial.rs`、`telnet.rs` 三个简单读取循环，消除三处重复样板。
  - **刻意不做**的部分：跨协议统一 session 注册表 / 读取循环。理由——三者 IO 模型异构（telnet 走 tokio async、serial/PTY 走阻塞线程 + 不同停止机制），且锁类型不一（PTY 用 `std::sync::Mutex`，telnet/serial 用 `tokio::sync::Mutex`）；SSH 的读取循环是多路复用状态机（Data/ExtendedData/Eof/重连/MFA/window-change），与简单循环本质不同。强行套一个泛型抽象是净负收益，故保留各自的 start/write/close 三件套，仅统一真正重复的 emit/decode 表面。
- **§9 get_connections 缓存**：在 `encryption.rs` 对 Argon2 KDF 结果做记忆化（`KDF_CACHE`，键 = `(sha256(password) 域分隔, salt, version)`）。
  - 选型：缓存 **派生密钥** 而非"解密后的凭据库"。派生密钥是 `(password, salt, version)` 的纯函数，命中**永不可能返回过期数据**（密码变→指纹变；`save` 每次写随机盐→新条目），因此**无需任何失效点**即正确；凭据明文仍每次从磁盘新鲜读取。相比缓存明文库，避免了散落在 set/change/disable/lock/import 各处的失效逻辑与漏失效风险。
  - 边界：上限 `KDF_CACHE_MAX=4`（淘汰最旧，密钥 `Zeroizing` 析构擦除）；纵深防御额外在 `lock_master_password`/`disable_master_password`/`change_master_password` 调 `clear_kdf_cache()`，使派生密钥不超出解锁会话存活。
- **§10 日志门控**：新增 `logging.rs` 两个 `#[macro_export]` 宏——`debug_log!`（仅 `debug_assertions` 输出，release 编译为丢弃 `format_args!` 的零成本块，避免"仅用于日志"的变量触发 release `unused` 警告）、`warn_log!`（任何 build 都到 stderr）。`ssh/mod.rs` 24 处诊断/状态打印（含原 `println!("Password authentication successful!")` 的认证旁路信息泄露）改 `debug_log!`；`main.rs` 窗口尺寸 trace 改 `debug_log!`、窗口边界恢复失败改 `warn_log!`；`connections.rs` 凭据库不可读改 `warn_log!`。
- **§11 补单测**：累计 +12 纯函数单测。本批新增 7 个 `util::TelnetIacFilter`（透传/IAC 转义/DO→WONT/WILL→DONT/不回应 WONT-DONT/子协商剥离/跨读取边界）+ 2 个 `build_tmux_attach_command`（附加优先 + shell 转义防注入）+ 1 个 `derive_key` 缓存一致性。
- **§12 App.css 拆分**：2724 行单文件按功能拆成 `src/styles/` 下 7 个文件（base-and-titlebar / tabs / workspace / input-bar / bookmark-sidebar / settings / overlays），仅在 section 注释边界切分、不切断任何规则；脚本校验拼回字节一致，且 `vite build` 产物 CSS 哈希不变（`index-B-FroVXd.css`）。`App.vue` 按原顺序 import 7 文件以保级联。
- **§13 Telnet IAC**：超出原"仅文档说明"——在 `util::TelnetIacFilter` 实现有状态 IAC 过滤：剥离命令序列使 `0xFF` 选项字节不再污染 UTF-8 解码/终端；对 `DO`/`WILL` 一律礼貌拒绝（回 `WONT`/`DONT`），`IAC IAC` 还原为字面 `0xFF`，子协商整段丢弃；序列可跨读取边界。`telnet.rs` 信道类型 `String`→`Vec<u8>`（协商响应含非 UTF-8 字节），reader 经过滤后用共享 emit helper 输出、协商响应经 writer 信道回发。
- **§14 unwrap/expect 收敛**：`MasterPasswordState::set`/`clear` 的 `.expect("poisoned")` 改为 `unwrap_or_else(|e| e.into_inner())` 恢复中毒锁（`panic = "abort"` 下原会整个崩溃）；`main.rs` 创建子窗口时的时钟 `unwrap()` 改 `map_or(0, …)`。其余 `unwrap/expect` 经审计均在测试模块或顶层 `tauri::Builder::run().expect(...)`（应用入口，无可恢复路径），保持不动。
