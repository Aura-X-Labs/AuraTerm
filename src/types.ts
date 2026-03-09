export type ConnectionProtocol = "ssh" | "telnet" | "serial";

export type ReconnectType = "screen" | "tmux";

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
  /** Enable auto-reconnect via screen/tmux session on the remote host */
  autoReconnect?: boolean;
  /** Which multiplexer to use for session persistence */
  reconnectType?: ReconnectType;
}

export type RemoteTransferMode = "sftp" | "scp";
export type RemoteTransferDirection = "upload" | "download";
export type RemoteTransferStatus = "started" | "progress" | "completed" | "failed";

export interface RemoteTransferProgress {
  id: string;
  direction: RemoteTransferDirection;
  status: RemoteTransferStatus;
  mode: RemoteTransferMode;
  fileName: string;
  remotePath: string;
  localPath?: string | null;
  transferredBytes: number;
  totalBytes?: number | null;
  message?: string | null;
}

export interface RemoteFileEntry {
  name: string;
  path: string;
  kind: "file" | "directory" | "symlink" | "other";
  isDir: boolean;
  size: number;
  modifiedAt?: number | null;
  permissions: string;
}

export interface RemoteDirectoryListing {
  path: string;
  parent?: string | null;
  entries: RemoteFileEntry[];
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
  /** SSH auto-reconnect via screen/tmux */
  autoReconnect?: boolean;
  /** Which multiplexer to use for auto-reconnect */
  reconnectType?: ReconnectType;
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
  fit: () => void;
}