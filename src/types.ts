export type ConnectionProtocol = "ssh" | "telnet" | "serial";

/** A single action surfaced in the command palette (Ctrl/Cmd+Shift+P). */
export interface PaletteCommand {
  id: string;
  title: string;
  subtitle?: string;
  group?: string;
  /** Extra terms folded into fuzzy matching but not displayed. */
  keywords?: string;
  /** Whether the action can run right now; disabled commands are hidden. */
  enabled?: boolean;
  run: () => void | Promise<void>;
}

export type ReconnectType = "manual" | "simple" | "screen" | "tmux";

type ReconnectConfigLike = {
  autoReconnect?: boolean;
  reconnectType?: ReconnectType | string;
};

export function normalizeReconnectType(config?: ReconnectConfigLike | null): ReconnectType {
  const reconnectType = config?.reconnectType;
  if (reconnectType === "manual" || reconnectType === "simple" || reconnectType === "screen" || reconnectType === "tmux") {
    return reconnectType;
  }

  return config?.autoReconnect ? "tmux" : "manual";
}

export function isReconnectEnabled(reconnectType: ReconnectType): boolean {
  return reconnectType !== "manual";
}

export type SshAuthType = "password" | "key" | "agent" | "none";

export interface JumpHostConfig {
  id: string;
  host: string;
  port: number;
  user: string;
  authType: SshAuthType;
  password?: string;
  privateKey?: string;
  passphrase?: string;
}

export interface AutoLoginRule {
  expect: string;
  response?: string;
  caseSensitive?: boolean;
  timeoutSecs?: number;
}

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
  passphrase?: string;
  authType?: SshAuthType;
  agentForwarding?: boolean;
  jumpHosts?: JumpHostConfig[];
  autoLoginRules?: AutoLoginRule[];
  postConnectCommands?: string[];
  savedConnectionId?: string;
  savedConnectionGroup?: string;
  /** Legacy compatibility field; reconnect behavior is driven by reconnectType */
  autoReconnect?: boolean;
  /** Reconnect behavior for SSH sessions */
  reconnectType?: ReconnectType;
  /** Port-forwarding tunnels configured for this session. */
  tunnels?: TunnelConfig[];
}

/** Forwarding direction, mirroring OpenSSH's `-L` / `-R` / `-D`. */
export type TunnelType = "local" | "remote" | "dynamic";

export type TunnelRuntimeStatus = "starting" | "active" | "error" | "stopped";

export interface TunnelConfig {
  id: string;
  type: TunnelType;
  /** Optional friendly label shown in the tunnel manager. */
  name?: string;
  /** Where to listen. Local/dynamic: a local interface (default 127.0.0.1).
   *  Remote: the bind address on the SSH server. */
  bindAddress?: string;
  bindPort: number;
  /** Destination host:port. Used by local (`-L`) and remote (`-R`); unused for
   *  dynamic (`-D`), where the SOCKS client chooses the destination. */
  destHost?: string;
  destPort?: number;
  /** Auto-start this tunnel once the SSH session connects. */
  autoStart?: boolean;
}

/** Payload of the global `ssh-tunnel-status` event emitted by the backend. */
export interface TunnelStatusEvent {
  sessionId: string;
  tunnelId: string;
  tunnelType: TunnelType;
  status: TunnelRuntimeStatus;
  message?: string | null;
}

/** Result of `ssh_list_tunnels`: which tunnels are currently running. */
export interface ActiveTunnelInfo {
  tunnelId: string;
  status: TunnelRuntimeStatus;
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

export type ZmodemDirection = "upload" | "download";
export type ZmodemStatus = "detected" | "started" | "progress" | "completed" | "failed" | "cancelled";

export interface ZmodemTransferEvent {
  id: string;
  direction: ZmodemDirection;
  status: ZmodemStatus;
  fileName?: string | null;
  localPath?: string | null;
  transferredBytes: number;
  totalBytes?: number | null;
  message?: string | null;
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
  logPath?: string;
  protocol?: ConnectionProtocol;
  host: string;
  port: number;
  user: string;
  authType: SshAuthType;
  password?: string;
  privateKey?: string;
  passphrase?: string;
  agentForwarding?: boolean;
  jumpHosts?: JumpHostConfig[];
  autoLoginRules?: AutoLoginRule[];
  postConnectCommands?: string[];
  portName?: string;
  baudRate?: number;
  dataBits?: 5 | 6 | 7 | 8;
  stopBits?: 1 | 2;
  parity?: "none" | "odd" | "even";
  flowControl?: "none" | "hardware" | "software";
  createdAt: number;
  lastUsed?: number;
  /** Legacy compatibility field; reconnect behavior is driven by reconnectType */
  autoReconnect?: boolean;
  /** Reconnect behavior for SSH sessions */
  reconnectType?: ReconnectType;
  /** Saved port-forwarding tunnels (SSH only). */
  tunnels?: TunnelConfig[];
}

/** Remote Assist guest tab: joins another AuraTerm's session with a code.
 *  Never persisted (the code is one-time and its secret must not be written
 *  to disk). */
export interface AssistGuestConfig {
  code: string;
  displayName?: string;
}

export type SessionConfig =
  | { protocol: "local"; cwd?: string }
  | { protocol: "ssh"; sshConfig: SshConfig }
  | { protocol: "telnet"; telnetConfig: TelnetConfig }
  | { protocol: "serial"; serialConfig: SerialConfig }
  | { protocol: "assist"; assistConfig: AssistGuestConfig };

export type SerialConnectionState = "idle" | "connecting" | "connected" | "closed" | "error";

export interface TerminalSearchOptions {
  caseSensitive?: boolean;
  wholeWord?: boolean;
  regex?: boolean;
  incremental?: boolean;
}

export interface TerminalSearchResults {
  query: string;
  resultIndex: number;
  resultCount: number;
  limitExceeded: boolean;
}

export interface TerminalHandle {
  saveLog: (tabTitle: string) => Promise<string>;
  sendData: (text: string) => void;
  /** Write raw input to this session's pty, bypassing the newline append of
   *  sendData. Used to fan out broadcast (MultiExec) keystrokes to target panes. */
  writeInput: (data: string) => void;
  fit: () => void;
  focus: () => void;
  findNext: (text: string, options?: TerminalSearchOptions) => boolean;
  findPrevious: (text: string, options?: TerminalSearchOptions) => boolean;
  clearSearch: () => void;
  clearSearchActiveDecoration: () => void;
  previousCommand: () => boolean;
  nextCommand: () => boolean;
  rerunLastCommand: () => boolean;
  copyLastCommand: () => Promise<boolean>;
  /** Command + captured output of the most recent (optionally failed) shell
   *  command block, for the AI assistant. Null without shell integration data. */
  lastCommandContext: (failedOnly?: boolean) => import("./aiContext").CommandContext | null;
  /** Plain text of the rows currently visible in the viewport, for the AI
   *  assistant's "summarize output" action. Works without shell integration;
   *  null when the viewport is blank. */
  visibleOutputContext: () => import("./aiContext").OutputContext | null;
}

/** Quick actions offered as chips in the AI assistant panel. */
export type AiQuickAction = "explain" | "explain-error" | "optimize" | "summarize";
