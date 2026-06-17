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
| 5 | 拆分 App.vue | ⬜ 待处理 |
| 6 | 统一 stream_pump | 🟡 部分完成：UTF-8 解码已抽到 `util::Utf8StreamDecoder` 共享；session 注册表与读取循环的统一仍待做 |
| 7 | per-session 事件/Channel | ⬜ 待处理 |
| 8 | logBuffer 优化 | ⬜ 待处理 |
| 9 | get_connections 缓存 | ⬜ 待处理 |
| 10 | ssh-debug 日志门控 | ⬜ 待处理 |
| 11 | 补单测 | 🟡 部分完成：新增 `util` 模块 6 个 UTF-8 解码单测 + 3 个 KDF/迁移单测 |
| 12 | App.css 拆分 | ⬜ 待处理 |
| 13 | Telnet IAC 说明 | ⬜ 待处理 |
| 14 | unwrap/expect 收敛 | ⬜ 待处理 |

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
