
## Doing

### 新增
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

