# AuraTerm: 基于 Tauri 的 TeraTerm 现代化重构设计与计划

## 1. 项目背景与目标 (Background & Objectives)
TeraTerm 是一款历史悠久且功能强大的终端模拟器（支持 SSH、Telnet、串口等），但其基于原生 Windows API 的 UI 已经显得陈旧，且仅支持 Windows 平台。
**AuraTerm** 旨在利用 **Tauri** 框架（Rust + Web 前端技术）重新实现 TeraTerm 的所有核心功能，达成以下目标：
1. **跨平台支持**：支持 Windows, macOS, Linux。
2. **现代化 UI/UX**：提供标签页、分屏、主题定制等现代终端体验。
3. **高性能与低资源占用**：利用 Rust 处理底层网络、串口通信和高密集计算，前端仅负责渲染。
4. **完全兼容**：兼容 TeraTerm 的核心特性（如 TTL 宏脚本、ZMODEM/YMODEM 传输、丰富的编码支持）。

## 2. 架构设计 (Architecture Design)

### 2.1 技术栈选型
* **前端 (UI 层)**: Vue 3 + TypeScript。
* **终端渲染引擎**: Xterm.js (结合 WebGL 插件实现高性能渲染)。
* **后端 (Core 层)**: Rust (Tauri 核心)。
* **进程间通信 (IPC)**: Tauri IPC (Commands & Events)。

### 2.2 核心模块划分
#### 后端 (Rust)
1. **Connection Manager (连接管理器)**:
   * **SSH2**: 基于 ussh\ 或 \ssh2-rs\ 实现 SSH 客户端（密码、公钥、键盘交互认证）。
   * **Telnet**: 实现 Telnet 协议状态机。
   * **Serial Port (串口)**: 基于 \serialport-rs\ 实现本地串口通信。
2. **File Transfer (文件传输引擎)**:
   * 实现 ZMODEM, YMODEM, XMODEM, Kermit 协议的 Rust 解析器。
   * 实现 SCP / SFTP 客户端。
3. **Macro Engine (TTL 宏引擎)**:
   * **解析器**: 使用 om\ 或 \pest\ 编写 TeraTerm Language (TTL) 的语法解析器。
   * **执行器**: 在 Rust 中实现 TTL 的运行时环境，通过 IPC 驱动前端 UI 或直接操作连接。
4. **Config & Session (配置与会话管理)**:
   * 兼容解析 \TERATERM.INI\ 文件。
   * 提供新的 JSON/TOML 格式配置，支持多 Profile 管理。

#### 前端 (Web)
1. **Terminal View (终端视图)**: 封装 Xterm.js，处理输入输出流、字体、颜色主题。
2. **Workspace Manager (工作区管理)**: 多标签页 (Tabs)、分屏 (Split Panes)、窗口停靠。
3. **Bookmark Sidebar (快捷连接侧边栏)**: 保存并展示历史 SSH/Telnet/Serial 连接，双击一键重连。
4. **UI Components (交互组件)**: 连接对话框（含"保存连接"选项）、设置面板、文件传输进度条、宏调试器。

## 3. 核心难点与解决方案 (Challenges & Solutions)
1. **TTL 宏脚本的完全兼容**:
   * *难点*: TTL 包含大量与 Windows API 强绑定的命令（如 DDE、窗口控制）。
   * *方案*: 抽象出一套跨平台的 Window/System API 接口。对于无法跨平台的特性，提供空实现或警告；核心的自动化交互（\wait\, \send\, \connect\）在 Rust 核心层实现。
2. **文件传输协议 (ZMODEM 等) 的实现**:
   * *难点*: 现代库中缺乏高质量的 Rust ZMODEM 实现。
   * *方案*: 可能需要参考 C 语言源码（如 lrzsz 或 TeraTerm 源码）在 Rust 中进行安全重写，并与 Xterm.js 的数据流进行拦截和桥接。
3. **终端编码 (Encoding) 支持**:
   * *难点*: TeraTerm 支持大量日文及其他遗留编码（Shift-JIS, EUC-JP 等）。
   * *方案*: 使用 Rust 的 \encoding_rs\ 库在后端进行统一的字节流编解码，前端 Xterm.js 统一接收 UTF-8。

## 4. 快捷连接（Bookmark）功能设计

### 4.1 数据模型

保存的连接信息存储在独立的 `connections.json` 文件中（与 `settings.json` 分离）：

```json
[
  {
    "id": "uuid-v4",
    "name": "My Server",
    "host": "10.127.120.163",
    "port": 22,
    "user": "bill",
    "authType": "password",
    "password": "（明文，未来可改为加密存储）",
    "privateKey": null,
    "createdAt": 1709000000000,
    "lastUsed": 1709100000000
  }
]
```

### 4.2 后端命令（Rust）

| 命令 | 入参 | 返回 | 说明 |
|------|------|------|------|
| `get_connections` | — | `Vec<SavedConnection>` | 读取全部已保存连接，按 `lastUsed` 降序 |
| `save_connection` | `SavedConnection` | `String`（id） | 新增或更新（同 id 覆盖） |
| `delete_connection` | `id: String` | `()` | 删除指定连接 |

