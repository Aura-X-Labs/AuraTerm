import { useState, useEffect, type MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type } from "@tauri-apps/plugin-os";
import { TerminalComponent } from "./TerminalComponent";
import { ConnectDialog, type SshConfig } from "./ConnectDialog";
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
        // Pick the previous tab or the next one
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
        <button className="tab-new-btn" onClick={() => setShowConnectDialog(true)} title="New SSH Connection" style={{ marginLeft: '4px' }}>
          &#x1F5A7;
        </button>
      </div>

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
            />
          ))
        )}
      </div>

      {showConnectDialog && (
        <ConnectDialog
          onConnect={(config) => {
            const newId = `tab-${nextTabId++}`;
            setTabs((prev) => [
              ...prev,
              { id: newId, title: `${config.user}@${config.host}`, sshConfig: config },
            ]);
            setActiveTabId(newId);
            setShowConnectDialog(false);
          }}
          onCancel={() => setShowConnectDialog(false)}
        />
      )}
    </div>
  );
}

export default App;
