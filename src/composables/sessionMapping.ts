import {
  isReconnectEnabled,
  normalizeReconnectType,
  type ConnectResult,
  type ConnectionProtocol,
  type SavedConnection,
  type SessionConfig,
} from "../types";
import { isNetworkSerialTransport, isSerialProtocol, serialTargetLabel, transportForProtocol } from "../serialTransport";

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
        savedConnectionGroup: result.saveGroup,
      },
    };
  }

  if (protocol === "telnet" && telnetConfig) {
    return {
      protocol: "telnet",
      telnetConfig,
    };
  }

  if (isSerialProtocol(protocol) && serialConfig) {
    // Keep the derived transport in step with the protocol the user picked.
    return {
      protocol,
      serialConfig: { ...serialConfig, transport: transportForProtocol(protocol) },
    } as SessionConfig;
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
        // The network serial protocols reuse the existing host/port columns
        // rather than growing a parallel pair.
        : result.serialConfig?.host || "",
    port: protocol === "ssh"
      ? result.sshConfig?.port || 22
      : protocol === "telnet"
        ? result.telnetConfig?.port || 23
        : result.serialConfig?.netPort || 0,
    user: protocol === "ssh" ? result.sshConfig?.user || "" : "",
    authType: protocol === "ssh"
      ? (result.sshConfig?.authType ?? (result.sshConfig?.privateKey ? "key" : "password"))
      : "none",
    password: protocol === "ssh" ? result.sshConfig?.password : undefined,
    privateKey: protocol === "ssh" ? result.sshConfig?.privateKey : undefined,
    passphrase: protocol === "ssh" ? result.sshConfig?.passphrase : undefined,
    agentForwarding: protocol === "ssh" ? result.sshConfig?.agentForwarding : undefined,
    jumpHosts: protocol === "ssh" ? result.sshConfig?.jumpHosts : undefined,
    autoLoginRules: protocol === "ssh" ? result.sshConfig?.autoLoginRules : undefined,
    postConnectCommands: protocol === "ssh" ? result.sshConfig?.postConnectCommands : undefined,
    portName: isSerialProtocol(protocol) ? result.serialConfig?.portName : undefined,
    adoptServerParams: protocol === "rfc2217" ? result.serialConfig?.adoptServerParams : undefined,
    serialAutoReconnect: protocol === "rfc2217" || protocol === "raw-tcp"
      ? result.serialConfig?.autoReconnect
      : undefined,
    baudRate: isSerialProtocol(protocol) ? result.serialConfig?.baudRate : undefined,
    dataBits: isSerialProtocol(protocol) ? result.serialConfig?.dataBits : undefined,
    stopBits: isSerialProtocol(protocol) ? result.serialConfig?.stopBits : undefined,
    parity: isSerialProtocol(protocol) ? result.serialConfig?.parity : undefined,
    flowControl: isSerialProtocol(protocol) ? result.serialConfig?.flowControl : undefined,
    createdAt,
    autoReconnect: protocol === "ssh" && reconnectType ? isReconnectEnabled(reconnectType) : undefined,
    reconnectType,
  };
}

export function buildSessionFromSavedConnection(connection: SavedConnection): SessionConfig {
  const protocol = resolveConnectionProtocol(connection);

  if (isSerialProtocol(protocol) && connection.baudRate) {
    const transport = transportForProtocol(protocol);
    const network = isNetworkSerialTransport(transport);
    return {
      protocol,
      serialConfig: {
        transport,
        // Rebuilt rather than trusted: a bookmark edited to a new host must not
        // keep connecting to the label it was saved with.
        portName: network
          ? serialTargetLabel(transport, connection.portName ?? "", connection.host, connection.port)
          : connection.portName ?? "",
        host: network ? connection.host : undefined,
        netPort: network ? connection.port : undefined,
        adoptServerParams: protocol === "rfc2217" ? connection.adoptServerParams : undefined,
        autoReconnect: network ? connection.serialAutoReconnect : undefined,
        baudRate: connection.baudRate,
        dataBits: connection.dataBits ?? 8,
        stopBits: connection.stopBits ?? 1,
        parity: connection.parity ?? "none",
        flowControl: connection.flowControl ?? "none",
      },
    } as SessionConfig;
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
      passphrase: connection.authType === "key" ? connection.passphrase : undefined,
      authType: connection.authType,
      agentForwarding: connection.agentForwarding,
      jumpHosts: connection.jumpHosts,
      autoLoginRules: connection.autoLoginRules,
      postConnectCommands: connection.postConnectCommands,
      savedConnectionId: connection.id,
      savedConnectionGroup: connection.group,
      autoReconnect: isReconnectEnabled(reconnectType),
      reconnectType,
    },
  };
}
