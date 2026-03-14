# AuraTerm 会话 Tab 全量恢复设计

## 1. 目标

为 AuraTerm 增加“重启后恢复所有历史会话 Tab”的能力，支持恢复：

1. 已打开的 Tab 列表。
2. 每个 Tab 对应的会话配置。
3. Tab 顺序与标题。
4. 当前多 pane 布局和焦点 pane。

该功能默认关闭，用户需在设置中显式开启。

## 2. 当前实现分析

### 2.1 当前会话模型

当前 `src/App.vue` 中的 `tabs` 是纯前端运行时状态：

- 每个 `tab.id` 同时也是终端/后端会话 ID。
- `handleNewLocalSession`、`handleConnectResult`、`handleBookmarkConnect` 都是直接向 `tabs` 追加新对象。
- `TerminalComponent` 在挂载时根据 `sessionId` + `session` 自行启动本地/SSH/Telnet/Serial 会话。

结论：

- 当前会话对象没有单独的“持久化层”。
- 一旦应用退出，`tabs` 会全部丢失。

### 2.2 当前启动行为

应用启动时直接初始化：

```ts
const tabs = ref<Tab[]>([{ id: "tab-0", title: "Local Shell", session: { protocol: "local" } }]);
```

也就是说当前是硬编码的“始终以一个默认 Local Shell 启动”。

结论：

- 现在不存在“根据上次会话状态决定初始 Tab 集合”的启动路径。
- 如果要支持全量恢复，启动流程必须从“先造默认 tab”改成“先尝试恢复，失败再回退默认 tab”。

### 2.3 当前已持久化内容

已持久化内容主要包括：

- `settings.json` 中的字体、主题、输入栏、窗口大小等设置。
- 最近串口配置。
- 书签连接。
- pane 布局相关状态已经开始写入 `settings.paneLayout`。

其中最大的问题是：

- `paneLayout` 单独持久化没有意义，因为它引用的是 `tabId`。
- 但重启后历史 `tabs` 并不会恢复，导致 `paneLayout` 缺少目标对象。

结论：

- “pane 布局恢复”和“Tab 全量恢复”必须设计成同一个工作区状态快照，不能拆开独立演进。

## 3. 需求定义

### 3.1 功能需求

用户开启“启动时恢复会话 Tab”后，AuraTerm 下次启动应恢复：

1. 所有上次打开的 Tab。
2. 每个 Tab 的协议与连接参数。
3. 用户改过的 Tab 标题。
4. Tab 顺序。
5. 当前 pane 布局。
6. 当前焦点 pane / 活动 tab。

### 3.2 默认行为

- 默认关闭。
- 关闭时，启动行为与现在一致：只打开默认 `Local Shell`。
- 开启后，优先尝试恢复历史工作区；仅在恢复失败或快照为空时回退到默认 `Local Shell`。

### 3.3 非目标

以下内容不纳入第一版：

1. 恢复终端滚动历史输出内容。
2. 恢复 SSH/tmux 内部 shell 的运行现场。
3. 恢复远程文件管理器当前浏览目录。
4. 跨窗口多工作区恢复。

## 4. 设计原则

1. 复用当前 `Tab` + `SessionConfig` 结构，不重建会话模型。
2. 将“恢复”视为重新创建同样的 Tab 集合，而不是恢复底层 PTY 进程本身。
3. 将 Tab 快照和 pane 布局合并为统一的“工作区快照”。
4. 开关必须显式存在，默认关闭。
5. 恢复失败时优雅回退，不影响正常启动。

## 5. 推荐方案

### 5.1 总体思路

引入一个新的“工作区快照”结构，记录：

- 上次打开的全部 Tab。
- pane 布局树。
- 焦点 pane。
- 活动 tab。

启动时流程改为：

1. 加载设置。
2. 检查 `restoreTabsOnStartup` 是否开启。
3. 若开启并存在有效工作区快照，则按快照恢复全部 Tab 与 pane 布局。
4. 若关闭或恢复失败，则回退到默认 `Local Shell`。

### 5.2 为什么不单独继续扩展 `paneLayout`

当前 `paneLayout` 只保存布局，不保存会话实体，因此无法独立工作。

如果继续沿用“单独存 `paneLayout`”的思路，会产生两个问题：

1. pane 指向的 `tabId` 在重启后没有对应 Tab。
2. 布局状态和会话状态会分散在不同地方，容易出现不一致。

因此推荐将它提升为更高层的 `workspaceState`，把 `paneLayout` 变成其中一个字段，而不是顶层单独字段。

## 6. 数据模型设计

### 6.1 设置项

在 `AppSettings` / Rust `Settings` 中新增：

```ts
restoreTabsOnStartup: boolean;
workspaceState: PersistedWorkspaceState | null;
```

默认值：

```ts
restoreTabsOnStartup: false;
workspaceState: null;
```

### 6.2 工作区快照结构

建议结构：

```ts
interface PersistedWorkspaceState {
  version: 1;
  tabs: PersistedTabSnapshot[];
  paneLayout: PaneNode;
  focusedPaneId: string | null;
  activeTabId: string | null;
}
```

其中每个 Tab 快照：

```ts
interface PersistedTabSnapshot {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}
```

这里建议直接复用当前 `Tab` 的核心字段，而不是重新造一套协议模型。

## 7. 安全与协议处理

### 7.1 Local Shell

可直接恢复：

- `protocol: "local"`
- `cwd`

