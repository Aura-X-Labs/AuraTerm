## 0.3.3

### 新增
- **书签管理页** —— 「View ▸ 书签管理…」（macOS 原生菜单栏与 Windows 自绘菜单两端一致）/ 命令面板 / 书签栏 ⚙ 按钮打开的全尺寸管理页：左栏分组树（全部 / 最近使用 / 未分组 + 派生分组树，与书签栏共享折叠态）、中栏可排序表格（名称 / 协议 / 目标 / 认证 / 分组 / 最近使用，密码认证以警告色标出）、右栏内嵌书签编辑器（设计见 `docs/plans/bookmark-manager-design.md`）
  - 多选：点击勾选框、⌘/Ctrl 点击、Shift 范围选、表头全选；选中后底部操作条可批量「移动到分组」（含新建分组）或删除，删除一律二次确认
  - 键盘：`/` 或 ⌘/Ctrl+F 聚焦搜索，↑↓ 在列表内移动，Enter 连接，Esc 先清空搜索再关闭；双击行连接
  - 分组维护：左栏「＋ 新建分组」可建立空分组（记入 `settings.json`，随云同步）；分组右键可重命名、新建子分组、解散（书签与子分组提升一级）或连同书签一并删除；把书签行拖到分组上即可移动
  - 导入导出：导出全部或所选为 AuraTerm JSON（默认不含凭据，另有一项可导出含密码与私钥的版本）；导入自动识别 OpenSSH config / PuTTY `.reg` / AuraTerm 备份，并可指定落点分组
  - 克隆：批量复制所选书签，连同凭据一起复制
  - 行右键菜单：连接 / 克隆 / 导出 / 删除；右键已勾选的行则作用于整个选区
  - 宽度：默认放宽到 `min(1440px, 96vw)`，拖右边缘可继续加宽（下限 760px，上限视窗宽度减 48px），宽度记在本机，双击边缘恢复默认
  - 主密码锁定时顶部提示凭据不可见；删除、克隆、导出凭据需要先解锁，而移动分组与重命名只改元数据，锁定状态下照常可用
- **分组改为下拉选择** —— 新建会话对话框与书签编辑器的分组字段优先列出已有分组（含 `a/b` 这类嵌套路径的中间层级），选「新建分组…」才展开输入框
- **远程协助（Remote Assist）——主机侧** —— 生成一个 12 位协助码，让对方用浏览器（`auraxlab.com/assist`）或另一台 AuraTerm 查看、并在你允许后控制当前终端会话（设计见 `docs/plans/remote-assist-design.md`）
  - 协助码 `XXXX-XXXX-XXXX`：前 4 位路由段由 AuraXLab 分配，后 8 位秘密段只存在于本机内存；双方用它跑 SPAKE2（RFC 9382，P-256）互相认证并派生端到端加密密钥——服务器与中继无法读取内容、无法冒充任何一方；错误尝试（含收到 `PAKE_B` 后放弃确认的连接）累计 3 次即锁定并结束
  - 「云服务 ▸ 远程协助…」/ 命令面板「远程协助」打开对话框：选择会话或「跟随当前标签页」、控制策略（仅查看 / 访客可申请 / 自动授予）、加入需确认、多人模式（≤3）、有效期 5–60 分钟；运行中显示大号协助码（有人加入后自动遮罩）、复制码/链接、剩余时间、访客列表（昵称/客户端/角色/会话指纹、授予/收回控制、踢出）、结束协助
  - 敲门审批：访客加入或申请控制时弹出确认（允许查看 / 允许控制 / 拒绝；60 秒不处理视为拒绝）；控制权带 fence，撤销即失效；`Ctrl/Cmd+Shift+Esc` 一键收回所有访客控制权；关闭被协助标签页即结束协助（跟随模式则自动切换）
  - 状态栏胶囊新增「协助·等待加入 / N 人观看 / N 人控制」状态，远端键入沿用既有提示
