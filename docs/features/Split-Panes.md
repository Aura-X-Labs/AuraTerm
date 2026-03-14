# AuraTerm 分栏终端设计方案

## 1. 目标

为 AuraTerm 增加“分栏显示多个终端”的能力，使用户可以在同一窗口内同时查看和操作多个终端会话，并尽量复用当前的标签页、会话生命周期和 `TerminalComponent` 实现。

该方案重点解决以下问题：

1. 同时显示多个终端内容，而不是仅显示当前活动标签页。
2. 保持现有“一个标签页对应一个会话”的模型，避免后端会话管理大改。
3. 让书签侧栏、输入栏、远程文件管理器继续工作，并明确它们与“当前焦点分栏”的关系。
4. 支持后续继续扩展为拖拽分栏、保存布局、快速切换等能力。

## 2. 现状分析

当前实现的关键结构如下：

### 2.1 会话模型

- `src/App.vue` 中的 `tabs` 是全局会话列表，每个 `tab.id` 直接作为终端会话 ID 使用。
- `TerminalComponent` 以 `session-id` 接收 `tab.id`，并在内部负责启动、重连、关闭 SSH/Telnet/Serial/Local 会话。
- 这意味着当前“标签页”本质上已经承担了“会话对象”的职责，而不只是 UI 容器。

### 2.2 布局模型

- 当前主界面为固定三段结构：`BookmarkSidebar` + `terminal-wrapper` + `RemoteFileManager`。
- `terminal-wrapper` 内部只有一个主终端显示区域，虽然会渲染全部 `TerminalComponent`，但组件通过 `isActive` 控制 `display: none`，同一时间只有一个终端可见。
- `TerminalInputBar`、串口状态栏、远程文件管理器都默认绑定“当前活动标签页”。

### 2.3 约束

- 后端 Rust 侧并不需要知道“分栏”，它只关心会话 ID 和终端尺寸。
- 真正需要变化的是前端布局层、焦点管理和每个终端实例的 `fit`/`resize` 时机。
- 由于 xterm 依赖容器尺寸，分栏后必须为每个可见终端分别做尺寸监听，而不能只在激活标签页时 `fit()`。

## 3. 设计原则

1. 保持“一个标签页 = 一个会话”的既有数据模型。
2. 将“分栏”设计为布局层能力，而不是重新定义会话层。
3. 焦点必须显式存在，所有输入、状态栏、远程文件面板都跟随“当前焦点分栏”。
4. 第一版只做窗口内分栏，不做跨窗口拖拽，不做复杂停靠系统。
5. 优先增量改造 `App.vue`，尽量少碰 Rust 后端。

## 4. 总体方案

### 4.1 核心思路

新增一棵“分栏布局树”，树叶节点绑定到现有 `tab.id`。标签页继续表示“打开的会话集合”，分栏布局则表示“哪些会话当前正在同屏显示，以及它们如何排布”。

也就是说：

- `tabs` 继续负责会话生命周期。
- `paneLayout` 负责视觉布局。
- `activeTabId` 逐步弱化为兼容字段，新增 `focusedPaneId` 作为真正的 UI 焦点来源。

### 4.2 布局树结构

建议新增如下类型：

```ts
type PaneAxis = "horizontal" | "vertical";

interface PaneLeafNode {
  kind: "leaf";
  paneId: string;
  tabId: string | null;
}

interface PaneSplitNode {
  kind: "split";
  splitId: string;
  axis: PaneAxis;
  ratio: number;
  first: PaneNode;
  second: PaneNode;
}

type PaneNode = PaneLeafNode | PaneSplitNode;
```

状态建议：

```ts
const paneLayout = ref<PaneNode>({
  kind: "leaf",
  paneId: "pane-0",
  tabId: "tab-0",
});

const focusedPaneId = ref("pane-0");
```

### 4.3 为什么不改成“标签页里再包含多个终端”

如果把顶部标签页改造成“工作区标签”，则需要同时重构：

- 会话创建逻辑
- 标签关闭逻辑
- 书签打开后的落点
- 输入栏和文件管理器对目标会话的判定

这会把“分栏功能”放大成“工作区模型重构”。当前项目更适合先保留 `tab.id === session.id`，只增加布局引用层，风险更低，也更容易分阶段上线。

## 5. 交互设计

### 5.1 第一版用户能力

第一版建议支持以下动作：

1. 将当前标签页向右分栏。
2. 将当前标签页向下分栏。
3. 在当前焦点分栏中切换显示某个已打开标签页。
4. 关闭某个分栏，自动把兄弟分栏提升。
5. 点击任意分栏后，该分栏成为焦点分栏。
6. 输入栏、状态栏、远程文件管理器全部跟随焦点分栏。

### 5.2 建议入口

