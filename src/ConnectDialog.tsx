import { useEffect, useState, FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./ConnectDialog.css";

export type ConnectionProtocol = "ssh" | "telnet" | "serial";

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
}

export interface ConnectResult {
  protocol: ConnectionProtocol;
  sshConfig?: SshConfig;
  telnetConfig?: TelnetConfig;
  serialConfig?: SerialConfig;
  saveAs?: string; // 非 undefined 表示需要保存，值为连接名称
  logPath?: string; // 非 undefined 表示开启持续写日志
}

export interface TelnetConfig {
  host: string;
  port: number;
}

export interface SerialConfig {
  portName: string;
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

interface SerialPortInfo {
  portName: string;
  portType: string;
  manufacturer?: string | null;
  serialNumber?: string | null;
  vid?: number | null;
  pid?: number | null;
}

interface ConnectDialogProps {
  initialProtocol?: ConnectionProtocol;
  onConnect: (result: ConnectResult) => void;
  onCancel: () => void;
}

export function ConnectDialog({ initialProtocol = "ssh", onConnect, onCancel }: ConnectDialogProps) {
  const [protocol, setProtocol] = useState<ConnectionProtocol>(initialProtocol);
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
  // Serial 字段
  const [serialPortName, setSerialPortName] = useState("");
  const [serialBaudRate, setSerialBaudRate] = useState("9600");
  const [serialDataBits, setSerialDataBits] = useState<"5" | "6" | "7" | "8">("8");
  const [serialStopBits, setSerialStopBits] = useState<"1" | "2">("1");
  const [serialParity, setSerialParity] = useState<"none" | "odd" | "even">("none");
  const [serialFlowControl, setSerialFlowControl] = useState<"none" | "hardware" | "software">("none");
  const [serialPorts, setSerialPorts] = useState<SerialPortInfo[]>([]);
  const [loadingSerialPorts, setLoadingSerialPorts] = useState(false);
  const [serialError, setSerialError] = useState("");
  // 日志字段
  const [enableLog, setEnableLog] = useState(true);
  const [logFilePath, setLogFilePath] = useState("");

  const isSsh = protocol === "ssh";
  const isTelnet = protocol === "telnet";
  const isSerial = protocol === "serial";

  useEffect(() => {
    setProtocol(initialProtocol);
  }, [initialProtocol]);

  const loadSerialPorts = async () => {
    setLoadingSerialPorts(true);
    setSerialError("");
    try {
      const ports = await invoke<SerialPortInfo[]>("list_serial_ports");
      setSerialPorts(ports);
      if (!serialPortName && ports.length > 0) {
        setSerialPortName(ports[0].portName);
      }
    } catch (error) {
      console.error("Failed to enumerate serial ports", error);
      setSerialError(String(error));
      setSerialPorts([]);
    } finally {
      setLoadingSerialPorts(false);
    }
  };

  useEffect(() => {
    if (isSerial) {
      void loadSerialPorts();
    }
  }, [isSerial]);

  // 当 host 或 user 变化时，自动填充默认连接名
  const defaultName = isSsh
    ? (user && host ? `${user}@${host}` : host)
    : isTelnet
      ? (host ? `telnet://${host}:${telnetPort}` : "")
      : (serialPortName ? `serial://${serialPortName}@${serialBaudRate}` : "");

  // 根据 defaultName 计算日志默认基础路径（无时间戳和后缀，连接时自动拼上）
  const safeDefaultName = defaultName
    .replace(/[^a-zA-Z0-9\-_@.]/g, "_")
    || "session";
  const defaultLogPath = `~/AuraTerm/logs/${safeDefaultName}`;

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if ((isSsh || isTelnet) && !host) return;
    if (isSsh && !user) return;
    if (isSerial && !serialPortName.trim()) return;

    const config: SshConfig = {
      host,
      port: parseInt(port, 10) || 22,
      user,
      password: authType === "password" ? password : undefined,
      privateKey: authType === "key" ? privateKey : undefined,
    };

    onConnect({
      protocol,
      sshConfig: isSsh ? config : undefined,
      telnetConfig: isTelnet ? { host, port: parseInt(telnetPort, 10) || 23 } : undefined,
      serialConfig: isSerial
        ? {
            portName: serialPortName.trim(),
            baudRate: parseInt(serialBaudRate, 10) || 9600,
            dataBits: parseInt(serialDataBits, 10) as 5 | 6 | 7 | 8,
            stopBits: parseInt(serialStopBits, 10) as 1 | 2,
            parity: serialParity,
            flowControl: serialFlowControl,
          }
        : undefined,
      saveAs: saveConnection ? (connectionName.trim() || defaultName) : undefined,
      logPath: enableLog ? (logFilePath.trim() || defaultLogPath) : undefined,
    });
  };