- **远程协助——访客侧**：「云服务 ▸ 加入远程协助…」/ 命令面板「加入远程协助」输入协助码或链接，即在新标签页（`assist` 类型）中接入对方 AuraTerm 的终端：无需账户，经 AuraXLab 取票、WSS 中继、SPAKE2 互证并建立端到端加密；标签页顶部横幅显示主机名/会话指纹/状态（验证中 / 等待批准 / 只读 / 控制中 / 已结束），可申请或放弃控制；只读时键入在本机丢弃，控制态键入以带 fence 的密文 `INPUT` 送达主机；终端跟随主机的 cols/rows（本地不 fit，容器内滚动）；访客标签页不进入工作区恢复、不可被再次共享
- **远程协助——续期与短链接**：对话框显示「会话将在 h:mm:ss 后自动结束」（默认 4 小时）并可「延长 1 小时」（服务端记录 `assist.extended`）；复制的链接改为短形式 `https://auraxlab.com/s#码`
- **终端尺寸转发**：前端每次 fit 后上报当前 cols/rows，Cloud Console 观看者与远程协助访客收到 `RESIZE` 并按主机真实网格渲染（取代原先写死的 80×24）

### 修复
- 书签保存失败现在会弹出原生错误对话框——此前走 `window.alert`，而它在 macOS WebView 中是静默 no-op，保存失败没有任何反馈
- 书签栏右键删除加二次确认（此前点下即删，无法撤销）
- 主密码锁定时保存书签会清空已存的密码与私钥——`get_connections` 在锁定态返回的连接不含凭据，再 `save_connection` 写回等于覆盖密文。现在这类保存会被直接拒绝并提示先解锁

### 内部
- 新增 `useBookmarkStore`：书签列表、分组折叠态与增删改/批量操作收敛为一份共享状态，书签栏与管理页共用（此前各自 `invoke`，一侧改动另一侧不刷新）
- `bookmarks.ts` 补齐 `collectGroupPaths` / `filterConnections` / `sortConnections` / `matchesScope` / `renameGroupPath` 等纯函数并纳入单测；`BookmarkEditor` 新增 `embedded` 模式供管理页内嵌复用
- 新增 Tauri 命令 `delete_connections` / `move_connections` / `rename_group` / `duplicate_connection` / `export_bookmarks`：批量操作一次读写落盘，取代前端逐条调用（主密码模式下每次 `save_connection` 都会用新盐触发一次 Argon2id 派生）；其中 `move_connections` 与 `rename_group` 只改元数据，不触碰凭据库
- `import_bookmarks` 支持 AuraTerm 导出格式：id 一律重新分配，文件内含凭据且主密码已解锁时一并写回凭据库，否则跳过并回报警告
- 新增 `bookmarkTransfer.ts`（格式嗅探 / 导出文件名 / 浏览器下载）与 `bookmarkGroups` 设置项（已加入云同步白名单）
- 新增 `BookmarkManager.test.ts`：挂载管理页覆盖列表渲染、分组筛选、批量删除、拖拽移动分组、分组重命名、空分组创建、导出与双击连接；Rust 侧补 `renamed_group_path` 与导入导出回环单测
- `assist_extend` 命令 + 主机测试（进程内假 AuraXLab 应答 `/extend`，校验截止时间更新与范围钳制）
- `cargo audit`：3 项均为传递依赖——`quick-xml 0.38`（经 `plist` ← `tauri-utils`，构建期配置解析，RUSTSEC-2026-0194/0195，需上游升到 0.41）、`rsa 0.10.0-rc`（经 `russh`，RUSTSEC-2023-0071 Marvin，无修复版本）；其余为 gtk-rs GTK3 绑定"不再维护"类告警（Tauri Linux 依赖）
- 新增 `pake.rs`（SPAKE2-P256，RFC 向量 + 跨语言 fixture 锁定）、`assist.rs`、`assist_host.rs`；E2EE 信封抽为 `e2ee.rs::PeerCipher`（方向标签进 AAD）
- 新增 `assist_client.rs`（访客端：join → 中继 → SPAKE2 → E2EE → 输出/状态事件；`assist_join/write_assist_input/assist_request_control/assist_release_control/close_assist_session`）
- `cargo test` 新增访客端集成测试（进程内假 AuraXLab + 假 relay/主机：握手、指纹一致、HELLO、只读丢弃输入、申请控制→授予→带 fence 输入；错码在确认前被识别且不发送确认）
- `cargo test` 新增主机握手集成测试（Tauri mock app + 模拟访客：正确码 → 状态/快照/输出/输入/撤销；错误码 3 次锁定；审批流程）