建议优先做三个入口：

1. 标签页右键菜单新增：
   - `Split Right`
   - `Split Down`
   - `Move To Focused Pane`
2. 终端分栏标题条新增快捷按钮：
   - `Split Right`
   - `Split Down`
   - `Close Pane`
3. View 菜单新增：
   - `Split Right`
   - `Split Down`
   - `Close Pane`

### 5.3 推荐交互语义

- “分栏”操作默认复制当前焦点分栏中的会话到新分栏，这样用户能立刻看到第二个终端位置，然后再切换成别的标签页。
- 若用户从标签页右键发起 `Split Right`，则优先把该标签页放进新分栏。
- 同一个 `tab` 在同一时刻只允许出现在一个分栏里，避免“一个 xterm 实例同时挂两处 DOM”导致状态冲突。
- 当一个可见分栏失去焦点时，不隐藏其终端，只取消高亮和输入目标。

## 6. 界面结构建议

### 6.1 新的主布局

建议将当前 `workspace` 中部改成如下结构：

```text
workspace
├─ BookmarkSidebar（可选）
├─ terminal-workspace
│  ├─ pane-tree
│  │  ├─ pane
│  │  └─ pane
│  ├─ TerminalInputBar（全局，仅作用于 focusedPane）
│  └─ terminal-statusbar（显示 focusedPane 的串口状态）
└─ RemoteFileManager（可选，绑定 focusedPane 的 ssh 会话）
```

### 6.2 Pane 外观

每个分栏建议包含：

- 轻量标题条：显示标签名、协议、连接状态。
- 操作区：分栏、关闭、选择会话。
- 终端容器：承载 `TerminalComponent`。
- 焦点态样式：边框高亮。

示意：

```text
┌──────────────────────────────┐
│ server-a        SSH   ⋮  ×   │
├──────────────────────────────┤
│                              │
│          Terminal            │
│                              │
└──────────────────────────────┘
```

### 6.3 分割条

- `vertical` 表示左右分栏，中间渲染垂直拖拽条。
- `horizontal` 表示上下分栏，中间渲染水平拖拽条。
- `ratio` 范围建议限制在 `0.15 ~ 0.85`，避免分栏被拖成不可用大小。

## 7. 状态模型设计

### 7.1 新增状态

建议在 `App.vue` 或新的 `usePaneLayout.ts` 中维护：

```ts
const paneLayout = ref<PaneNode>(...);
const focusedPaneId = ref<string>(...);
```

辅助计算：

```ts
const visibleTabIds = computed(() => collectVisibleTabIds(paneLayout.value));
const focusedTabId = computed(() => findTabIdByPaneId(paneLayout.value, focusedPaneId.value));
const focusedTab = computed(() => tabs.value.find(tab => tab.id === focusedTabId.value));
```

### 7.2 兼容现有状态

现有 `activeTabId` 可以保留一个阶段，但语义调整为：

- 当用户点击标签页时：`activeTabId = tab.id`，并把该标签页放入 `focusedPaneId` 对应的叶子节点。
- 当用户点击某个 pane 时：更新 `focusedPaneId`，同时同步 `activeTabId = pane.tabId`。

这样现有大量依赖 `activeTabId` 的逻辑可以先继续工作，再逐步改为依赖 `focusedTabId`。

### 7.3 分栏操作 API

建议封装以下纯函数：

```ts
splitPane(root, paneId, axis, newTabId)
closePane(root, paneId)
replacePaneTab(root, paneId, tabId)
findPaneById(root, paneId)
findPaneByTabId(root, tabId)
collectLeafPanes(root)
```

这样 UI 层只负责触发动作，不直接手写树遍历。

## 8. TerminalComponent 改造建议

### 8.1 拆分 `isActive`

当前 `TerminalComponent` 的 `isActive` 同时承担三件事：

1. 控制是否显示。
2. 控制是否自动聚焦。
3. 控制窗口 resize 时是否执行 `fit()`。

分栏后这三个条件不再等价，建议拆成：

```ts
isVisible: boolean;
isFocused: boolean;
```

语义改为：

- `isVisible`: 当前会话是否在任意 pane 中展示。
- `isFocused`: 当前会话是否就是焦点 pane 内的会话。

### 8.2 resize 策略

每个可见分栏都应在以下时机执行 `fit()`：

1. pane 初次显示。
2. pane 尺寸变化。
3. 整个窗口 resize。
4. 侧边栏或远程文件面板开关导致中部区域尺寸变化。
5. 输入栏高度变化。

建议为每个 pane 容器使用 `ResizeObserver`，而不是只在激活标签页上 `setTimeout(fit, 0)`。

### 8.3 生命周期