### 4.3 UI 布局

```
┌─────────────────────────────────────────────────────┐
│                    Titlebar                          │
├────────────────────────────────────────────────────-┤
│  [tab1] [tab2] [+] [⊞] [⚙]               [☰]     │
├──────────────────────────────────────────────────────┤
│ ┌──────────┐ ┌──────────────────────────────────┐   │
│ │ Bookmark │ │                                  │   │
│ │ Sidebar  │ │       Terminal View               │   │
│ │ -------- │ │                                  │   │
│ │ ▶ server1│ │                                  │   │
│ │ ▶ server2│ │                                  │   │
│ │          │ │                                  │   │
│ └──────────┘ └──────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

- 侧边栏默认**收起**，点击 Tab Bar 上的书签图标（🔖）展开/收起。
- 侧边栏宽度 200px，不可调整（初版）。
- 每个连接条目显示：🖥 图标 + 名称（`name`）+ 副标题（`user@host:port`）。
- **双击**条目 → 打开新标签页并立即建立 SSH 连接。
- **右键菜单**：编辑名称、删除连接。
- 连接建立成功后，若该 `host+port+user` 组合未保存过，**自动在侧边栏底部显示"保存此连接"提示条**；或在 ConnectDialog 中内联提供"保存连接"复选框（默认勾选）。

### 4.4 安全说明

当前版本密码以**明文**存储在用户配置目录的 `connections.json` 中。后续版本计划：
- macOS: 使用 **Keychain** 存储凭据。
- Windows: 使用 **Windows Credential Manager (DPAPI)** 加密。
- Linux: 使用 **libsecret / Secret Service API**。

---

## 5. 实施计划 (Implementation Plan)

项目分为 5 个主要阶段（Phase），采用敏捷迭代方式推进：

### Phase 0: 已完成功能（基础框架）
* Tauri + Vue/TypeScript 项目初始化。 ✅
* Xterm.js PTY 绑定（本地 Shell）。 ✅
* 多标签页 UI 框架。 ✅
* 基础设置持久化（JSON）。 ✅
* SSH2 连接（密码认证）。 ✅
* SSH 认证失败密码重试 overlay。 ✅
* 快捷连接（Bookmark）侧边栏。 ✅

### Phase 1: 基础设施与基础终端 (第 1-2 个月)
* **目标**: 搭建 Tauri 框架，实现基础的本地终端和 UI 骨架。
* **任务**:
   * 初始化 Tauri + Vue 项目。 --Done
  * 集成 Xterm.js 并实现基础的 PTY 绑定（本地 Shell 测试）。 --Done
  * 设计并实现多标签页 (Tab) UI 框架。 --Done
  * 实现基础的设置持久化（JSON）。 --Done

### Phase 2: 核心通信协议接入 (第 3-4 个月)
* **目标**: 实现 SSH2、Telnet 和串口通信。
* **任务**:
  * Rust 后端集成 \ssh2-rs\，实现 SSH 登录、数据收发、HostKey 验证。 --Done
  * 实现 Telnet 协议解析。 --Done
   * 集成 \serialport-rs\，实现串口设备的枚举和连接。 --Done
  * 前端实现新建连接对话框（支持 SSH/Telnet/Serial 切换）。 --Done
   * 实现连接状态反馈（连接中、成功、失败）。 --Done

### Phase 2.5: 快捷连接增强
* SSH 公钥认证支持（`userauth_pubkey_memory`）。 --Done
* Bookmark 侧边栏支持分组/文件夹。 --Done
* 连接凭据加密存储（Keychain / DPAPI）。 --Done

### Phase 3: 文件传输与高级终端特性 (第 5-6 个月)
* **目标**: 补齐 TeraTerm 的特色文件传输功能。
* **任务**:
  * 实现 SFTP/SCP 的可视化文件管理器。 --Done
  * 在 Rust 中实现 ZMODEM/YMODEM 协议状态机。
  * 实现终端数据流拦截，自动触发 ZMODEM 接收弹窗。
  * 完善 Xterm.js 的 VT100/VT200 兼容性测试。

### Phase 4: TTL 宏引擎与兼容性 (第 7-8 个月)
* **目标**: 兼容 TeraTerm 的自动化脚本能力。
* **任务**:
  * 编写 TTL 词法和语法解析器。
  * 实现 TTL 运行时（支持变量、循环、条件判断）。
  * 实现核心 TTL 命令（\connect\, \sendln\, \wait\, \waitregex\ 等）。
  * 支持导入和解析旧版 \TERATERM.INI\。

### Phase 5: 优化、测试与发布 (第 9-10 个月)
* **目标**: 达到生产可用状态。
* **任务**:
  * 跨平台打包与 CI/CD 配置（Windows .msi, macOS .dmg, Linux .deb/AppImage）。
  * 性能调优（降低 CPU 和内存占用，优化大段文本输出的渲染）。
  * 国际化 (i18n) 支持。
  * 发布 1.0 Beta 版本并收集社区反馈。
