import { invoke } from "@tauri-apps/api/core";
import { isReconnectEnabled, normalizeReconnectType } from "../types";
import type {
  ReconnectType,
  SavedConnection,
  SerialConfig,
  SerialParams,
  SessionConfig,
  SshConfig,
} from "../types";

// ── Serial control surface ───────────────────────────────────────────────────
//
// Exported at module scope rather than from the composable: these need none of
// its closure state, and the status bar and command palette in App.vue drive
// them directly.

/** Retune a live serial session. No reconnect, so nothing the device printed in
 *  the meantime is lost — which is the whole point when you are guessing a baud
 *  rate. */
export async function setSerialParams(sessionId: string, params: SerialParams) {
  await invoke("set_serial_params", {
    id: sessionId,
    baudRate: params.baudRate,
    dataBits: params.dataBits,
    stopBits: params.stopBits,
    parity: params.parity,
    flowControl: params.flowControl,
  });
}

/** Hold BREAK on the line, then release it. This is how a device that is not
 *  reading characters — U-Boot, Cisco ROMMON, a Solaris OK prompt — is
 *  interrupted. */
export async function sendSerialBreak(sessionId: string, durationMs?: number) {
  await invoke("send_serial_break", { id: sessionId, durationMs: durationMs ?? null });
}

/** Drive DTR and/or RTS; omitted lines are left alone. */
export async function setSerialSignals(sessionId: string, signals: { dtr?: boolean; rts?: boolean }) {
  await invoke("set_serial_signals", {
    id: sessionId,
    dtr: signals.dtr ?? null,
    rts: signals.rts ?? null,
  });
}

export async function purgeSerialBuffers(
  sessionId: string,
  target: "input" | "output" | "both" = "both",
) {
  await invoke("purge_serial_buffers", { id: sessionId, target });
}

interface UseTerminalSessionCommandsOptions {
  onSshPasswordUpdated: () => void;
}

