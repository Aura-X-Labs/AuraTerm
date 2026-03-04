import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SshConfig } from "./ConnectDialog";

export interface SavedConnection {
  id: string;
  name: string;
  protocol?: "ssh" | "telnet";
  host: string;
  port: number;
  user: string;
  authType: "password" | "key";
  password?: string;
  privateKey?: string;
  createdAt: number;
  lastUsed?: number;
}

interface BookmarkSidebarProps {
  onConnect: (config: SshConfig, connectionId: string) => void;
  /** 每次递增该值，侧边栏会自动重新加载连接列表 */
  refreshToken?: number;
}

interface ContextMenu {
  x: number;
  y: number;
  connection: SavedConnection;
}

export function BookmarkSidebar({ onConnect, refreshToken }: BookmarkSidebarProps) {
  const [connections, setConnections] = useState<SavedConnection[]>([]);
  const [contextMenu, setContextMenu] = useState<ContextMenu | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const contextMenuRef = useRef<HTMLDivElement>(null);

  const loadConnections = async () => {
    try {
      const conns = await invoke<SavedConnection[]>("get_connections");
      setConnections(conns);
    } catch (e) {
      console.error("Failed to load connections", e);
    }
  };

  useEffect(() => {
    void loadConnections();
  }, [refreshToken]);

  // 点击其他地方关闭右键菜单
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (contextMenuRef.current && !contextMenuRef.current.contains(e.target as Node)) {
        setContextMenu(null);
      }
    };
    if (contextMenu) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [contextMenu]);

  const handleDoubleClick = async (conn: SavedConnection) => {
    // 更新 last_used 时间戳
    try {
      await invoke("touch_connection", { id: conn.id, timestamp: Date.now() });
    } catch (_) {}

    onConnect(
      {
        host: conn.host,
        port: conn.port,
        user: conn.user,
        password: conn.authType === "password" ? conn.password : undefined,
        privateKey: conn.authType === "key" ? conn.privateKey : undefined,
      },
      conn.id
    );
  };

  const handleContextMenu = (e: React.MouseEvent, conn: SavedConnection) => {
    e.preventDefault();
    setContextMenu({ x: e.clientX, y: e.clientY, connection: conn });
  };

  const handleDelete = async (id: string) => {
    try {
      await invoke("delete_connection", { id });
      setConnections((prev) => prev.filter((c) => c.id !== id));
    } catch (e) {
      console.error("Failed to delete connection", e);
    }
    setContextMenu(null);
  };

  const handleStartRename = (conn: SavedConnection) => {
    setEditingId(conn.id);
    setEditingName(conn.name);
    setContextMenu(null);
  };

  const handleRenameSubmit = async (conn: SavedConnection) => {
    if (!editingName.trim()) {
      setEditingId(null);
      return;
    }
    const updated: SavedConnection = { ...conn, name: editingName.trim() };
    try {
      await invoke("save_connection", { connection: updated });
      setConnections((prev) => prev.map((c) => (c.id === conn.id ? updated : c)));
    } catch (e) {
      console.error("Failed to rename connection", e);
    }
    setEditingId(null);
  };

  return (
    <div className="bookmark-sidebar">
      <div className="bookmark-sidebar-header">
        <span className="bookmark-sidebar-title">🔖 快捷连接</span>
        <button
          className="bookmark-refresh-btn"
          title="刷新列表"
          onClick={loadConnections}
        >
          ↻
        </button>
      </div>

      {connections.length === 0 ? (
        <div className="bookmark-empty">
          暂无保存的连接。
          <br />
          新建 SSH 连接时勾选
          <br />
          "保存此连接"即可添加。
        </div>
      ) : (
        <ul className="bookmark-list">
          {connections.map((conn) => {
            const isTelnet = conn.protocol === "telnet";
            const subtitle = isTelnet
              ? `telnet://${conn.host}:${conn.port}`
              : `${conn.user}@${conn.host}:${conn.port}`;
            const icon = isTelnet ? "🌐" : "🖥";
            return (
              <li
                key={conn.id}
                className="bookmark-item"
                onDoubleClick={() => handleDoubleClick(conn)}
                onContextMenu={(e) => handleContextMenu(e, conn)}
                title={`${subtitle}\n双击连接`}
              >
                <span className="bookmark-icon">{icon}</span>
                <div className="bookmark-info">
                {editingId === conn.id ? (
                  <input
                    className="bookmark-name-input"
                    value={editingName}
                    autoFocus
                    onChange={(e) => setEditingName(e.target.value)}
                    onBlur={() => handleRenameSubmit(conn)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") handleRenameSubmit(conn);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    onClick={(e) => e.stopPropagation()}
                  />
                ) : (
                  <span className="bookmark-name">{conn.name}</span>
                )}
                <span className="bookmark-host">
                  {isTelnet ? `${conn.host}:${conn.port}` : `${conn.user}@${conn.host}:${conn.port}`}
                </span>
              </div>
            </li>
            );
          })}
        </ul>
      )}

      {/* 右键菜单 */}
      {contextMenu && (
        <div
          ref={contextMenuRef}
          className="bookmark-context-menu"
          style={{ top: contextMenu.y, left: contextMenu.x }}
        >
          <button
            className="bookmark-context-item"
            onClick={() => handleStartRename(contextMenu.connection)}
          >
            ✏️ 重命名
          </button>
          <button
            className="bookmark-context-item danger"
            onClick={() => handleDelete(contextMenu.connection.id)}
          >
            🗑 删除
          </button>
        </div>
      )}
    </div>
  );
}

// 导出 refresh 触发器供父组件通知侧边栏刷新（通过 ref）
export type BookmarkSidebarRef = {
  refresh: () => void;
};