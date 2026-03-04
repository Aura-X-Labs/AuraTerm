import { useState, useEffect, useRef, type MouseEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { type } from "@tauri-apps/plugin-os";
import { TerminalComponent, type TerminalHandle } from "./TerminalComponent";
import { ConnectDialog, type SshConfig, type ConnectResult } from "./ConnectDialog";
import { BookmarkSidebar, type SavedConnection } from "./BookmarkSidebar";
import { SettingsDialog } from "./SettingsDialog";
import { AboutDialog } from "./AboutDialog";
import { type AppSettings, DEFAULT_SETTINGS } from "./settings";
import "./App.css";

interface Tab {
  id: string;
  title: string;
  sshConfig?: SshConfig;
  logPath?: string;
}

let nextTabId = 1;

function App() {
  const [tabs, setTabs] = useState<Tab[]>([{ id: `tab-0`, title: "Local Shell" }]);
  const [activeTabId, setActiveTabId] = useState<string>("tab-0");
  const [osType, setOsType] = useState<string>("windows");
  const [showConnectDialog, setShowConnectDialog] = useState(false);
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [showSettings, setShowSettings] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [sidebarRefreshToken, setSidebarRefreshToken] = useState(0);
  const [showNewTabMenu, setShowNewTabMenu] = useState(false);

  /** Map from tab id → TerminalHandle (populated by callback refs in JSX) */
  const termRefs = useRef<Map<string, TerminalHandle>>(new Map());

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

  useEffect(() => {
    const unlisten = listen("show-about", () => setShowAbout(true));
    return () => { unlisten.then(f => f()); };
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
    const { protocol, sshConfig, telnetConfig, saveAs } = result;

    if (protocol === "ssh" && sshConfig) {
      setTabs((prev) => [
        ...prev,
        { id: newId, title: `${sshConfig.user}@${sshConfig.host}`, sshConfig: sshConfig, logPath: result.logPath },
      ]);
    } else if (protocol === "telnet" && telnetConfig) {
      setTabs((prev) => [
        ...prev,
        { id: newId, title: `telnet://${telnetConfig.host}:${telnetConfig.port}`, logPath: result.logPath },
      ]);
    }
    
    setActiveTabId(newId);
    setShowConnectDialog(false);

    if (saveAs) {
      const conn: SavedConnection = {
        id: crypto.randomUUID(),
        name: saveAs,
        protocol: protocol,
        host: protocol === "ssh" ? sshConfig!.host : telnetConfig!.host,
        port: protocol === "ssh" ? sshConfig!.port : telnetConfig!.port,
        user: protocol === "ssh" ? sshConfig!.user : "",
        authType: protocol === "ssh" ? (sshConfig!.privateKey ? "key" : "password") : "password",
        password: protocol === "ssh" ? sshConfig!.password : "",
        privateKey: protocol === "ssh" ? sshConfig!.privateKey : undefined,
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
        <button
          className={`tab-new-btn bookmark-toggle-btn ${sidebarOpen ? 'active' : ''}`}
          onClick={() => setSidebarOpen(v => !v)}
          title="快捷连接"
          style={{ marginRight: '4px' }}
        >
          🔖
        </button>
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
        <button className="tab-new-btn" onClick={() => setShowNewTabMenu(true)} title="New Tab">
          +
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
                ref={(el) => {
                  if (el) termRefs.current.set(tab.id, el);
                  else termRefs.current.delete(tab.id);
                }}
                isActive={activeTabId === tab.id}
                sshConfig={tab.sshConfig}
                logPath={tab.logPath}
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

      {showAbout && (
        <AboutDialog onClose={() => setShowAbout(false)} />
      )}

      {showNewTabMenu && (
        <div className="newtab-overlay" onClick={() => setShowNewTabMenu(false)}>
          <div className="newtab-dialog" onClick={(e) => e.stopPropagation()}>
            <div className="newtab-dialog-title">New Tab</div>
            <div className="newtab-options">
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); handleNewTab(); }}
              >
                <span className="newtab-option-icon">🖥</span>
                <span className="newtab-option-label">Local Shell</span>
                <span className="newtab-option-desc">Open a local terminal session</span>
              </button>
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); setShowConnectDialog(true); }}
              >
                <span className="newtab-option-icon">🔗</span>
                <span className="newtab-option-label">SSH / Telnet</span>
                <span className="newtab-option-desc">Connect to a remote host</span>
              </button>
            </div>
          </div>
        </div>
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
