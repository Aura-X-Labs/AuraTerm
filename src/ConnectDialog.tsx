import { useState, FormEvent } from "react";
import "./ConnectDialog.css";

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
}

export interface ConnectResult {
  protocol: "ssh" | "telnet";
  sshConfig?: SshConfig;
  telnetConfig?: TelnetConfig;
  saveAs?: string; // 非 undefined 表示需要保存，值为连接名称
  logPath?: string; // 非 undefined 表示开启持续写日志
}

export interface TelnetConfig {
  host: string;
  port: number;
}

interface ConnectDialogProps {
  onConnect: (result: ConnectResult) => void;
  onCancel: () => void;
}

export function ConnectDialog({ onConnect, onCancel }: ConnectDialogProps) {
  const [protocol, setProtocol] = useState<"ssh" | "telnet">("ssh");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [authType, setAuthType] = useState<"password" | "key">("password");
  const [saveConnection, setSaveConnection] = useState(true);
  const [connectionName, setConnectionName] = useState("");
  // Telnet 字段
  const [telnetPort, setTelnetPort] = useState("23");
  // 日志字段
  const [enableLog, setEnableLog] = useState(true);
  const [logFilePath, setLogFilePath] = useState("");

  const isSsh = protocol === "ssh";

  // 当 host 或 user 变化时，自动填充默认连接名
  const defaultName = isSsh && user && host
    ? `${user}@${host}`
    : host ? `telnet://${host}:${telnetPort}` : "";

  // 根据 defaultName 计算日志默认基础路径（无时间戳和后缀，连接时自动拼上）
  const safeDefaultName = defaultName
    .replace(/[^a-zA-Z0-9\-_@.]/g, "_")
    || "session";
  const defaultLogPath = `~/AuraTerm/logs/${safeDefaultName}`;

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!host) return;
    if (isSsh && !user) return;

    const config: SshConfig = {
      host,
      port: parseInt(port, 10) || 22,
      user,
      password: authType === "password" ? password : undefined,
      privateKey: authType === "key" ? privateKey : undefined,
    };

    onConnect({
      protocol: isSsh ? "ssh" : "telnet",
      sshConfig: isSsh ? config : undefined,
      telnetConfig: protocol === "telnet" ? { host, port: parseInt(telnetPort, 10) || 23 } : undefined,
      saveAs: saveConnection ? (connectionName.trim() || defaultName) : undefined,
      logPath: enableLog ? (logFilePath.trim() || defaultLogPath) : undefined,
    });
  };

  return (
    <div className="dialog-overlay">
      <div className="dialog-content">
        <h2 className="dialog-title">New Connection</h2>
        <div className="protocol-selector">
          <label className={isSsh ? "active" : ""}>
            <input
              type="radio"
              name="protocol"
              checked={isSsh}
              onChange={() => setProtocol("ssh")}
            />
            SSH
          </label>
          <label className={!isSsh ? "active" : ""}>
            <input
              type="radio"
              name="protocol"
              checked={!isSsh}
              onChange={() => setProtocol("telnet")}
            />
            Telnet
          </label>
        </div>

        <form onSubmit={handleSubmit}>
          <div className="form-group">
            <label>Host:</label>
            <input
              type="text"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="e.g. 192.168.1.100"
              autoFocus
              required
            />
          </div>

          {isSsh ? (
            <>
              <div className="form-group">
                <label>Port:</label>
                <input
                  type="number"
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  required
                />
              </div>
              <div className="form-group">
                <label>User:</label>
                <input
                  type="text"
                  value={user}
                  onChange={(e) => setUser(e.target.value)}
                  required
                />
              </div>
            </>
          ) : (
            <div className="form-group">
              <label>Port:</label>
              <input
                type="number"
                value={telnetPort}
                onChange={(e) => setTelnetPort(e.target.value)}
                required
              />
            </div>
          )}

          {isSsh && (
            <>
              <div className="form-group auth-type-group">
                <label>Auth Type:</label>
                <select
                  value={authType}
                  onChange={(e) => setAuthType(e.target.value as "password" | "key")}
                >
                  <option value="password">Password</option>
                  <option value="key">Private Key</option>
                </select>
              </div>

              {authType === "password" ? (
                <div className="form-group">
                  <label>Password:</label>
                  <input
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                </div>
              ) : (
                <div className="form-group">
                  <label>Private Key (PEM):</label>
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
                    placeholder="Key Passphrase (optional)"
                    style={{ marginTop: "8px" }}
                  />
                </div>
              )}
            </>
          )}

          {/* 保存连接选项 */}
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

          {/* 日志选项 */}
          <div className="form-group save-connection-group">
            <label className="save-connection-label">
              <input
                type="checkbox"
                checked={enableLog}
                onChange={(e) => setEnableLog(e.target.checked)}
              />
              <span>保存会话日志</span>
            </label>
            {enableLog && (
              <input
                type="text"
                className="save-connection-name"
                value={logFilePath}
                onChange={(e) => setLogFilePath(e.target.value)}
                placeholder={`${defaultLogPath}_YYYYMMDD_HHmmss.log`}
              />
            )}
          </div>

          <div className="dialog-actions">
            <button type="button" className="btn-cancel" onClick={onCancel}>
              Cancel
            </button>
            <button type="submit" className="btn-connect" disabled={!host || !user}>
              Connect
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
