import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface SavedConnection {
  id: string;
  name: string;
  group?: string;
  protocol?: "ssh" | "telnet" | "serial";
  host: string;
  port: number;
  user: string;
  authType: "password" | "key" | "none";
  password?: string;
  privateKey?: string;
  portName?: string;
  baudRate?: number;
  dataBits?: 5 | 6 | 7 | 8;
  stopBits?: 1 | 2;
  parity?: "none" | "odd" | "even";
  flowControl?: "none" | "hardware" | "software";
  createdAt: number;
  lastUsed?: number;
}

interface BookmarkSidebarProps {
  onConnect: (connection: SavedConnection, connectionId: string) => void;
  refreshToken?: number;
}

interface ContextMenu {
  x: number;
  y: number;
  connection: SavedConnection;
}

const UNGROUPED_LABEL = "未分组";

function toDisplayGroup(value?: string) {
  return value?.trim() || UNGROUPED_LABEL;
}

function buildSubtitle(conn: SavedConnection) {
  if (conn.protocol === "serial") {
    return `${conn.portName ?? "serial"} · ${conn.baudRate ?? 9600} baud`;
  }
  if (conn.protocol === "telnet") {
    return `telnet://${conn.host}:${conn.port}`;
  }
  return `${conn.user}@${conn.host}:${conn.port}`;
}

function buildIcon(conn: SavedConnection) {
  if (conn.protocol === "serial") return "🔌";
  if (conn.protocol === "telnet") return "🌐";
  return "🖥";
}

