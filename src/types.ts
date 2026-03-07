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

export interface SavedConnection {
  id: string;
  name: string;
  group?: string;
  protocol?: ConnectionProtocol;
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

export type SessionConfig =
  | { protocol: "local" }
  | { protocol: "ssh"; sshConfig: SshConfig }
  | { protocol: "telnet"; telnetConfig: TelnetConfig }
  | { protocol: "serial"; serialConfig: SerialConfig };

export type SerialConnectionState = "idle" | "connecting" | "connected" | "closed" | "error";

export interface TerminalHandle {
  saveLog: (tabTitle: string) => Promise<string>;
  sendData: (text: string) => void;
}