import { useState, FormEvent } from "react";
import "./ConnectDialog.css";

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
}

export interface TelnetConfig {
  host: string;
  port: number;
}

export interface ConnectResult {
  protocol: "ssh" | "telnet";
  sshConfig?: SshConfig;
  telnetConfig?: TelnetConfig;
  saveAs?: string; // 非 undefined 表示需要保存，值为连接名称
}

type Protocol = "ssh" | "telnet";

interface ConnectDialogProps {
  onConnect: (result: ConnectResult) => void;
  onCancel: () => void;
}

export function ConnectDialog({ onConnect, onCancel }: ConnectDialogProps) {
  const [protocol, setProtocol] = useState<Protocol>("ssh");

  // 共用字段
  const [host, setHost] = useState("");
  // SSH 字段
  const [sshPort, setSshPort] = useState("22");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [authType, setAuthType] = useState<"password" | "key">("password");
  const [saveConnection, setSaveConnection] = useState(true);
  const [connectionName, setConnectionName] = useState("");
  // Telnet 字段
  const [telnetPort, setTelnetPort] = useState("23");

  const isSsh = protocol === "ssh";

  const defaultName = isSsh && user && host
    ? `${user}@${host}`
    : host ? `telnet://${host}:${telnetPort}` : "";

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!host) return;

    if (protocol === "telnet") {
      onConnect({
        protocol: "telnet",
        telnetConfig: { host, port: parseInt(telnetPort, 10) || 23 },
        saveAs: saveConnection ? (connectionName.trim() || `telnet://${host}:${telnetPort}`) : undefined,
      });
      return;
    }

    // SSH
    if (!user) return;
    const config: SshConfig = {
      host,
      port: parseInt(sshPort, 10) || 22,
      user,
      password: authType === "password" ? password : undefined,
      privateKey: authType === "key" ? privateKey : undefined,
    };
    onConnect({
      protocol: "ssh",
      sshConfig: config,
      saveAs: saveConnection ? (connectionName.trim() || defaultName) : undefined,
    });
  };

  return (
    <div className="dialog-overlay">
      <div className="dialog-content">
        <h2 className="dialog-title">新建连接</h2>
        <form onSubmit={handleSubmit}>
          {/* 协议选择 */}
          <div className="form-group">
            <label>协议:</label>
            <div className="protocol-tabs">
              <button
                type="button"
                className={`protocol-tab-btn${protocol === "ssh" ? " active" : ""}`}
                onClick={() => setProtocol("ssh")}
              >
                SSH
              </button>
              <button
                type="button"
                className={`protocol-tab-btn${protocol === "telnet" ? " active" : ""}`}
                onClick={() => setProtocol("telnet")}
              >
                Telnet
              </button>
            </div>
          </div>

          {/* 主机 */}
          <div className="form-group">
            <label>主机:</label>
            <input
              type="text"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="例如 192.168.1.100"
              autoFocus
              required
            />
          </div>

          {/* 端口 */}
          <div className="form-group">
            <label>端口:</label>
            <input
              type="number"
              value={isSsh ? sshPort : telnetPort}
              onChange={(e) => isSsh ? setSshPort(e.target.value) : setTelnetPort(e.target.value)}
              required
            />
          </div>

          {/* SSH 专用字段 */}
          {isSsh && (
            <>
              <div className="form-group">
                <label>用户名:</label>
                <input
                  type="text"
                  value={user}
                  onChange={(e) => setUser(e.target.value)}
                  required
                />
              </div>
              <div className="form-group auth-type-group">
                <label>认证方式:</label>
                <select
                  value={authType}
                  onChange={(e) => setAuthType(e.target.value as "password" | "key")}
                >
                  <option value="password">密码</option>
                  <option value="key">私钥</option>
                </select>
              </div>

              {authType === "password" ? (
                <div className="form-group">
                  <label>密码:</label>
                  <input
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </div>
              ) : (
                <div className="form-group">
                  <label>私钥 (PEM):</label>
                  <textarea
                    value={privateKey}
                    onChange={(e) => setPrivateKey(e.target.value)}
                    placeholder="-----BEGIN RSA PRIVATE KEY-----..."
                    rows={4}
                  />
                  <input
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="Key Passphrase（可选）"
                    style={{ marginTop: "8px" }}
                  />
                </div>
              )}
            </>
          )}

          {/* 保存此连接 — SSH 和 Telnet 均支持 */}
          <div className="form-group save-connection-group">
            <label className="save-connection-label">
              <input
                type="checkbox"
                checked={saveConnection}
                onChange={(e) => setSaveConnection(e.target.checked)}
              />
              <span>保存此连接</span>
            </label>
            {saveConnection && (
              <input
                type="text"
                className="save-connection-name"
                value={connectionName}
                onChange={(e) => setConnectionName(e.target.value)}
                placeholder={defaultName || "连接名称（可选）"}
              />
            )}
          </div>

          <div className="dialog-actions">
            <button type="button" className="btn-cancel" onClick={onCancel}>
              取消
            </button>
            <button
              type="submit"
              className="btn-connect"
              disabled={!host || (isSsh && !user)}
            >
              连接
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