export function BookmarkSidebar({ onConnect, refreshToken }: BookmarkSidebarProps) {
  const [connections, setConnections] = useState<SavedConnection[]>([]);
  const [contextMenu, setContextMenu] = useState<ContextMenu | null>(null);
  const [editingConnection, setEditingConnection] = useState<SavedConnection | null>(null);
  const [editDraft, setEditDraft] = useState<SavedConnection | null>(null);
  const [editError, setEditError] = useState("");
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

  const groupedConnections = useMemo(() => {
    const groups = new Map<string, SavedConnection[]>();
    for (const conn of connections) {
      const group = toDisplayGroup(conn.group);
      const list = groups.get(group) ?? [];
      list.push(conn);
      groups.set(group, list);
    }
    return Array.from(groups.entries()).sort(([a], [b]) => {
      if (a === UNGROUPED_LABEL) return 1;
      if (b === UNGROUPED_LABEL) return -1;
      return a.localeCompare(b, "zh-CN");
    });
  }, [connections]);

  const handleDoubleClick = async (conn: SavedConnection) => {
    try {
      await invoke("touch_connection", { id: conn.id, timestamp: Date.now() });
    } catch {
      // ignore
    }
    onConnect(conn, conn.id);
  };

  const handleContextMenu = (e: ReactMouseEvent, conn: SavedConnection) => {
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

  const openEditDialog = (conn: SavedConnection) => {
    setEditingConnection(conn);
    setEditDraft({ ...conn });
    setEditError("");
    setContextMenu(null);
  };

  const closeEditDialog = () => {
    setEditingConnection(null);
    setEditDraft(null);
    setEditError("");
  };

  const updateDraft = <K extends keyof SavedConnection>(key: K, value: SavedConnection[K]) => {
    setEditDraft((prev) => (prev ? { ...prev, [key]: value } : prev));
  };

  const saveDraft = async () => {
    if (!editDraft || !editingConnection) return;

    const protocol = editDraft.protocol ?? "ssh";
    if (!editDraft.name.trim()) {
      setEditError("名称不能为空。");
      return;
    }
    if (protocol === "serial") {
      if (!editDraft.portName?.trim()) {
        setEditError("串口设备不能为空。");
        return;
      }
    } else {
      if (!editDraft.host.trim()) {
        setEditError("主机地址不能为空。");
        return;
      }
      if (protocol === "ssh" && !editDraft.user.trim()) {
        setEditError("SSH 用户名不能为空。");
        return;
      }
    }

    const normalized: SavedConnection = {
      ...editDraft,
      name: editDraft.name.trim(),
      group: editDraft.group?.trim() || undefined,
      host: protocol === "serial" ? "" : editDraft.host.trim(),
      port: protocol === "serial" ? 0 : editDraft.port,
      user: protocol === "ssh" ? editDraft.user.trim() : "",
      authType: protocol === "ssh" ? editDraft.authType : "none",
      password: protocol === "ssh" && editDraft.authType === "password" ? editDraft.password : undefined,
      privateKey: protocol === "ssh" && editDraft.authType === "key" ? editDraft.privateKey : undefined,
      portName: protocol === "serial" ? editDraft.portName?.trim() : undefined,
      baudRate: protocol === "serial" ? editDraft.baudRate : undefined,
      dataBits: protocol === "serial" ? editDraft.dataBits : undefined,
      stopBits: protocol === "serial" ? editDraft.stopBits : undefined,
      parity: protocol === "serial" ? editDraft.parity : undefined,
      flowControl: protocol === "serial" ? editDraft.flowControl : undefined,
    };

    try {
      await invoke("save_connection", { connection: normalized });
      setConnections((prev) => prev.map((conn) => (conn.id === editingConnection.id ? normalized : conn)));
      closeEditDialog();
    } catch (e) {
      console.error("Failed to save connection", e);
      setEditError(String(e));
    }
  };

  return (
    <div className="bookmark-sidebar">
      <div className="bookmark-sidebar-header">
        <span className="bookmark-sidebar-title">🔖 快捷连接</span>
        <button className="bookmark-refresh-btn" title="刷新列表" onClick={loadConnections}>
          ↻
        </button>
      </div>

      {connections.length === 0 ? (
        <div className="bookmark-empty">
          暂无保存的连接。
          <br />
          新建会话时勾选
          <br />
          “保存此连接”即可添加。
        </div>
      ) : (
        <div className="bookmark-list">
          {groupedConnections.map(([group, items]) => (
            <div key={group} className="bookmark-group">
              <div className="bookmark-group-header">
                <span className="bookmark-group-name">{group}</span>
                <span className="bookmark-group-count">{items.length}</span>
              </div>
              <ul className="bookmark-group-list">
                {items.map((conn) => {
                  const subtitle = buildSubtitle(conn);
                  return (
                    <li
                      key={conn.id}
                      className="bookmark-item"
                      onDoubleClick={() => handleDoubleClick(conn)}
                      onContextMenu={(e) => handleContextMenu(e, conn)}
                      title={`${subtitle}\n双击连接`}
                    >
                      <span className="bookmark-icon">{buildIcon(conn)}</span>
                      <div className="bookmark-info">
                        <span className="bookmark-name">{conn.name}</span>
                        <span className="bookmark-host">{subtitle}</span>
                      </div>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </div>
      )}

      {contextMenu && (
        <div
          ref={contextMenuRef}
          className="bookmark-context-menu"
          style={{ top: contextMenu.y, left: contextMenu.x }}
        >
          <button className="bookmark-context-item" onClick={() => openEditDialog(contextMenu.connection)}>
            ✏️ 编辑
          </button>
          <button className="bookmark-context-item danger" onClick={() => handleDelete(contextMenu.connection.id)}>
            🗑 删除
          </button>
        </div>
      )}

      {editingConnection && editDraft && (
        <div className="bookmark-editor-overlay" onClick={closeEditDialog}>
          <div className="bookmark-editor-dialog" onClick={(e) => e.stopPropagation()}>
            <div className="bookmark-editor-header">
              <div>
                <div className="bookmark-editor-title">编辑书签</div>
                <div className="bookmark-editor-subtitle">{editDraft.protocol === "serial" ? "串口参数" : editDraft.protocol === "telnet" ? "Telnet 参数" : "SSH 参数"}</div>
              </div>
              <button type="button" className="bookmark-editor-close" onClick={closeEditDialog}>×</button>
            </div>

            <div className="bookmark-editor-body">
              <div className="bookmark-editor-grid">
                <div className="form-group">
                  <label>名称</label>
                  <input type="text" value={editDraft.name} onChange={(e) => updateDraft("name", e.target.value)} />
                </div>
                <div className="form-group">
                  <label>分组</label>
                  <input type="text" value={editDraft.group ?? ""} onChange={(e) => updateDraft("group", e.target.value)} placeholder="未分组" />
                </div>
              </div>

              {editDraft.protocol === "serial" ? (
                <>
                  <div className="bookmark-editor-grid">
                    <div className="form-group bookmark-editor-span-2">
                      <label>串口设备</label>
                      <input type="text" value={editDraft.portName ?? ""} onChange={(e) => updateDraft("portName", e.target.value)} placeholder="/dev/cu.usbserial-1410" />
                    </div>
                    <div className="form-group">
                      <label>Baud Rate</label>
                      <input type="number" value={editDraft.baudRate ?? 9600} onChange={(e) => updateDraft("baudRate", Number(e.target.value) || 9600)} />
                    </div>
                    <div className="form-group">
                      <label>Data Bits</label>
                      <select value={String(editDraft.dataBits ?? 8)} onChange={(e) => updateDraft("dataBits", Number(e.target.value) as 5 | 6 | 7 | 8)}>
                        <option value="5">5</option>
                        <option value="6">6</option>
                        <option value="7">7</option>
                        <option value="8">8</option>
                      </select>
                    </div>
                    <div className="form-group">
                      <label>Stop Bits</label>
                      <select value={String(editDraft.stopBits ?? 1)} onChange={(e) => updateDraft("stopBits", Number(e.target.value) as 1 | 2)}>
                        <option value="1">1</option>
                        <option value="2">2</option>
                      </select>
                    </div>
                    <div className="form-group">
                      <label>Parity</label>
                      <select value={editDraft.parity ?? "none"} onChange={(e) => updateDraft("parity", e.target.value as "none" | "odd" | "even")}>
                        <option value="none">None</option>
                        <option value="odd">Odd</option>
                        <option value="even">Even</option>
                      </select>
                    </div>
                    <div className="form-group bookmark-editor-span-2">
                      <label>Flow Control</label>
                      <select value={editDraft.flowControl ?? "none"} onChange={(e) => updateDraft("flowControl", e.target.value as "none" | "hardware" | "software")}>
                        <option value="none">None</option>
                        <option value="hardware">Hardware</option>
                        <option value="software">Software</option>
                      </select>
                    </div>
                  </div>
                </>
              ) : (
                <>
                  <div className="bookmark-editor-grid">
                    <div className="form-group bookmark-editor-span-2">
                      <label>Host</label>
                      <input type="text" value={editDraft.host} onChange={(e) => updateDraft("host", e.target.value)} />
                    </div>
                    <div className="form-group">
                      <label>Port</label>
                      <input type="number" value={editDraft.port} onChange={(e) => updateDraft("port", Number(e.target.value) || 0)} />
                    </div>
                    {(editDraft.protocol ?? "ssh") === "ssh" ? (
                      <div className="form-group">
                        <label>User</label>
                        <input type="text" value={editDraft.user} onChange={(e) => updateDraft("user", e.target.value)} />
                      </div>
                    ) : (
                      <div className="form-group">
                        <label>协议</label>
                        <input type="text" value="Telnet" disabled />
                      </div>
                    )}
                  </div>

                  {(editDraft.protocol ?? "ssh") === "ssh" && (
                    <>
                      <div className="bookmark-editor-grid">
                        <div className="form-group">
                          <label>认证方式</label>
                          <select value={editDraft.authType} onChange={(e) => updateDraft("authType", e.target.value as "password" | "key" | "none")}>
                            <option value="password">Password</option>
                            <option value="key">Private Key</option>
                          </select>
                        </div>
                      </div>
                      {editDraft.authType === "password" ? (
                        <div className="form-group">
                          <label>Password</label>
                          <input type="password" value={editDraft.password ?? ""} onChange={(e) => updateDraft("password", e.target.value)} />
                        </div>
                      ) : (
                        <div className="form-group">
                          <label>Private Key (PEM)</label>
                          <textarea rows={5} value={editDraft.privateKey ?? ""} onChange={(e) => updateDraft("privateKey", e.target.value)} />
                        </div>
                      )}
                    </>
                  )}
                </>
              )}

              {editError && <div className="bookmark-editor-error">{editError}</div>}
            </div>

            <div className="bookmark-editor-footer">
              <button type="button" className="bookmark-editor-btn secondary" onClick={closeEditDialog}>取消</button>
              <button type="button" className="bookmark-editor-btn primary" onClick={() => void saveDraft()}>保存</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export type BookmarkSidebarRef = {
  refresh: () => void;
};
