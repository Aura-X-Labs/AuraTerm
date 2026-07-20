
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
- 本地开发可用 `AURATERM_AURAXLAB_URL` / `VITE_AURAXLAB_URL` 指向自托管 AuraXLab 服务器；新增 `make run` 一键联调

### 修复
- macOS 原生菜单补齐「Cloud Sync…」入口（与 Windows 自绘菜单对齐），两端共用同一打开逻辑

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