## 0.3.2

### 新增
- **AI 助手（可接 Anthropic / OpenAI 兼容 API）** —— 内置流式聊天面板 + 终端上下文快捷操作，密钥本地加密存储
  - 提供商任选：Anthropic Messages API 或任意 OpenAI 兼容端点（DeepSeek、Kimi、Ollama 等）；Settings ▸ AI 配置提供商 / Base URL / 模型 / 最大 token，「测试连接」探针在按钮被禁用时会说明原因（未启用助手 / 未保存密钥）
  - 聊天面板：多轮流式对话、随时取消、代码块一键复制/插入终端、token 用量显示；标签栏新增 ✨ 按钮开关面板（设置按钮同时换成真正的齿轮图标）
  - 终端上下文动作：命令面板「解释上一条命令 / 分析上一条失败命令」（基于 OSC 133 shell 集成命令块），面板顶部快捷 chips「解释 / 分析报错 / 优化命令 / 总结输出」（总结读取 xterm 当前可见视口，不依赖命令标记）；所有上下文先生成可编辑草稿、确认后才发送
  - 输入栏 Ctrl/Cmd+K：自然语言描述任务 → 生成单条命令填入输入栏供审阅——绝不自动执行，回车始终在用户手里
  - API Key 用设备本地密钥 AES-256-GCM 加密单独存放：不进凭据库、不进设置导出、不进云同步（仅 AI 配置可同步，密钥永不同步）
- **界面多语言：简体中文** —— 轻量响应式 i18n 层（en / zh-CN），语言设置支持「跟随系统 / English / 简体中文」
  - 覆盖全部前端界面：菜单/标题栏、设置各页、连接与书签、云同步、隧道、远程文件、命令面板、主密码、输入栏、About、终端浮层与各处 tooltip；macOS 原生菜单栏在切换语言时即时重建
- **新一级菜单「云服务 / Cloud」**（自绘标题栏与 macOS 原生菜单同步重构）—— 账户、同步、监控三组入口集中一处
  - 账户：未登录显示「登录…」（登录/注册），登录后变为「我的账户…」——账户中心展示用户名、服务器，以及 **Cloud Console 消耗流量（总流量 + 上行/下行 + 会话数）**，支持刷新与退出登录；设备绑定管理并入同一对话框
  - 同步：「立即同步」一键按当前 provider 全量同步（结果以浮层提示，未配置/口令未解锁时引导到同步设置）；「同步设置…」打开原云同步对话框选择同步范围
  - 监控：「Cloud Console」勾选开关 = 自动共享当前会话到云端观测；「允许远程发送 (Allow Remote Send)」默认开启、可单独关闭——关闭后所有共享钳制为只读、Rust 桥接层直接丢弃远端 INPUT（fail-closed：设置未推送前一律拒绝），既有共享会就地降级为只读
  - 原生菜单账户/云同步/自动共享项从 File 菜单迁出；登录态与两个开关的勾选状态经 `sync_cloud_menu_state` 实时重建原生菜单；修复原生菜单「AuraXLab 账户…」点击无响应（`on_menu_event` 缺转发分支，现改为 `menu-*` 通配转发）
  - 自动共享策略跟随「允许远程发送」：允许时以 read_write 共享（浏览器仍需 recent-auth + TX 租约），关闭时 read_only
  - **移除手动「共享到云端」入口**（状态栏按钮、逐会话策略对话框与命令面板项）——共享统一走「Cloud Console」单开关模型（单槽位、跟随当前会话）；随之取消的还有仅手动路径可选的「临时控制（自动到期）」策略（Rust 桥接层仍保留该策略支持）
  - **状态栏云端状态指示**：被控端实时显示 Cloud Console 链路状态——受控中（有控制者接入，红点脉冲）/ 被观看（N 个观看者，蓝）/ 云端在线 / 云端待机 / 云端重连中 / 云端离线；未绑定设备时隐藏。角色信息取自中继 `E2EE_INIT` 的 `role` 字段（观看者/控制者接入、断开即时刷新，主窗口点击胶囊可直接开关 Cloud Console）
  - **修复：活跃会话在云端网页打开即「连接已断开」**——E2EE_INIT 处理曾先把 viewer 注册进输出泵、后发 `E2EE_READY`，繁忙终端的加密输出帧会抢先到达浏览器（彼时浏览器尚无解密密钥，直接判死连接）。现改为 `E2EE_READY` + 快照发出后才注册 peer；配套 AuraXLab console.js 对 READY 之前到达的加密帧做缓冲回放（兼容旧版本 AuraTerm）
  - 登录/注册表单提取为 `AuraxlabAuthForm.vue`，云同步对话框与账户中心复用；命令面板新增「立即同步 / Cloud Console 开关 / 允许远程发送开关」，账户命令标题随登录态切换
  - 云桥 HTTP 传输改为长轮询：服务器最长 hold 25s、有帧立即返回——空闲流量降到心跳级（~2 次/分钟），新观看者仍亚秒级发现（要求 AuraXLab 中继支持 `wait` 参数）。心跳遵循服务端 connect-grant 下发的 `heartbeat_interval`（默认 60s，5–600s 钳制），HTTP 与 WebSocket 传输一致
