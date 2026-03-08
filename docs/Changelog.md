## 0.1.5

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
- 版本更新至 0.1.5
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