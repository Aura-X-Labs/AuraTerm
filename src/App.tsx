import { useState, useEffect, useRef, type MouseEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { type } from "@tauri-apps/plugin-os";
import { TerminalComponent, type SerialConnectionState, type TerminalHandle } from "./TerminalComponent.tsx";
import {
  ConnectDialog,
  type SshConfig,
  type TelnetConfig,
  type SerialConfig,
  type ConnectResult,
  type ConnectionProtocol,
} from "./ConnectDialog.tsx";
import { BookmarkSidebar, type SavedConnection } from "./BookmarkSidebar.tsx";
import { SettingsDialog } from "./SettingsDialog";
import { AboutDialog } from "./AboutDialog";
import { TerminalInputBar } from "./TerminalInputBar";
import { type AppSettings, type QuickButton, type SerialHistoryItem, DEFAULT_SETTINGS } from "./settings";
import "./App.css";

type TabSession =
  | { protocol: "local" }
  | { protocol: "ssh"; sshConfig: SshConfig }
  | { protocol: "telnet"; telnetConfig: TelnetConfig }
  | { protocol: "serial"; serialConfig: SerialConfig };

interface Tab {
  id: string;
  title: string;
  session: TabSession;
  logPath?: string;
}

type AppMenuId = "file" | "help";

let nextTabId = 1;

function formatSerialFrame(serialConfig: SerialConfig) {
  const parity = serialConfig.parity === "none" ? "N" : serialConfig.parity === "even" ? "E" : "O";
  return `${serialConfig.dataBits}${parity}${serialConfig.stopBits}`;
}

function App() {
  const [tabs, setTabs] = useState<Tab[]>([{ id: `tab-0`, title: "Local Shell", session: { protocol: "local" } }]);
  const [activeTabId, setActiveTabId] = useState<string>("tab-0");
  const [osType, setOsType] = useState<string>("windows");
  const [showConnectDialog, setShowConnectDialog] = useState(false);
  const [connectDialogProtocol, setConnectDialogProtocol] = useState<ConnectionProtocol>("ssh");
  const [settings, setSettings] = useState<AppSettings>(DEFAULT_SETTINGS);
  const [showSettings, setShowSettings] = useState(false);
  const [showAbout, setShowAbout] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [sidebarRefreshToken, setSidebarRefreshToken] = useState(0);
  const [showNewTabMenu, setShowNewTabMenu] = useState(false);
  const [isWindowFocused, setIsWindowFocused] = useState(true);
  const [serialConnectionStates, setSerialConnectionStates] = useState<Record<string, SerialConnectionState>>({});
  const [openMenuId, setOpenMenuId] = useState<AppMenuId | null>(null);
  const settingsRef = useRef<AppSettings>(DEFAULT_SETTINGS);
  const menuBarRef = useRef<HTMLDivElement | null>(null);

  /** Map from tab id → TerminalHandle (populated by callback refs in JSX) */
  const termRefs = useRef<Map<string, TerminalHandle>>(new Map());

  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  useEffect(() => {
    let unlistenFocus: (() => void) | null = null;
    let unlistenBlur: (() => void) | null = null;

    async function setupWindowFocusListeners() {
      try {
        unlistenFocus = await listen("tauri://focus", () => setIsWindowFocused(true));
        unlistenBlur = await listen("tauri://blur", () => setIsWindowFocused(false));
      } catch (err) {
        console.error("Failed to setup window focus listeners:", err);
      }
    }

    setupWindowFocusListeners();

    return () => {
      unlistenFocus?.();
      unlistenBlur?.();
    };
  }, []);

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
      .then((loaded) => setSettings({ ...DEFAULT_SETTINGS, ...loaded }))
      .catch(() => setSettings(DEFAULT_SETTINGS));
  }, []);

  useEffect(() => {
    const unlisten = listen("show-about", () => setShowAbout(true));
    return () => { unlisten.then(f => f()); };
  }, []);

  useEffect(() => {
    if (!openMenuId) return;

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuBarRef.current?.contains(target)) return;
      setOpenMenuId(null);
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpenMenuId(null);
      }
    };

    document.addEventListener("pointerdown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);

    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [openMenuId]);

  const handleSaveSettings = async (newSettings: AppSettings) => {
    await invoke("save_settings", { settings: newSettings }).catch(console.error);
    setSettings(newSettings);
    setShowSettings(false);
  };

  const persistSettingsSilently = (newSettings: AppSettings) => {
    settingsRef.current = newSettings;
    setSettings(newSettings);
    void invoke("save_settings", { settings: newSettings }).catch((error) => {
      console.error("save_settings failed", error);
    });
  };

  const rememberSerialConfig = (serialConfig: SerialConfig) => {
    const configKey = `${serialConfig.portName}|${serialConfig.baudRate}|${serialConfig.dataBits}|${serialConfig.stopBits}|${serialConfig.parity}|${serialConfig.flowControl}`;
    const historyItem: SerialHistoryItem = {
      id: crypto.randomUUID(),
      name: `${serialConfig.portName} · ${serialConfig.baudRate} ${serialConfig.dataBits}${serialConfig.parity === "none" ? "N" : serialConfig.parity === "even" ? "E" : "O"}${serialConfig.stopBits}`,
      portName: serialConfig.portName,
      baudRate: serialConfig.baudRate,
      dataBits: serialConfig.dataBits,
      stopBits: serialConfig.stopBits,
      parity: serialConfig.parity,
      flowControl: serialConfig.flowControl,
    };

    const current = settingsRef.current;
    const recentSerialConfigs = [historyItem, ...current.recentSerialConfigs.filter((item) => {
      const itemKey = `${item.portName}|${item.baudRate}|${item.dataBits}|${item.stopBits}|${item.parity}|${item.flowControl}`;
      return itemKey !== configKey;
    })].slice(0, 8);

    persistSettingsSilently({
      ...current,
      lastSerialConfig: historyItem,
      recentSerialConfigs,
    });
  };

  /** Send text to the currently active tab's terminal */
  const sendToActiveTerminal = (text: string) => {
    const handle = termRefs.current.get(activeTabId);
    if (handle) handle.sendData(text);
  };

  /** Update quick buttons list (write directly to config without closing settings panel) */
  const handleButtonsChange = async (buttons: QuickButton[]) => {
    const newSettings: AppSettings = { ...settings, quickButtons: buttons };
    await invoke("save_settings", { settings: newSettings }).catch(console.error);
    setSettings(newSettings);
  };

  const updateSerialConnectionState = (tabId: string, state: SerialConnectionState) => {
    setSerialConnectionStates((prev) => {
      if (prev[tabId] === state) return prev;
      return { ...prev, [tabId]: state };
    });
  };

  const handleTitlebarMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest("[data-no-drag='true']")) return;
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

  const handleExitApp = async () => {
    setOpenMenuId(null);
    await getCurrentWindow().close().catch((error) => {
      console.error("exit failed", error);
    });
  };

  const handleOpenAbout = () => {
    setOpenMenuId(null);
    setShowAbout(true);
  };

  const toggleMenu = (menuId: AppMenuId) => {
    setOpenMenuId((current) => current === menuId ? null : menuId);
  };

  const stopDragPropagation = (event: MouseEvent<HTMLElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

  const handleNewLocalSession = () => {
    const newId = `tab-${nextTabId++}`;
    setTabs(prev => [...prev, { id: newId, title: "Local Shell", session: { protocol: "local" } }]);
    setActiveTabId(newId);
  };

  const openConnectDialog = (protocol: ConnectionProtocol) => {
    setConnectDialogProtocol(protocol);
    setShowConnectDialog(true);
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
    setSerialConnectionStates((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  /**
   * Handle ConnectDialog result:
   * - Open new tab to establish SSH/Telnet/Serial connection
   * - If saveAs has a value, save connection to connections.json
   */
  const handleConnectResult = async (result: ConnectResult) => {
    const newId = `tab-${nextTabId++}`;
    const { protocol, sshConfig, telnetConfig, serialConfig, saveAs, saveGroup } = result;

    if (protocol === "ssh" && sshConfig) {
      setTabs((prev) => [
        ...prev,
        {
          id: newId,
          title: `${sshConfig.user}@${sshConfig.host}`,
          session: { protocol: "ssh", sshConfig },
          logPath: result.logPath,
        },
      ]);
    } else if (protocol === "telnet" && telnetConfig) {
      setTabs((prev) => [
        ...prev,
        {
          id: newId,
          title: `telnet://${telnetConfig.host}:${telnetConfig.port}`,
          session: { protocol: "telnet", telnetConfig },
          logPath: result.logPath,
        },
      ]);
    } else if (protocol === "serial" && serialConfig) {
      setTabs((prev) => [
        ...prev,
        {
          id: newId,
          title: `${serialConfig.portName} @ ${serialConfig.baudRate}`,
          session: { protocol: "serial", serialConfig },
          logPath: result.logPath,
        },
      ]);
    }

    if (protocol === "serial" && serialConfig) {
      rememberSerialConfig(serialConfig);
      updateSerialConnectionState(newId, "connecting");
    }
    
    setActiveTabId(newId);
    setShowConnectDialog(false);

    if (saveAs) {
      const conn: SavedConnection = {
        id: crypto.randomUUID(),
        name: saveAs,
        group: saveGroup,
        protocol: protocol,
        host: protocol === "ssh" ? sshConfig!.host : protocol === "telnet" ? telnetConfig!.host : "",
        port: protocol === "ssh" ? sshConfig!.port : protocol === "telnet" ? telnetConfig!.port : 0,
        user: protocol === "ssh" ? sshConfig!.user : "",
        authType: protocol === "ssh" ? (sshConfig!.privateKey ? "key" : "password") : "none",
        password: protocol === "ssh" ? sshConfig!.password : undefined,
        privateKey: protocol === "ssh" ? sshConfig!.privateKey : undefined,
        portName: protocol === "serial" ? serialConfig!.portName : undefined,
        baudRate: protocol === "serial" ? serialConfig!.baudRate : undefined,
        dataBits: protocol === "serial" ? serialConfig!.dataBits : undefined,
        stopBits: protocol === "serial" ? serialConfig!.stopBits : undefined,
        parity: protocol === "serial" ? serialConfig!.parity : undefined,
        flowControl: protocol === "serial" ? serialConfig!.flowControl : undefined,
        createdAt: Date.now(),
      };
      try {
        await invoke("save_connection", { connection: conn });
        // Notify sidebar to refresh
        setSidebarRefreshToken(t => t + 1);
        // Expand sidebar automatically if it's closed
        setSidebarOpen(true);
      } catch (e) {
        console.error("Failed to save connection", e);
      }
    }
  };

  /**
   * Sidebar double-click on saved connection: Opens a new tab immediately
   */
  const handleBookmarkConnect = (connection: SavedConnection, _connectionId: string) => {
    const newId = `tab-${nextTabId++}`;
    const protocol = connection.protocol ?? "ssh";

    let tab: Tab;
    if (protocol === "serial" && connection.portName && connection.baudRate) {
      rememberSerialConfig({
        portName: connection.portName,
        baudRate: connection.baudRate,
        dataBits: connection.dataBits ?? 8,
        stopBits: connection.stopBits ?? 1,
        parity: connection.parity ?? "none",
        flowControl: connection.flowControl ?? "none",
      });
      tab = {
        id: newId,
        title: `${connection.portName} @ ${connection.baudRate}`,
        session: {
          protocol: "serial",
          serialConfig: {
            portName: connection.portName,
            baudRate: connection.baudRate,
            dataBits: connection.dataBits ?? 8,
            stopBits: connection.stopBits ?? 1,
            parity: connection.parity ?? "none",
            flowControl: connection.flowControl ?? "none",
          },
        },
      };
    } else if (protocol === "telnet") {
      tab = {
        id: newId,
        title: `telnet://${connection.host}:${connection.port}`,
        session: {
          protocol: "telnet",
          telnetConfig: {
            host: connection.host,
            port: connection.port,
          },
        },
      };
    } else {
      tab = {
        id: newId,
        title: `${connection.user}@${connection.host}`,
        session: {
          protocol: "ssh",
          sshConfig: {
            host: connection.host,
            port: connection.port,
            user: connection.user,
            password: connection.authType === "password" ? connection.password : undefined,
            privateKey: connection.authType === "key" ? connection.privateKey : undefined,
          },
        },
      };
    }

    if (tab.session.protocol === "serial") {
      updateSerialConnectionState(newId, "connecting");
    }

    setTabs((prev) => [
      ...prev,
      tab,
    ]);
    setActiveTabId(newId);
  };

  const activeTab = tabs.find((tab) => tab.id === activeTabId);
  const activeSerialConfig = activeTab?.session.protocol === "serial" ? activeTab.session.serialConfig : null;
  const activeSerialConnectionState =
    activeTab && activeSerialConfig ? serialConnectionStates[activeTab.id] ?? "connecting" : null;

  return (
    <div className={`app-container ${osType} ${isWindowFocused ? 'focused' : 'blurred'}`}>
      <div className="titlebar" onMouseDown={handleTitlebarMouseDown}>
        {osType !== "windows" && (
          <div className="titlebar-controls" aria-label="Window controls" data-no-drag="true">
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
        {osType === "windows" ? (
          <div className="titlebar-windows-main">
            <div className="titlebar-title">AuraTerm</div>
            <div
              ref={menuBarRef}
              className="titlebar-menubar"
              data-no-drag="true"
            >
              <div
                className="titlebar-menu-group"
                onMouseEnter={() => {
                  if (openMenuId) setOpenMenuId("file");
                }}
              >
                <button
                  className={`titlebar-menu-btn ${openMenuId === "file" ? "open" : ""}`}
                  onMouseDown={stopDragPropagation}
                  onClick={() => toggleMenu("file")}
                  type="button"
                >
                  File
                </button>
                {openMenuId === "file" && (
                  <div className="titlebar-menu-dropdown" onMouseDown={stopDragPropagation}>
                    <button className="titlebar-menu-item" onClick={handleExitApp} type="button">
                      <span>Exit</span>
                      <span className="titlebar-menu-item-hint">Alt+F4</span>
                    </button>
                  </div>
                )}
              </div>
              <div
                className="titlebar-menu-group"
                onMouseEnter={() => {
                  if (openMenuId) setOpenMenuId("help");
                }}
              >
                <button
                  className={`titlebar-menu-btn ${openMenuId === "help" ? "open" : ""}`}
                  onMouseDown={stopDragPropagation}
                  onClick={() => toggleMenu("help")}
                  type="button"
                >
                  Help
                </button>
                {openMenuId === "help" && (
                  <div className="titlebar-menu-dropdown" onMouseDown={stopDragPropagation}>
                    <button className="titlebar-menu-item" onClick={handleOpenAbout} type="button">
                      <span>About AuraTerm</span>
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
        ) : (
          <div className="titlebar-title">AuraTerm</div>
        )}
        {osType === "windows" && (
          <div className="titlebar-controls-win" aria-label="Window controls" data-no-drag="true">
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
          title="Bookmarks"
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

        <div className="terminal-wrapper">
          <div className="terminal-container">
            {tabs.length === 0 ? (
              <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100%', color: '#666' }}>
                No open tabs. Click + to open a new tab.
              </div>
            ) : (
              tabs.map((tab) => (
                <TerminalComponent
                  key={tab.id}
                  ref={(el: TerminalHandle | null) => {
                    if (el) termRefs.current.set(tab.id, el);
                    else termRefs.current.delete(tab.id);
                  }}
                  isActive={activeTabId === tab.id}
                  session={tab.session}
                  logPath={tab.logPath}
                  settings={settings}
                  onSerialConnectionStateChange={(state) => updateSerialConnectionState(tab.id, state)}
                />
              ))
            )}
          </div>
          <TerminalInputBar
            quickButtons={settings.quickButtons}
            onSend={sendToActiveTerminal}
            onButtonsChange={handleButtonsChange}
          />
          {activeTab && activeSerialConfig && activeSerialConnectionState && (
            <div className="terminal-statusbar">
              <div className="terminal-statusbar-left">
                <span className={`terminal-status-indicator ${activeSerialConnectionState}`} />
                <span>{activeSerialConfig.portName}</span>
                <span className="terminal-status-pill">{activeSerialConnectionState}</span>
              </div>
              <div className="terminal-statusbar-right">
                <span>{activeSerialConfig.baudRate} baud</span>
                <span>{formatSerialFrame(activeSerialConfig)}</span>
                <span>{activeSerialConfig.flowControl}</span>
              </div>
            </div>
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
            <div className="newtab-dialog-title">New Session</div>
            <div className="newtab-options">
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); handleNewLocalSession(); }}
              >
                <span className="newtab-option-icon">🖥</span>
                <span className="newtab-option-label">Local Shell</span>
                <span className="newtab-option-desc">Open a local terminal session</span>
              </button>
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); openConnectDialog("ssh"); }}
              >
                <span className="newtab-option-icon">🔗</span>
                <span className="newtab-option-label">SSH</span>
                <span className="newtab-option-desc">Connect to a remote shell over SSH</span>
              </button>
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); openConnectDialog("telnet"); }}
              >
                <span className="newtab-option-icon">🌐</span>
                <span className="newtab-option-label">Telnet</span>
                <span className="newtab-option-desc">Open a TCP terminal session</span>
              </button>
              <button
                className="newtab-option-btn"
                onClick={() => { setShowNewTabMenu(false); openConnectDialog("serial"); }}
              >
                <span className="newtab-option-icon">🔌</span>
                <span className="newtab-option-label">Serial</span>
                <span className="newtab-option-desc">Enumerate and connect to a serial device</span>
              </button>
            </div>
          </div>
        </div>
      )}

      {showConnectDialog && (
        <ConnectDialog
          initialProtocol={connectDialogProtocol}
          lastSerialConfig={settings.lastSerialConfig}
          recentSerialConfigs={settings.recentSerialConfigs}
          onConnect={handleConnectResult}
          onCancel={() => setShowConnectDialog(false)}
        />
      )}
    </div>
  );
}

export default App;