- **Cloud Console 桥接生产化**（配合 AuraXLab 独立中继）
  - 设备身份升级为 Ed25519 签名 + 能力校验，支持凭据轮换（旧钥签新钥）；设备身份用设备本地密钥加密落盘（`console_device.enc`），启动自动恢复并以指数退避重连
  - 中继授予 ws(s):// 地址时走 WebSocket 传输（HTTP 长轮询兜底），重连时热切换传输层，长生命周期泵始终使用当前连接
  - 纯 E2EE 输出通道：移除全部非加密发送路径，终端字节只在每观看者的 E2EE 信封内离开设备——无观看者接入时不上传任何内容
  - 空闲降级：无共享 60s 后断开中继、转入 30s presence ping 的待机态，账户中心显示「在线（待机）」；发起共享按需拨号
  - 串口会话同样可云端观测（serial RX 桥）；终端事件统一经事件 broker 分发
- **账户中心「一次登录、两种凭证」** —— 登录表单直连设备绑定：勾选「同时绑定此设备到 Cloud Console」（默认开启）后，同一次密码输入先换取同步凭证、再直批设备登记（密码仅当次使用、不落盘），一步完成登录 + 绑定；两种凭证仍各自独立（sync scope 与设备身份互不可替代），合并的只是流程
  - 已登录未绑定时，绑定卡片预填账户邮箱，仅需补一次密码（服务端要求当场密码作为新鲜度证明，同步凭证被明确拒绝用于绑定新设备）；浏览器批准路径保留（应用内零密码）
  - 「本设备」区不再有独立的邮箱 + 密码表单；服务器地址上移为登录表单字段，登录与绑定共用同一地址，不再可能各指一个服务器
  - 对话框顶部新增「接入状态」总览：云同步登录态与 Cloud Console 绑定态并列徽章展示，四种组合状态一目了然
- **书签会话持久化徽章** —— 侧栏书签名旁显示 T（tmux）/ S（screen）小徽章并附说明 tooltip，一眼区分可断线重连的会话；搜索可直接匹配 tmux / screen 关键字
- 新增 `make run` 一键启动桌面端开发环境

### 修复
- macOS 原生菜单补齐「Cloud Sync…」入口（与 Windows 自绘菜单对齐），两端共用同一打开逻辑
- 账户中心重新打开时优先读取本地加密登录态，不再因线上资料刷新延迟短暂显示登录表单；账户资料与流量改为后台刷新，并防止退出/切换账户期间的旧请求覆盖新状态

## 0.3.0

### 新增
- **WebGL 加速渲染** —— 对当前可见/聚焦的终端启用 WebGL 渲染器，上下文丢失时自动回退 DOM 渲染
- **端到端加密云同步（书签 / 设置 / known-hosts）** —— 对标 Electerm 的自托管思路，免自建后端
  - 四种存储后端可选：GitHub Gist、Gitee Gist、WebDAV、AuraXLab 账户（官方私有同步服务）
  - 上传前用独立的「同步口令」经 Argon2id + AES-256-GCM 端到端加密（复用现有加密原语，新增 `AURASYNC` blob 格式），存储方/服务端只见密文（零知识）
  - 同步内容可选：书签（默认）、设置子集（主题/字体/快捷按钮/输出规则等，剔除设备相关项）、SSH known-hosts、以及（可选）已保存凭据（需主密码解锁）
  - 合并策略：书签按 id 合并、known-hosts 并集且本地信任优先；提供「合并 / 覆盖本地 / 覆盖云端」与「立即双向同步」
  - 新增设置入口 `Cloud Sync`（File ▸ Preferences 与命令面板），后端 `cloud_sync.rs` + `reqwest`(rustls)
