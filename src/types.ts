export type ConnectionProtocol = "ssh" | "telnet" | "serial" | "rfc2217" | "raw-tcp";

/** The protocols that all drive a UART, local or remote.
 *
 *  They share one config shape, one backend command set and one control
 *  surface; only the connect form differs, which is why they are separate
 *  entries in the protocol picker rather than a mode buried inside one. */
export type SerialProtocol = "serial" | "rfc2217" | "raw-tcp";

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

/** How a serial session reaches the UART.
 *
 *  `local` is a port on this machine. `rfc2217` speaks the Telnet Com Port
 *  Control Option to a device server, so line parameters actually take effect
 *  remotely. `raw-tcp` is a bare byte pipe with no parameter control at all. */
export type SerialTransport = "local" | "rfc2217" | "raw-tcp";

/** The five line parameters, shared by the connect form and the live status. */
export interface SerialParams {
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

export interface SerialConfig extends SerialParams {
  /** Absent means a local port, so workspaces and bookmarks saved before
   *  network serial existed keep loading unchanged. */
  transport?: SerialTransport;
  /** Local: the device path. Network: a derived `host:port` label, which keeps
   *  every existing display and log-naming path working as-is. */
  portName: string;
  host?: string;
  netPort?: number;
  /** RFC 2217 only: negotiate the option but do not push parameters, adopting
   *  whatever the device server is already configured with. Shared console
   *  servers need this — otherwise the first client to connect silently
   *  retunes the port for everyone already on it. */
  adoptServerParams?: boolean;
  /** Network transports only: come back automatically when the link drops.
   *  Defaults to on — a device server rebooting is routine, and a console that
   *  recovers by itself is the point. */
  autoReconnect?: boolean;
}

/** Which line parameters the peer explicitly confirmed. Without this the UI
 *  cannot tell "the server agreed to 115200" from "nobody ever answered". */
export interface SerialParamsConfirmed {
  baudRate: boolean;
  dataBits: boolean;
  stopBits: boolean;
  parity: boolean;
  flowControl: boolean;
}

/** Modem status lines reported by the peer. */
export interface SerialModemLines {
  cts: boolean;
  dsr: boolean;
  cd: boolean;
  ri: boolean;
}

/** Line status errors. Framing or parity errors are usually the most direct
 *  evidence that the baud rate is wrong. */
export interface SerialLineErrors {
  breakDetected: boolean;
  framing: boolean;
  parity: boolean;
  overrun: boolean;
}

/** The two output control lines, as last driven by this session.
 *
 *  They are outputs, so a local UART cannot read them back — this is what we
 *  set. RFC 2217 servers acknowledge them, and those acknowledgements win. */
export interface SerialSignals {
  dtr: boolean;
  rts: boolean;
}

/** Payload of the `serial-status:<id>` event. */
export interface SerialStatus {
  id: string;
  transport: SerialTransport;
  rfc2217Negotiated: boolean;
  /** True once the handshake has a verdict — agreed, refused, or timed out.
   *  A reconnect passes through "not agreed yet" on its way to a verdict, and
   *  those two states deserve very different words. */
  negotiationSettled: boolean;
  binaryNegotiated: boolean;
  requested: SerialParams;
  effective: SerialParams;
  confirmed: SerialParamsConfirmed;
  modem: SerialModemLines;
  signals: SerialSignals;
  lineErrors: SerialLineErrors;
  /** Whether this session can retune the line, send BREAK and drive DTR/RTS.
   *  False for raw TCP, and for RFC 2217 until the option is agreed. */
  controllable: boolean;
  signature?: string | null;
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

/** Where a bookmark came from when it was not created here.
 *
 *  `entryId` is the sharer's connection id and stays stable across re-exports
 *  of that bookmark; the local `id` is reassigned on every import, so it cannot
 *  serve as a cross-machine identity. Mirrors `BookmarkOrigin` in
 *  `connections.rs`. */
export interface BookmarkOrigin {
  bundleId: string;
  entryId: string;
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
  /** Local serial only: the device path. Network protocols keep their endpoint
   *  in `host`/`port` above rather than growing a parallel pair. */
  portName?: string;
  adoptServerParams?: boolean;
  serialAutoReconnect?: boolean;
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
  /** Set on bookmarks that arrived in a share bundle. */
  origin?: BookmarkOrigin;
}

/** Remote Assist guest tab: joins another AuraTerm's session with a code.
 *  Never persisted (the code is one-time and its secret must not be written
 *  to disk). */
export interface AssistGuestConfig {
  code: string;
  displayName?: string;
}

/** Live Relay: attach to a session another of the account's own devices
 *  already shares (design docs/plans/live-sync-design.md §5). Transient —
 *  a relay tab is never restored from the workspace snapshot. */
export interface RelayGuestConfig {
  deviceId: string;
  sessionId: string;
  deviceLabel: string;
  shareLabel: string;
  /** Ask for a ticket that may request control (writable shares only). */
  wantControl?: boolean;
}

export type SessionConfig =
  | { protocol: "local"; cwd?: string }
  | { protocol: "ssh"; sshConfig: SshConfig }
  | { protocol: "telnet"; telnetConfig: TelnetConfig }
  | { protocol: "serial"; serialConfig: SerialConfig }
  | { protocol: "rfc2217"; serialConfig: SerialConfig }
  | { protocol: "raw-tcp"; serialConfig: SerialConfig }
  | { protocol: "assist"; assistConfig: AssistGuestConfig }
  | { protocol: "relay"; relayConfig: RelayGuestConfig };

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