- `TerminalComponent` 仍然保持“一会话一个组件实例”。
- 不在布局切换时销毁组件，只改变是否可见。
- 这样 SSH 重连、串口状态、日志缓存、MFA 状态都能延续，不会因调整布局丢失。

## 9. 关键行为定义

### 9.1 打开新连接

新建连接成功后：

1. 仍然创建新 `tab`。
2. 如果当前只有一个分栏，可直接替换焦点分栏为新 `tab`，保持现有体验。
3. 如果当前已存在多个分栏，建议提供设置项：
   - 默认在焦点分栏中打开
   - 默认在新分栏中打开

第一版可固定为“替换焦点分栏内容并切换焦点”。

### 9.2 关闭标签页

关闭某个标签页时：

1. 先从 `tabs` 移除。
2. 如果该标签页当前在某个 pane 中显示，则该 pane 进入空状态，或自动回填一个最近活动标签页。
3. 若关闭后 pane 为空且存在兄弟 pane，自动执行一次 `closePane` 合并布局。

建议第一版采用“自动合并布局”，避免出现大量空白分栏。

### 9.3 焦点与输入

- `TerminalInputBar` 始终发送到 `focusedTabId` 对应的终端。
- 串口状态栏显示 `focusedTabId` 对应会话的状态。
- `RemoteFileManager` 仅在 `focusedTabId` 是 SSH 会话时可用。

### 9.4 书签连接

书签双击新开连接时，默认替换焦点分栏内容即可，不必强制新建分栏。

## 10. 推荐组件拆分

为了避免 `App.vue` 继续膨胀，建议新增以下前端组件：

1. `TerminalPaneTree.vue`
   - 递归渲染 `PaneNode`
   - 负责 split bar 和 pane 容器布局
2. `TerminalPane.vue`
   - 渲染单个叶子 pane 的标题条、焦点态、工具按钮
3. `usePaneLayout.ts`
   - 放置布局树纯函数和状态操作逻辑

这样 `App.vue` 只保留：

- 会话列表管理
- 菜单与对话框
- 书签/设置/远程文件管理器开关
- 与 pane layout 的高层联动

## 11. 实施步骤

### Phase 1: 数据结构与静态分栏

- 引入 `PaneNode` 数据结构。
- 把主终端区改为递归 pane 布局容器。
- 支持一分为二的左右/上下分栏。
- 点击 pane 可切换焦点。

此阶段先不做拖拽调整比例，可先写死 50/50。

### Phase 2: 终端显示与焦点联动

- 将 `TerminalComponent` 的 `isActive` 拆为 `isVisible` / `isFocused`。
- 确保多个可见终端同时渲染。
- 输入栏、状态栏、远程文件管理器改为绑定 `focusedTabId`。

### Phase 3: 分割条拖拽调整

- 为 split node 增加拖拽调整 `ratio`。
- 接入 `ResizeObserver`，确保所有可见 xterm 正确 `fit()`。

### Phase 4: 交互增强

- 标签页右键菜单增加分栏命令。
- pane 标题条增加切换会话下拉和快捷操作。
- 可选：将布局持久化到 settings。

## 12. 非目标

以下内容不建议放在第一版：

1. 同一会话在多个 pane 中镜像显示。
2. 类似 IDE 的复杂拖拽停靠系统。
3. 每个 pane 独立的输入栏。
4. 跨窗口拖出为新窗口。
5. 后端感知 pane 概念。

## 13. 风险与注意点

### 13.1 xterm 尺寸同步

这是最容易出问题的部分。若 pane 尺寸变化后没有及时 `fit()`，就会出现：

- 文本换行错位
- 输入回显位置错误
- SSH PTY 尺寸与前端不一致

因此分栏实现是否稳定，核心不在 DOM 切分，而在尺寸监听是否可靠。

### 13.2 焦点竞争

当前代码默认“活动标签页就是唯一焦点终端”。分栏后必须统一规则：

- 鼠标点击 pane 切焦点。
- 标签点击会把该标签绑定到焦点 pane，并把该 pane 设为焦点。
- 菜单命令总是作用于焦点 pane。

### 13.3 `termRefs` 管理

当前 `termRefs` 用 `tabId -> TerminalHandle` 映射，这一点可以保留。但之后调用 `fit`、`focus`、`sendData` 时都应从 `focusedTabId` 或 `visibleTabIds` 出发，而不是简单使用 `activeTabId`。

## 14. 最终建议

综合当前代码结构，最合适的路线是：

1. 保留现有标签页即会话模型。
2. 新增前端 pane layout 树，作为显示层。
3. 用 `focusedPaneId` 和 `focusedTabId` 替代“单活动标签页”思维。
4. 把难点集中在前端布局与 xterm 尺寸同步，不触碰 Rust 会话后端。

这是当前项目中风险最低、收益最高、也最容易分阶段上线的分栏方案。