若 `cwd` 不存在，回退到启动目录或默认 shell 工作目录。

### 7.2 SSH

可恢复：

- host / port / user
- password 或 privateKey
- reconnectType
- logPath

注意：

- 当前项目的书签模型本身已允许明文保存密码。
- 因此“恢复会话”在安全级别上不会比当前书签机制更弱。
- 但该功能默认关闭，减少用户无意中保存敏感连接状态的概率。

### 7.3 Telnet / Serial

同理直接恢复配置即可。

恢复失败时策略：

- Serial 端口不存在：tab 保留，但终端显示连接失败。
- SSH 认证失效：走现有认证失败重试 overlay。

## 8. 启动恢复流程

### 8.1 启动时序

推荐流程：

1. 初始化最小空状态，而不是立即固定创建默认 `Local Shell`。
2. `onMounted` 后先读取 settings。
3. 若 `restoreTabsOnStartup === true` 且 `workspaceState` 有效：
   - 恢复 `tabs`
   - 恢复 `paneLayout`
   - 恢复 `focusedPaneId`
   - 同步 `activeTabId`
   - 修正 `nextTabId` / `nextPaneId` / `nextSplitId`
4. 若恢复失败或快照为空：
   - 创建默认 `Local Shell`

### 8.2 为什么要保留原 `tab.id`

推荐直接恢复原始 `tab.id`，原因是：

1. 当前 `paneLayout` 已经引用 `tab.id`。
2. 当前 `termRefs` 也是以 `tab.id` 建索引。
3. 如果恢复后重新生成一批新 ID，就必须做一轮映射替换，复杂度更高。

因此建议：

- 工作区快照里的 `tabs[].id` 和 `paneLayout.tabId` 直接原样恢复。
- 启动后同步修正 `nextTabId`，避免后续新开 tab 冲突。

## 9. 持久化策略

### 9.1 何时写入快照

建议在以下变化后 debounce 写入：

1. `tabs` 增减。
2. Tab 标题变化。
3. pane 布局变化。
4. `focusedPaneId` 变化。
5. `activeTabId` 变化。

推荐 debounce 时间：`200ms ~ 500ms`。

### 9.2 何时强制写入

以下场景建议强制立刻保存：

1. 窗口关闭前。
2. 设置项 `restoreTabsOnStartup` 从开切到关或从关切到开时。

### 9.3 关闭开关时的行为

建议关闭该选项时：

1. 停止继续写工作区快照。
2. 清空 `workspaceState`。
3. 同步清理旧的 `paneLayout` 顶层字段，避免残留旧状态污染启动逻辑。

这能保证“关闭就是彻底不恢复”，行为更直观。

## 10. 与当前 pane 恢复的关系

### 10.1 当前问题

现在 `paneLayout` 已经开始持久化，但它依赖运行中的 `tabs`。

如果实现全量恢复，建议调整为：

- `settings.paneLayout` 废弃为兼容字段。
- 新逻辑统一从 `workspaceState.paneLayout` 读取。

### 10.2 迁移方案

第一版可以兼容：

1. 若 `workspaceState` 不存在，但 `paneLayout` 存在，则仅恢复 pane 结构，不恢复历史 tab。
2. 一旦用户开启“恢复会话 tab”并发生新的持久化，后续只写 `workspaceState`。

## 11. 设置 UI 设计

建议在 `SettingsDialog.vue` 的 Terminal 页增加一个开关：

```text
[ ] Restore session tabs on startup
    Restore all open tabs and pane layout after restarting AuraTerm.
```

行为：

- 默认未勾选。
- 用户保存设置后立即生效。
- 关闭时清空已保存工作区快照。

## 12. 失败处理

### 12.1 快照损坏

如果 `workspaceState` 无法反序列化或字段非法：

1. 打印日志。
2. 回退默认 `Local Shell`。
3. 清空损坏快照，避免下次继续失败。

### 12.2 单个会话恢复失败

如果只是某个 tab 无法连接：

- 不应中断整个工作区恢复。
- 保留该 tab，由 `TerminalComponent` 显示当前已有的失败提示。

## 13. 实施步骤

### Phase 1: 数据结构与设置开关

- 新增 `restoreTabsOnStartup`。
- 新增 `workspaceState`。
- Rust `Settings` 同步扩展。

### Phase 2: 启动恢复路径

- 将默认 `Local Shell` 的创建从顶层常量初始化，改为“恢复失败后的回退逻辑”。
- 启动时恢复 tabs + paneLayout + focusedPaneId。

### Phase 3: 持久化与清理

- 对 `tabs` / pane 状态做 debounce 保存。
- 关闭选项时清空工作区快照。

### Phase 4: 细节与迁移

- 兼容现有 `paneLayout` 顶层字段。
- 修复边界条件：空快照、损坏快照、缺失 tabId、ID 冲突。

## 14. 最终建议

基于当前项目结构，最稳妥的方案是：

1. 新增 `restoreTabsOnStartup` 开关，默认关闭。
2. 将“Tab 列表 + pane 布局 + 焦点状态”合并为一个 `workspaceState` 快照统一持久化。
3. 启动时优先恢复完整工作区，失败再回退默认 `Local Shell`。
4. 废弃“仅持久化 paneLayout”的孤立方案，避免状态分裂。

这样可以最大程度复用当前 `Tab`、`SessionConfig`、`TerminalComponent` 和 pane tree 实现，改动集中在启动路径和设置持久化层，技术风险最低。