- **AuraXLab 私有书签同步** —— 应用内注册/登录 AuraXLab 账户，按版本化并发控制上传加密 vault（服务端零知识，可自托管）
  - 注册采用「先验证邮箱」流程：填邮箱 → 收 6 位验证码 → 验证通过后再建号（账户直接 confirmed，可立即登录）
  - 注册字段在本地按与服务器一致的规则预校验（邮箱/用户名格式、密码 ≥ 8）
- **用户手册入口** —— Help 菜单与命令面板新增「User Manual」，跳转至 AuraXLab 站点的在线用户手册（`@tauri-apps/plugin-shell` 在系统浏览器打开）
- **云同步联调测试** —— 新增四后端（GitHub/Gitee Gist、WebDAV、AuraXLab）provider 集成测试：默认对内置 mock 服务跑 encrypt→push→pull→decrypt 全链路；`--ignored` 的真实端点测试可凭环境变量对真实服务联调（dev-dependency：`tiny_http`）
- **Phase 5: 协议打磨、嵌套书签与 Shell 集成**
  - Telnet IAC 支持 BINARY/ECHO/SGA、终端类型协商和 NAWS 窗口尺寸更新，未知选项保持安全拒绝
  - Local/SSH/Telnet/Serial 原始流自动识别 `rz`/`sz`，内置 Zmodem 上传、下载、进度、取消及安全文件名处理
  - 书签分组升级为任意层级文件夹，支持导入 OpenSSH config 与 PuTTY `.reg` 会话并自动去重
  - xterm OSC 133 Shell 集成记录命令、退出码和滚动标记，支持命令导航、重跑、复制及提示符降级识别
  - 四类会话统一复用 stream pump，避免协议帧进入 UTF-8 解码与终端渲染
- **Phase 4: SFTP、Snippets 与输出规则**
  - 输出规则引擎统一支持关键字/正则高亮、响铃、桌面通知、自动应答、冷却时间及全局/主机作用域
  - Quick Buttons 升级为 Snippets:多套工具栏、分组、主机/书签组绑定、`{{变量}}` 参数和 Raw 控制字符发送
  - SFTP 支持拖放上传、顺序传输队列、可选断点续传及 SSH 连接后自动打开
  - 远程文件双击进入轻量 UTF-8 编辑器，支持 `Ctrl/Cmd+S` 回写、2 MiB 限制与未保存保护
