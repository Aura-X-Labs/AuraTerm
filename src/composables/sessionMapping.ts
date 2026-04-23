import {
  isReconnectEnabled,
  normalizeReconnectType,
  type ConnectResult,
  type ConnectionProtocol,
  type SavedConnection,
  type SessionConfig,
} from "../types";

function resolveConnectionProtocol(connection: SavedConnection): ConnectionProtocol {
  return connection.protocol ?? "ssh";
}

export function buildSessionFromConnectResult(result: ConnectResult, savedConnectionId?: string): SessionConfig | null {
  const { protocol, sshConfig, telnetConfig, serialConfig } = result;

  if (protocol === "ssh" && sshConfig) {
    return {
      protocol: "ssh",
      sshConfig: {
        ...sshConfig,
        savedConnectionId,
      },
    };
  }

  if (protocol === "telnet" && telnetConfig) {
    return {
      protocol: "telnet",
      telnetConfig,
    };
  }

  if (protocol === "serial" && serialConfig) {
    return {
      protocol: "serial",
      serialConfig,
    };
  }

  return null;
}

export function buildSavedConnectionFromConnectResult(
  result: ConnectResult,
  id: string,
  createdAt: number,
): SavedConnection {
  const protocol = result.protocol;
  const reconnectType = protocol === "ssh" ? normalizeReconnectType(result.sshConfig) : undefined;

  return {
    id,
    name: result.saveAs || "Connection",
    group: result.saveGroup,
    logPath: result.logPath,
    protocol,
    host: protocol === "ssh"
      ? result.sshConfig?.host || ""
      : protocol === "telnet"
        ? result.telnetConfig?.host || ""
        : "",
    port: protocol === "ssh"
      ? result.sshConfig?.port || 22
      : protocol === "telnet"
        ? result.telnetConfig?.port || 23
        : 0,
    user: protocol === "ssh" ? result.sshConfig?.user || "" : "",
    authType: protocol === "ssh"
      ? (result.sshConfig?.privateKey ? "key" : "password")
      : "none",
    password: protocol === "ssh" ? result.sshConfig?.password : undefined,
    privateKey: protocol === "ssh" ? result.sshConfig?.privateKey : undefined,
    portName: protocol === "serial" ? result.serialConfig?.portName : undefined,
    baudRate: protocol === "serial" ? result.serialConfig?.baudRate : undefined,
    dataBits: protocol === "serial" ? result.serialConfig?.dataBits : undefined,
    stopBits: protocol === "serial" ? result.serialConfig?.stopBits : undefined,
    parity: protocol === "serial" ? result.serialConfig?.parity : undefined,
    flowControl: protocol === "serial" ? result.serialConfig?.flowControl : undefined,
    createdAt,
    autoReconnect: protocol === "ssh" && reconnectType ? isReconnectEnabled(reconnectType) : undefined,
    reconnectType,
  };
}

export function buildSessionFromSavedConnection(connection: SavedConnection): SessionConfig {
  const protocol = resolveConnectionProtocol(connection);

  if (protocol === "serial" && connection.portName && connection.baudRate) {
    return {
      protocol: "serial",
      serialConfig: {
        portName: connection.portName,
        baudRate: connection.baudRate,
        dataBits: connection.dataBits ?? 8,
        stopBits: connection.stopBits ?? 1,
        parity: connection.parity ?? "none",
        flowControl: connection.flowControl ?? "none",
      },
    };
  }

  if (protocol === "telnet") {
    return {
      protocol: "telnet",
      telnetConfig: {
        host: connection.host,
        port: connection.port,
      },
    };
  }

  const reconnectType = normalizeReconnectType(connection);
  return {
    protocol: "ssh",
    sshConfig: {
      host: connection.host,
      port: connection.port,
      user: connection.user,
      password: connection.password,
      privateKey: connection.authType === "key" ? connection.privateKey : undefined,
      savedConnectionId: connection.id,
      autoReconnect: isReconnectEnabled(reconnectType),
      reconnectType,
    },
  };
}
