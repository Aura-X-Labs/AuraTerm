import { useEffect, useMemo, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { SerialHistoryItem } from "./settings";
import "./ConnectDialog.css";

export type ConnectionProtocol = "ssh" | "telnet" | "serial";

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

export interface SerialConfig {
  portName: string;
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

export interface ConnectResult {
  protocol: ConnectionProtocol;
  sshConfig?: SshConfig;
  telnetConfig?: TelnetConfig;
  serialConfig?: SerialConfig;
  saveAs?: string;
  saveGroup?: string;
  logPath?: string;
}

interface SerialPortInfo {
  portName: string;
  portType: string;
  manufacturer?: string | null;
  serialNumber?: string | null;
  vid?: number | null;
  pid?: number | null;
}

interface SerialPresetOption {
  id: string;
  name: string;
  portName?: string;
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

interface ConnectDialogProps {
  initialProtocol?: ConnectionProtocol;
  lastSerialConfig?: SerialHistoryItem | null;
  recentSerialConfigs?: SerialHistoryItem[];
  onConnect: (result: ConnectResult) => void;
  onCancel: () => void;
}

const BUILTIN_SERIAL_PRESETS: SerialPresetOption[] = [
  { id: "builtin-115200-8n1", name: "115200 · 8N1", baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-9600-8n1", name: "9600 · 8N1", baudRate: 9600, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-57600-8n1", name: "57600 · 8N1", baudRate: 57600, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-38400-8n1", name: "38400 · 8N1", baudRate: 38400, dataBits: 8, stopBits: 1, parity: "none", flowControl: "none" },
  { id: "builtin-9600-7e1", name: "9600 · 7E1", baudRate: 9600, dataBits: 7, stopBits: 1, parity: "even", flowControl: "none" },
];

function applySerialOption(option: Pick<SerialPresetOption, "portName" | "baudRate" | "dataBits" | "stopBits" | "parity" | "flowControl">, setPortName: (value: string) => void, setBaudRate: (value: string) => void, setDataBits: (value: "5" | "6" | "7" | "8") => void, setStopBits: (value: "1" | "2") => void, setParity: (value: "none" | "odd" | "even") => void, setFlowControl: (value: "none" | "hardware" | "software") => void) {
  if (option.portName) setPortName(option.portName);
  setBaudRate(String(option.baudRate));
  setDataBits(String(option.dataBits) as "5" | "6" | "7" | "8");
  setStopBits(String(option.stopBits) as "1" | "2");
  setParity(option.parity);
  setFlowControl(option.flowControl);
}

export function ConnectDialog({
  initialProtocol = "ssh",
  lastSerialConfig = null,
  recentSerialConfigs = [],
  onConnect,
  onCancel,
}: ConnectDialogProps) {
  const [protocol, setProtocol] = useState<ConnectionProtocol>(initialProtocol);
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [authType, setAuthType] = useState<"password" | "key">("password");
  const [saveConnection, setSaveConnection] = useState(true);
  const [connectionName, setConnectionName] = useState("");
  const [connectionGroup, setConnectionGroup] = useState("");
  const [telnetPort, setTelnetPort] = useState("23");
  const [serialPortName, setSerialPortName] = useState(lastSerialConfig?.portName ?? "");
  const [serialBaudRate, setSerialBaudRate] = useState(String(lastSerialConfig?.baudRate ?? 9600));
  const [serialDataBits, setSerialDataBits] = useState<"5" | "6" | "7" | "8">(String(lastSerialConfig?.dataBits ?? 8) as "5" | "6" | "7" | "8");
  const [serialStopBits, setSerialStopBits] = useState<"1" | "2">(String(lastSerialConfig?.stopBits ?? 1) as "1" | "2");
  const [serialParity, setSerialParity] = useState<"none" | "odd" | "even">(lastSerialConfig?.parity ?? "none");
  const [serialFlowControl, setSerialFlowControl] = useState<"none" | "hardware" | "software">(lastSerialConfig?.flowControl ?? "none");
  const [selectedSerialPresetId, setSelectedSerialPresetId] = useState("custom");
  const [serialPorts, setSerialPorts] = useState<SerialPortInfo[]>([]);
  const [loadingSerialPorts, setLoadingSerialPorts] = useState(false);
  const [serialError, setSerialError] = useState("");
  const [enableLog, setEnableLog] = useState(true);
  const [logFilePath, setLogFilePath] = useState("");

  const isSsh = protocol === "ssh";
  const isTelnet = protocol === "telnet";
  const isSerial = protocol === "serial";

  const recentPresetOptions = useMemo<SerialPresetOption[]>(() => {
    const seen = new Set<string>();
    return recentSerialConfigs
      .filter((item) => {
        const key = `${item.portName}|${item.baudRate}|${item.dataBits}|${item.stopBits}|${item.parity}|${item.flowControl}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .map((item) => ({
        id: item.id,
        name: item.name,
        portName: item.portName,
        baudRate: item.baudRate,
        dataBits: item.dataBits,
        stopBits: item.stopBits,
        parity: item.parity,
        flowControl: item.flowControl,
      }));
  }, [recentSerialConfigs]);

  const serialPresetOptions = useMemo(() => [...recentPresetOptions, ...BUILTIN_SERIAL_PRESETS], [recentPresetOptions]);

  useEffect(() => {
    setProtocol(initialProtocol);
    if (lastSerialConfig) {
      applySerialOption(
        lastSerialConfig,
        setSerialPortName,
        setSerialBaudRate,
        setSerialDataBits,
        setSerialStopBits,
        setSerialParity,
        setSerialFlowControl,
      );
    }
  }, [initialProtocol, lastSerialConfig]);

  useEffect(() => {
    const matched = serialPresetOptions.find((option) =>
      option.portName === undefined || option.portName === serialPortName
        ? option.baudRate === (parseInt(serialBaudRate, 10) || 9600) &&
          option.dataBits === (parseInt(serialDataBits, 10) as 5 | 6 | 7 | 8) &&
          option.stopBits === (parseInt(serialStopBits, 10) as 1 | 2) &&
          option.parity === serialParity &&
          option.flowControl === serialFlowControl &&
          (option.portName ? option.portName === serialPortName : true)
        : false
    );
    setSelectedSerialPresetId(matched?.id ?? "custom");
  }, [serialBaudRate, serialDataBits, serialFlowControl, serialParity, serialPortName, serialPresetOptions, serialStopBits]);

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

  const defaultName = isSsh
    ? (user && host ? `${user}@${host}` : host)
    : isTelnet
      ? (host ? `telnet://${host}:${telnetPort}` : "")
      : (serialPortName ? `serial://${serialPortName}@${serialBaudRate}` : "");

  const safeDefaultName = defaultName.replace(/[^a-zA-Z0-9\-_@.]/g, "_") || "session";
  const defaultLogPath = `~/AuraTerm/logs/${safeDefaultName}`;

  const handleSerialPresetChange = (presetId: string) => {
    setSelectedSerialPresetId(presetId);
    if (presetId === "custom") return;
    const preset = serialPresetOptions.find((item) => item.id === presetId);
    if (!preset) return;
    applySerialOption(
      preset,
      setSerialPortName,
      setSerialBaudRate,
      setSerialDataBits,
      setSerialStopBits,
      setSerialParity,
      setSerialFlowControl,
    );
  };

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if ((isSsh || isTelnet) && !host.trim()) return;
    if (isSsh && !user.trim()) return;
    if (isSerial && !serialPortName.trim()) return;

    const sshConfig: SshConfig = {
      host,
      port: parseInt(port, 10) || 22,
      user,
      password: authType === "password" ? password : undefined,
      privateKey: authType === "key" ? privateKey : undefined,
    };

    onConnect({
      protocol,
      sshConfig: isSsh ? sshConfig : undefined,
      telnetConfig: isTelnet ? { host, port: parseInt(telnetPort, 10) || 23 } : undefined,
      serialConfig: isSerial ? {
        portName: serialPortName.trim(),
        baudRate: parseInt(serialBaudRate, 10) || 9600,
        dataBits: parseInt(serialDataBits, 10) as 5 | 6 | 7 | 8,
        stopBits: parseInt(serialStopBits, 10) as 1 | 2,
        parity: serialParity,
        flowControl: serialFlowControl,
      } : undefined,
      saveAs: saveConnection ? (connectionName.trim() || defaultName) : undefined,
      saveGroup: saveConnection && connectionGroup.trim() ? connectionGroup.trim() : undefined,
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
      <div className="dialog-content dialog-content--wide">
        <h2 className="dialog-title">New Session</h2>
        <div className="protocol-selector">
          <label className={isSsh ? "active" : ""}>
            <input type="radio" name="protocol" checked={isSsh} onChange={() => setProtocol("ssh")} />
            SSH
          </label>
          <label className={isTelnet ? "active" : ""}>
            <input type="radio" name="protocol" checked={isTelnet} onChange={() => setProtocol("telnet")} />
            Telnet
          </label>
          <label className={isSerial ? "active" : ""}>
            <input type="radio" name="protocol" checked={isSerial} onChange={() => setProtocol("serial")} />
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
              <div className="two-column-grid">
                <div className="form-group">
                  <label>Port:</label>
                  <input type="number" value={port} onChange={(e) => setPort(e.target.value)} required />
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
              </div>
            </>
          ) : isTelnet ? (
            <div className="form-group">
              <label>Port:</label>
              <input type="number" value={telnetPort} onChange={(e) => setTelnetPort(e.target.value)} required />
            </div>
          ) : (
            <>
              <div className="serial-settings-grid serial-settings-grid--compact">
                <div className="form-group serial-settings-grid-span-2">
                  <label>Preset:</label>
                  <select value={selectedSerialPresetId} onChange={(e) => handleSerialPresetChange(e.target.value)}>
                    <option value="custom">Custom</option>
                    {recentPresetOptions.length > 0 && (
                      <optgroup label="Recent">
                        {recentPresetOptions.map((preset) => (
                          <option key={preset.id} value={preset.id}>{preset.name}</option>
                        ))}
                      </optgroup>
                    )}
                    <optgroup label="Common">
                      {BUILTIN_SERIAL_PRESETS.map((preset) => (
                        <option key={preset.id} value={preset.id}>{preset.name}</option>
                      ))}
                    </optgroup>
                  </select>
                </div>

                <div className="form-group serial-settings-grid-span-2">
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
                    <button type="button" className="serial-refresh-btn" onClick={() => void loadSerialPorts()} disabled={loadingSerialPorts}>
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
                    <div className="form-hint">已发现 {serialPorts.length} 个设备：{serialPorts.map((item) => item.portName).join(", ")}</div>
                  ) : (
                    <div className="form-hint">未发现串口设备，可手动输入设备路径后连接。</div>
                  )}
                </div>

                <div className="form-group">
                  <label>Baud Rate:</label>
                  <input type="number" value={serialBaudRate} onChange={(e) => setSerialBaudRate(e.target.value)} min="1" required />
                </div>
                <div className="form-group">
                  <label>Data Bits:</label>
                  <select value={serialDataBits} onChange={(e) => setSerialDataBits(e.target.value as "5" | "6" | "7" | "8")}>
                    <option value="5">5</option>
                    <option value="6">6</option>
                    <option value="7">7</option>
                    <option value="8">8</option>
                  </select>
                </div>
                <div className="form-group">
                  <label>Stop Bits:</label>
                  <select value={serialStopBits} onChange={(e) => setSerialStopBits(e.target.value as "1" | "2")}>
                    <option value="1">1</option>
                    <option value="2">2</option>
                  </select>
                </div>
                <div className="form-group">
                  <label>Parity:</label>
                  <select value={serialParity} onChange={(e) => setSerialParity(e.target.value as "none" | "odd" | "even")}>
                    <option value="none">None</option>
                    <option value="odd">Odd</option>
                    <option value="even">Even</option>
                  </select>
                </div>
                <div className="form-group serial-settings-grid-span-2">
                  <label>Flow Control:</label>
                  <select value={serialFlowControl} onChange={(e) => setSerialFlowControl(e.target.value as "none" | "hardware" | "software")}>
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
                <select value={authType} onChange={(e) => setAuthType(e.target.value as "password" | "key")}>
                  <option value="password">Password</option>
                  <option value="key">Private Key</option>
                </select>
              </div>

              {authType === "password" ? (
                <div className="form-group">
                  <label>Password:</label>
                  <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
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

          <div className="form-group save-connection-group">
            <label className="save-connection-label">
              <input type="checkbox" checked={saveConnection} onChange={(e) => setSaveConnection(e.target.checked)} />
              <span>保存此连接</span>
            </label>
            {saveConnection && (
              <div className="two-column-grid">
                <input
                  type="text"
                  className="save-connection-name"
                  value={connectionName}
                  onChange={(e) => setConnectionName(e.target.value)}
                  placeholder={defaultName || "连接名称（可选）"}
                />
                <input
                  type="text"
                  className="save-connection-name"
                  value={connectionGroup}
                  onChange={(e) => setConnectionGroup(e.target.value)}
                  placeholder="分组（可选）"
                />
              </div>
            )}
          </div>

          <div className="form-group save-connection-group">
            <label className="save-connection-label">
              <input type="checkbox" checked={enableLog} onChange={(e) => setEnableLog(e.target.checked)} />
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
            <button type="button" className="btn-cancel" onClick={onCancel}>Cancel</button>
            <button type="submit" className="btn-connect" disabled={!canConnect}>Connect</button>
          </div>
        </form>
      </div>
    </div>
  );
}