- **SSH 端口转发 / 隧道管理器**(Phase 2，对标 MobaXterm / SecureCRT 隧道管理）
  - 支持三种转发模式:本地 `-L`、远程 `-R`、动态 `-D`(内置 SOCKS5 代理)
  - 图形化隧道管理器(`Port Forwarding…`,View 菜单或命令面板打开):新增/编辑/删除隧道,逐条启动/停止,实时状态(starting/active/error)与错误提示
  - 隧道可保存进书签并随连接自启(`autoStart`);自动重连后远程转发自动重建
  - 本地/动态转发绑定端口冲突即时反馈;会话关闭时自动回收其所有隧道
- **命令面板(Command Palette)**
  - `Ctrl/Cmd+Shift+P` 唤起模糊搜索弹层,`↑/↓` 导航、`Enter` 执行、`Esc` 关闭
  - 聚合新建会话、开关书签栏/SFTP、打开端口转发、分屏、终端查找、字号、全屏、设置等动作,以及逐条书签快连
- 终端查找功能
  - 支持 `Cmd/Ctrl+F` 打开当前活动终端的搜索栏
  - 支持 `Enter` / `Shift+Enter` 与按钮方式跳转到上一处或下一处匹配
  - 支持大小写匹配、整词匹配、正则表达式三种搜索选项
  - 在分屏场景下始终对当前焦点终端生效，并显示匹配计数或无结果状态

## 0.1.8

### 新增
- **全新 SSH 重连模式**
  - 支持四种重连模式：`Manual` (手动), `Simple` (简单重试), `tmux`, `screen`
  - 默认为 `Manual` 模式，避免意外重连导致的输入丢失
  - 增加手动重连快捷操作：连接断开后，可直接点击底部状态条或按下 `R` 键快速重连
  - 优化重连 UI 提示，连接断开时显示明显的提示状态条
  - 完美兼容旧版本的 `autoReconnect` 布尔值配置，自动迁移至新模式

## 0.1.7

### 新增
- Windows 资源管理器上下文菜单集成，支持"在 AuraTerm 中打开"功能
  - 通过 NSIS 安装程序注册 Windows 资源管理器上下文菜单
  - 支持在文件夹和文件夹背景上右键打开
  - 实现命令行参数解析以接受启动目录
  - 后端支持 PTY 会话的可选工作目录
  - 新增 `get_startup_dir` Tauri 命令供前端获取启动目录
  - 从上下文菜单启动时自动切换到指定目录
- SSH 自动重连功能（通过 screen/tmux）
  - 使用 screen/tmux 实现 SSH 自动重连
  - 提示是否附加到现有会话
  - 使用 'at-' 作为会话前缀
  - 添加后端处理和前端 UI

### 变更
- 窗口位置和大小持久化
  - 将窗口位置/大小保存到设置中，启动时恢复
  - 限制恢复的边界到可见显示器工作区
  - 节流和去重保存
  - 新增 WindowBounds 类型和相关设置迁移

### 改进
- PTY 配置优化
  - 为本地 PTY 子进程显式设置 TERM=xterm-256color
  - 从密码数据库（登录 shell）解析默认 shell，而不仅仅依赖 $SHELL
  - 仍支持通过 settings.shell_path 配置的 shell
  - 以登录 shell 启动（argv0 加 -basename 前缀）以确保正确的登录初始化
  - 添加 libc 依赖以读取 passwd 条目

## 0.1.6

### 新增
- 添加版本同步脚本 (scripts/sync-version.mjs)
- 设置中新增日志保存默认路径和默认文件名模板设置
- 日志文件名模板支持多种占位符：
  - 时间占位符：`{timestamp}`, `{datetime}`, `{date}`, `{time}`, `{yyyy}`, `{MM}`, `{dd}`, `{HH}`, `{mm}`, `{ss}`, `{unix}`
  - 会话占位符：`{session}`, `{protocol}`, `{host}`, `{user}`, `{port}`, `{serialPort}`, `{baudRate}`
- 连接对话框支持自定义日志保存路径，自动按模板生成默认路径

### 变更
- 重构构建和发布流程，优化 Makefile
- 移除 repackage 目标，调整 release/upload 逻辑
- 版本更新至 0.1.6
- 全局样式从 index.html 移至 main.ts
- AboutDialog 改为运行时获取版本信息
- 优化终端组件键盘事件处理
- 日志文件名中的时间占位符在连接建立时解析一次，确保每个会话固定一个日志文件

### 改进
- release.py: 重构为从 `src-tauri/target/release/bundle` 目录读取构建产物，不再依赖目录遍历
- release.py: 自动提取构建产物的发布日期（从文件修改时间）
- upload.py: 简化文件查找逻辑，直接从 target 目录读取 exe/dmg 文件
- upload.py: 移除不必要的 JSON 文件检查，改由 release 目标确保 JSON 存在
- upload.py: 简化文件列表验证，不再检查空列表
- vite.config.ts: 配置 `manualChunks: undefined` 优化打包策略
- package.json: 添加版本同步脚本和 tauri 命令前的版本同步步骤
- RemoteFileManager toolbar: reordered buttons (Up, Download, Upload, Refresh, New Folder, Delete)
- Toolbar now uses icons instead of text labels
- All UI and messages switched to English
- Menu structure expanded: File/View/Help menus now expose all major features (new session, close tab, bookmarks, remote files, settings)
- Improved menu accessibility for Windows/macOS
- UI polish: button layout, icon centering, disabled state