  const canConnect = isSsh
    ? Boolean(host.trim() && user.trim())
    : isTelnet
      ? Boolean(host.trim())
      : Boolean(serialPortName.trim());

  return (
    <div className="dialog-overlay">
      <div className="dialog-content">
        <h2 className="dialog-title">New Session</h2>
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
          <label className={isTelnet ? "active" : ""}>
            <input
              type="radio"
              name="protocol"
              checked={isTelnet}
              onChange={() => setProtocol("telnet")}
            />
            Telnet
          </label>
          <label className={isSerial ? "active" : ""}>
            <input
              type="radio"
              name="protocol"
              checked={isSerial}
              onChange={() => setProtocol("serial")}
            />
            Serial
          </label>
        </div>

        <form onSubmit={handleSubmit}>
          {(isSsh || isTelnet) && (
            <div className="form-group">
              <label>Host:</label>
              <input
                type="text"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                placeholder="e.g. 192.168.1.100"
                autoFocus
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                required
              />
            </div>
          )}

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
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  required
                />
              </div>
            </>
          ) : isTelnet ? (
            <div className="form-group">
              <label>Port:</label>
              <input
                type="number"
                value={telnetPort}
                onChange={(e) => setTelnetPort(e.target.value)}
                required
              />
            </div>
          ) : (
            <>
              <div className="form-group">
                <label>Serial Port:</label>
                <div className="serial-port-row">
                  <input
                    type="text"
                    value={serialPortName}
                    onChange={(e) => setSerialPortName(e.target.value)}
                    placeholder="e.g. /dev/cu.usbserial-1410"
                    autoFocus
                    list="serial-port-options"
                    required
                  />
                  <button
                    type="button"
                    className="serial-refresh-btn"
                    onClick={() => void loadSerialPorts()}
                    disabled={loadingSerialPorts}
                  >
                    {loadingSerialPorts ? "..." : "↻"}
                  </button>
                </div>
                <datalist id="serial-port-options">
                  {serialPorts.map((portInfo) => (
                    <option key={portInfo.portName} value={portInfo.portName}>
                      {portInfo.manufacturer ? `${portInfo.manufacturer} (${portInfo.portType})` : portInfo.portType}
                    </option>
                  ))}
                </datalist>
                {serialError ? (
                  <div className="form-hint error">串口枚举失败：{serialError}</div>
                ) : serialPorts.length > 0 ? (
                  <div className="form-hint">
                    已发现 {serialPorts.length} 个设备：{serialPorts.map((item) => item.portName).join(", ")}
                  </div>
                ) : (
                  <div className="form-hint">未发现串口设备，可手动输入设备路径后连接。</div>
                )}
              </div>

              <div className="form-group">
                <label>Baud Rate:</label>
                <input
                  type="number"
                  value={serialBaudRate}
                  onChange={(e) => setSerialBaudRate(e.target.value)}
                  min="1"
                  required
                />
              </div>

              <div className="serial-settings-grid">
                <div className="form-group">
                  <label>Data Bits:</label>
                  <select value={serialDataBits} onChange={(e) => setSerialDataBits(e.target.value as "5" | "6" | "7" | "8") }>
                    <option value="5">5</option>
                    <option value="6">6</option>
                    <option value="7">7</option>
                    <option value="8">8</option>
                  </select>
                </div>
                <div className="form-group">
                  <label>Stop Bits:</label>
                  <select value={serialStopBits} onChange={(e) => setSerialStopBits(e.target.value as "1" | "2") }>
                    <option value="1">1</option>
                    <option value="2">2</option>
                  </select>
                </div>
                <div className="form-group">
                  <label>Parity:</label>
                  <select value={serialParity} onChange={(e) => setSerialParity(e.target.value as "none" | "odd" | "even") }>
                    <option value="none">None</option>
                    <option value="odd">Odd</option>
                    <option value="even">Even</option>
                  </select>
                </div>
                <div className="form-group">
                  <label>Flow Control:</label>
                  <select value={serialFlowControl} onChange={(e) => setSerialFlowControl(e.target.value as "none" | "hardware" | "software") }>
                    <option value="none">None</option>
                    <option value="hardware">Hardware</option>
                    <option value="software">Software</option>
                  </select>
                </div>
              </div>
            </>
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
            <button type="submit" className="btn-connect" disabled={!canConnect}>
              Connect
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
