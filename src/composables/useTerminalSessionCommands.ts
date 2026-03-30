import { invoke } from "@tauri-apps/api/core";
import { isReconnectEnabled, normalizeReconnectType } from "../types";
import type {
  ReconnectType,
  SavedConnection,
  SerialConfig,
  SessionConfig,
  SshConfig,
} from "../types";

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
    }
  }

  function resizeSession(id: string, cols: number, rows: number, session: SessionConfig) {
    switch (session.protocol) {
      case "ssh":
        return invoke("resize_ssh_pty", { id, cols, rows });
      case "local":
        return invoke("resize_pty", { id, cols, rows });
      case "telnet":
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
      cols,
      rows,
      autoReconnect: isReconnectEnabled(reconnectType),
      reconnectType,
    });
  }

  async function startTelnetSession(sessionId: string, host: string, port: number) {
    await invoke("start_telnet_session", {
      id: sessionId,
      host,
      port,
    });
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
    });
  }

  async function startLocalSession(sessionId: string, cols: number, rows: number, cwd?: string) {
    await invoke("start_pty", { id: sessionId, cols, rows, cwd });
  }

  async function persistUpdatedSshPassword(sshConfig: SshConfig) {
    if (!sshConfig.savedConnectionId) {
      return;
    }

    try {
      const connections = await invoke<SavedConnection[]>("get_connections");
      const existing = connections.find((connection) => connection.id === sshConfig.savedConnectionId);
      if (!existing) {
        return;
      }

      await invoke("save_connection", {
        connection: {
          ...existing,
          password: sshConfig.password,
        },
      });
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

  return {
    writeSessionInput,
    resizeSession,
    closeSession,
    getSshReconnectType,
    startSshSession,
    startTelnetSession,
    startSerialSession,
    startLocalSession,
    persistUpdatedSshPassword,
    saveTerminalLog,
    appendToLog,
    answerSshMfa,
    answerSshReconnectChoice,
  };
}