import { useState, useEffect, type MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { type } from "@tauri-apps/plugin-os";
import { TerminalComponent } from "./TerminalComponent";
import { ConnectDialog, type SshConfig, type ConnectResult } from "./ConnectDialog";
import { BookmarkSidebar, type SavedConnection } from "./BookmarkSidebar";
import { SettingsDialog } from "./SettingsDialog";
import { type AppSettings, DEFAULT_SETTINGS } from "./settings";
import "./App.css";

interface Tab {
  id: string;
  title: string;
  sshConfig?: SshConfig;
}

let nextTabId = 1;

function App() {
  const [tabs, setTabs] = useState<Tab[]>([{ id: `tab-0`, title: "Local Shell" }]);
  const [activeTabId, setActiveTabId] = useState<string>("tab-0");
  const [osType, setOsType] = useState<string>("windows");
  const [showConnectDialog, setShowConnectDialog] = useState(false);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [showSettings, setShowSettings] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [sidebarRefreshToken, setSidebarRefreshToken] = useState(0);

  useEffect(() => {
    async function determineOs() {
      try {
        const os = await type();
        setOsType(os);
      } catch (err) {
        console.error("Failed to detect OS:", err);
      }
    }
    determineOs();
  }, []);

  useEffect(() => {
    invoke<AppSettings>("get_settings")
      .then(setSettings)
      .catch(() => setSettings(DEFAULT_SETTINGS));
  }, []);

  const handleSaveSettings = async (newSettings: AppSettings) => {
    await invoke("save_settings", { settings: newSettings }).catch(console.error);
    setSettings(newSettings);
    setShowSettings(false);
  };

  const handleTitlebarMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest(".titlebar-controls")) return;
    getCurrentWindow().startDragging().catch((error) => {
      console.error("startDragging failed", error);
    });
  };

  const handleMinimize = async () => {
    await getCurrentWindow().minimize().catch((error) => {
      console.error("minimize failed", error);
    });
  };

  const handleToggleMaximize = async () => {
    const window = getCurrentWindow();
    const isMaximized = await window.isMaximized().catch((error) => {
      console.error("isMaximized failed", error);
      return false;
    });
    if (isMaximized) {
      await window.unmaximize().catch((error) => {
        console.error("unmaximize failed", error);
      });
      return;
    }
    await window.maximize().catch((error) => {
      console.error("maximize failed", error);
    });
  };

  const handleClose = async () => {
    await getCurrentWindow().close().catch((error) => {
      console.error("close failed", error);
    });
  };

  const stopDragPropagation = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

  const handleNewTab = () => {
    const newId = `tab-${nextTabId++}`;
    setTabs(prev => [...prev, { id: newId, title: "Local Shell" }]);
    setActiveTabId(newId);
  };

  const handleCloseTab = (id: string, event: MouseEvent) => {
    event.stopPropagation();
    setTabs(prev => {
      const newTabs = prev.filter(t => t.id !== id);
      if (activeTabId === id) {
        const index = prev.findIndex(t => t.id === id);
        if (newTabs.length > 0) {
          setActiveTabId(newTabs[Math.max(0, index - 1)].id);
        } else {
          setActiveTabId("");
        }
      }
      return newTabs;
    });
  };

  /**
   * 处理 ConnectDialog 的连接结果：
   * - 开新标签页建立 SSH 连接
   * - 如果 saveAs 有值，则保存到 connections.json
   */
  const handleConnectResult = async (result: ConnectResult) => {
    const newId = `tab-${nextTabId++}`;
    const { config, saveAs } = result;

    setTabs((prev) => [
      ...prev,
      { id: newId, title: `${config.user}@${config.host}`, sshConfig: config },
    ]);
    setActiveTabId(newId);
    setShowConnectDialog(false);

    if (saveAs) {
      const conn: SavedConnection = {
        id: crypto.randomUUID(),
        name: saveAs,
        host: config.host,
        port: config.port,
        user: config.user,
        authType: config.privateKey ? "key" : "password",
        password: config.password,
        privateKey: config.privateKey,
        createdAt: Date.now(),
      };
      try {
        await invoke("save_connection", { connection: conn });
        // 通知侧边栏刷新（递增 token）
        setSidebarRefreshToken(t => t + 1);
        // 如果侧边栏是关闭的，自动展开提示用户
        setSidebarOpen(true);
      } catch (e) {
        console.error("Failed to save connection", e);
      }
    }
  };

  /**
   * 侧边栏双击已保存的连接，直接开新标签页
   */
  const handleBookmarkConnect = (config: SshConfig, _connectionId: string) => {
    const newId = `tab-${nextTabId++}`;
    setTabs((prev) => [
      ...prev,
      { id: newId, title: `${config.user}@${config.host}`, sshConfig: config },
    ]);
    setActiveTabId(newId);
  };

  return (
    <div className={`app-container ${osType}`}>
      <div className="titlebar" onMouseDown={handleTitlebarMouseDown}>
        {osType !== "windows" && (
          <div className="titlebar-controls" aria-label="Window controls">
            <button
              className="titlebar-control-btn titlebar-control-close"
              onMouseDown={stopDragPropagation}
              onClick={handleClose}
              aria-label="Close"
              type="button"
            />
            <button
              className="titlebar-control-btn titlebar-control-minimize"
              onMouseDown={stopDragPropagation}
              onClick={handleMinimize}
              aria-label="Minimize"
              type="button"
            />
            <button
              className="titlebar-control-btn titlebar-control-maximize"
              onMouseDown={stopDragPropagation}
              onClick={handleToggleMaximize}
              aria-label="Maximize"
              type="button"
            />
          </div>
        )}
        <div className="titlebar-title">AuraTerm</div>
        {osType === "windows" && (
          <div className="titlebar-controls-win" aria-label="Window controls">
            <button
              className="titlebar-control-win-btn"
              onMouseDown={stopDragPropagation}
              onClick={handleMinimize}
              aria-label="Minimize"
              type="button"
            >
              &#xE921;
            </button>
            <button
              className="titlebar-control-win-btn"
              onMouseDown={stopDragPropagation}
              onClick={handleToggleMaximize}
              aria-label="Maximize"
              type="button"
            >
              &#xE922;
            </button>
            <button
              className="titlebar-control-win-btn close"
              onMouseDown={stopDragPropagation}
              onClick={handleClose}
              aria-label="Close"
              type="button"
            >
              &#xE8BB;
            </button>
          </div>
        )}
      </div>

      <div className="tab-bar">
        {tabs.map((tab) => (
          <div
            key={tab.id}
            className={`tab-item ${activeTabId === tab.id ? 'active' : ''}`}
            onClick={() => setActiveTabId(tab.id)}
          >
            <span className="tab-title">{tab.title}</span>
            <button
              className="tab-close-btn"
              onClick={(e) => handleCloseTab(tab.id, e)}
              title="Close Tab"
            >
              ×
            </button>
          </div>
        ))}
        <button className="tab-new-btn" onClick={handleNewTab} title="New Local Shell">
          +
        </button>
        <button
          className="tab-new-btn"
          onClick={() => setShowConnectDialog(true)}
          title="New SSH Connection"
          style={{ marginLeft: '4px' }}
        >
          &#x1F5A7;
        </button>
        <button
          className={`tab-new-btn bookmark-toggle-btn ${sidebarOpen ? 'active' : ''}`}
          onClick={() => setSidebarOpen(v => !v)}
          title="快捷连接"
          style={{ marginLeft: '4px' }}
        >
          🔖
        </button>
        <button
          className="tab-new-btn"
          onClick={() => setShowSettings(true)}
          title="Settings"
          style={{ marginLeft: 'auto' }}
        >
          &#x2699;
        </button>
      </div>

      <div className="workspace">
        {sidebarOpen && (
          <BookmarkSidebar
            refreshToken={sidebarRefreshToken}
            onConnect={handleBookmarkConnect}
          />
        )}

        <div className="terminal-container">
          {tabs.length === 0 ? (
            <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100%', color: '#666' }}>
              No open tabs. Click + to open a new tab.
            </div>
          ) : (
            tabs.map((tab) => (
              <TerminalComponent
                key={tab.id}
                isActive={activeTabId === tab.id}
                sshConfig={tab.sshConfig}
                settings={settings}
              />
            ))
          )}
        </div>
      </div>

      {showSettings && (
        <SettingsDialog
          initial={settings}
          onSave={handleSaveSettings}
          onCancel={() => setShowSettings(false)}
        />
      )}

      {showConnectDialog && (
        <ConnectDialog
          onConnect={handleConnectResult}
          onCancel={() => setShowConnectDialog(false)}
        />
      )}
    </div>
  );
}

export default App;