export function useTerminalSessionCommands({ onSshPasswordUpdated }: UseTerminalSessionCommandsOptions) {
  function writeSessionInput(id: string, data: string, session: SessionConfig) {
    switch (session.protocol) {
      case "ssh":
        return invoke("write_ssh_pty_input", { id, data });
      case "telnet":
        return invoke("write_telnet_input", { id, data });
      case "serial":
        return invoke("write_serial_input", { id, data });
      case "local":
        return invoke("write_pty_input", { id, data });
      case "assist":
        return invoke("write_assist_input", { id, data });
    }
  }

  function resizeSession(id: string, cols: number, rows: number, session: SessionConfig) {
    if (session.protocol === "assist") {
      // A guest tab follows the host's grid; it never resizes anything.
      return Promise.resolve();
    }
    // Cloud Console viewers and Remote Assist guests follow the host's grid;
    // the bridge only sends RESIZE when the size actually changed.
    void invoke("cloud_bridge_report_size", { localSessionId: id, cols, rows }).catch(() => {});
    switch (session.protocol) {
      case "ssh":
        return invoke("resize_ssh_pty", { id, cols, rows });
      case "local":
        return invoke("resize_pty", { id, cols, rows });
      case "telnet":
        return invoke("resize_telnet", { id, cols, rows });
      case "serial":
        return Promise.resolve();
    }
  }

  function closeSession(id: string, session: SessionConfig) {
    switch (session.protocol) {
      case "ssh":
        return invoke("close_ssh_pty", { id });
      case "telnet":
        return invoke("close_telnet_session", { id });
      case "serial":
        return invoke("close_serial_session", { id });
      case "local":
        return invoke("close_pty", { id });
      case "assist":
        return invoke("close_assist_session", { id });
    }
  }

  function getSshReconnectType(sshConfig: SshConfig): ReconnectType {
    return normalizeReconnectType(sshConfig);
  }

  async function startSshSession(sessionId: string, sshConfig: SshConfig, cols: number, rows: number) {
    const reconnectType = getSshReconnectType(sshConfig);
    await invoke("start_ssh_pty", {
      id: sessionId,
      host: sshConfig.host,
      port: sshConfig.port,
      user: sshConfig.user,
      password: sshConfig.password ?? null,
      privateKey: sshConfig.privateKey ?? null,
      passphrase: sshConfig.passphrase ?? null,
      authType: sshConfig.authType ?? null,
      agentForwarding: sshConfig.agentForwarding ?? false,
      jumpHosts: sshConfig.jumpHosts ?? [],
      autoLoginRules: sshConfig.autoLoginRules ?? [],
      postConnectCommands: sshConfig.postConnectCommands ?? [],
      cols,
      rows,
      autoReconnect: isReconnectEnabled(reconnectType),
      reconnectType,
    });
  }

  async function startTelnetSession(sessionId: string, host: string, port: number, cols: number, rows: number) {
    await invoke("start_telnet_session", {
      id: sessionId,
      host,
      port,
      cols,
      rows,
    });
  }

  function writeSessionBytes(id: string, data: number[], session: SessionConfig) {
    switch (session.protocol) {
      case "ssh":
        return invoke("write_ssh_pty_bytes", { id, data });
      case "telnet":
        return invoke("write_telnet_bytes", { id, data });
      case "serial":
        return invoke("write_serial_bytes", { id, data });
      case "local":
        return invoke("write_pty_bytes", { id, data });
    }
  }

  function startZmodemSend(id: string, fileName: string, data: number[]) {
    return invoke<number[]>("zmodem_start_send", { id, fileName, data });
  }

  function cancelZmodem(id: string) {
    return invoke<number[]>("zmodem_cancel", { id });
  }

  async function startSerialSession(sessionId: string, serialConfig: SerialConfig) {
    await invoke("start_serial_session", {
      id: sessionId,
      portName: serialConfig.portName,
      baudRate: serialConfig.baudRate,
      dataBits: serialConfig.dataBits,
      stopBits: serialConfig.stopBits,
      parity: serialConfig.parity,
      flowControl: serialConfig.flowControl,
      // Omitted by pre-network workspaces, where the backend defaults to a
      // local port.
      transport: serialConfig.transport ?? "local",
      host: serialConfig.host ?? null,
      netPort: serialConfig.netPort ?? null,
      adoptServerParams: serialConfig.adoptServerParams ?? false,
      autoReconnect: serialConfig.autoReconnect ?? true,
    });
  }

  async function startLocalSession(sessionId: string, cols: number, rows: number, cwd?: string) {
    await invoke("start_pty", { id: sessionId, cols, rows, cwd });
  }

  async function persistUpdatedSshPassword(sshConfig: SshConfig) {
    try {
      const connections = await invoke<SavedConnection[]>("get_connections");

      // 1. Prefer matching by the explicit savedConnectionId.
      let target = sshConfig.savedConnectionId
        ? connections.find((connection) => connection.id === sshConfig.savedConnectionId)
        : undefined;

      // 2. Fallback: match an existing SSH bookmark by host/port/user so that
      //    quick-connect reconnects for a known server also update the stored
      //    password (only when exactly one candidate exists — otherwise we
      //    would silently mutate the wrong bookmark).
      if (!target) {
        const candidates = connections.filter(
          (connection) =>
            (connection.protocol ?? "ssh") === "ssh"
            && connection.host === sshConfig.host
            && connection.port === sshConfig.port
            && connection.user === sshConfig.user,
        );
        if (candidates.length === 1) {
          target = candidates[0];
          console.info(
            `[password-retry] matched bookmark "${target.name}" by host/port/user; updating its stored password`,
          );
        } else if (candidates.length > 1) {
          console.warn(
            `[password-retry] ${candidates.length} bookmarks match ${sshConfig.user}@${sshConfig.host}:${sshConfig.port}; skipping persistence to avoid ambiguous overwrite`,
          );
        }
      }

      if (!target) {
        console.info(
          `[password-retry] no saved bookmark found for ${sshConfig.user}@${sshConfig.host}:${sshConfig.port}; new password will only last for the current session`,
        );
        return;
      }

      await invoke("save_connection", {
        connection: {
          ...target,
          // Keep the bookmark's stored authType; if it was "none" we still
          // upgrade to "password" because the user just authenticated with one.
          authType: target.authType === "key" ? "key" : "password",
          password: sshConfig.password,
        },
      });
      console.info(`[password-retry] password for bookmark "${target.name}" updated`);
      onSshPasswordUpdated();
    } catch (error) {
      console.error("Failed to persist updated SSH password", error);
    }
  }

  function saveTerminalLog(content: string, tabName: string) {
    return invoke<string>("save_terminal_log", {
      content,
      tabName,
    });
  }

  function appendToLog(path: string, content: string) {
    return invoke("append_to_log", {
      path,
      content,
    });
  }

  function answerSshMfa(id: string, responses: string[]) {
    return invoke("answer_ssh_mfa", {
      id,
      responses,
    });
  }

  function answerSshReconnectChoice(id: string, sessionName: string | null) {
    return invoke("answer_ssh_reconnect_choice", {
      id,
      sessionName,
    });
  }

  function answerSshHostKeyMismatch(id: string, trustNewFingerprint: boolean) {
    return invoke("answer_ssh_host_key_mismatch", {
      id,
      trustNewFingerprint,
    });
  }

  return {
    writeSessionInput,
    writeSessionBytes,
    resizeSession,
    closeSession,
    getSshReconnectType,
    startSshSession,
    startTelnetSession,
    startSerialSession,
    startLocalSession,
    startZmodemSend,
    cancelZmodem,
    persistUpdatedSshPassword,
    saveTerminalLog,
    appendToLog,
    answerSshMfa,
    answerSshHostKeyMismatch,
    answerSshReconnectChoice,
  };
}
