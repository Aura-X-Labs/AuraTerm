import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import type { ConnectionProtocol, SavedConnection } from "./types";

export interface LogTemplateContext {
  protocol: ConnectionProtocol;
  host?: string;
  user?: string;
  port?: number | string;
  serialPort?: string;
  baudRate?: number | string;
  session?: string;
}

function sanitizeFileSegment(value: string) {
  return value.replace(/[^a-zA-Z0-9\-_@.]/g, "_") || "session";
}

function sanitizeFileNameTemplateResult(value: string) {
  return value.replace(/[^a-zA-Z0-9\-_@.{}]/g, "_") || "session";
}

function buildDefaultSessionName(context: LogTemplateContext) {
  if (context.session?.trim()) {
    return context.session.trim();
  }

  if (context.protocol === "ssh") {
    if (context.user?.trim() && context.host?.trim()) {
      return `${context.user.trim()}@${context.host.trim()}`;
    }
    return context.host?.trim() || "session";
  }

  if (context.protocol === "telnet") {
    const host = context.host?.trim() || "localhost";
    const port = String(context.port ?? 23).trim() || "23";
    return `telnet://${host}:${port}`;
  }

  const serialPort = context.serialPort?.trim() || "serial";
  const baudRate = String(context.baudRate ?? 9600).trim() || "9600";
  return `serial://${serialPort}@${baudRate}`;
}

export function renderLogFileNameTemplate(settings: AppSettings | undefined, context: LogTemplateContext) {
  const rawTemplate = (settings?.logFileNameTemplate ?? DEFAULT_SETTINGS.logFileNameTemplate).trim();
  const template = rawTemplate || DEFAULT_SETTINGS.logFileNameTemplate;
  const hostText = context.host?.trim() || "localhost";
  const userText = context.user?.trim() || "user";
  const portText = String(context.port ?? (context.protocol === "ssh" ? 22 : 23)).trim();
  const serialPortText = context.serialPort?.trim() || "serial";
  const baudRateText = String(context.baudRate ?? 9600).trim();

  const tokens: Record<string, string> = {
    session: sanitizeFileSegment(buildDefaultSessionName(context)),
    protocol: sanitizeFileSegment(context.protocol),
    host: sanitizeFileSegment(hostText),
    user: sanitizeFileSegment(userText),
    port: sanitizeFileSegment(portText),
    serialPort: sanitizeFileSegment(serialPortText),
    baudRate: sanitizeFileSegment(baudRateText),
  };

  const rendered = template.replace(/\{(session|protocol|host|user|port|serialPort|baudRate)\}/g, (_match, token: string) => {
    return tokens[token] ?? "";
  });

  return sanitizeFileNameTemplateResult(rendered);
}

export function joinLogPath(directory: string, fileName: string) {
  const normalizedDirectory = directory.trim().replace(/[\\/]+$/, "") || DEFAULT_SETTINGS.logSavePath;
  const separator = normalizedDirectory.includes("\\") ? "\\" : "/";
  return `${normalizedDirectory}${separator}${fileName}.log`;
}

export function buildDefaultLogPath(settings: AppSettings | undefined, context: LogTemplateContext) {
  const logDir = (settings?.logSavePath ?? DEFAULT_SETTINGS.logSavePath).trim() || DEFAULT_SETTINGS.logSavePath;
  return joinLogPath(logDir, renderLogFileNameTemplate(settings, context));
}

export function buildConnectionLogContext(connection: SavedConnection): LogTemplateContext {
  const protocol = connection.protocol ?? "ssh";
  if (protocol === "serial") {
    return {
      protocol,
      serialPort: connection.portName,
      baudRate: connection.baudRate,
      session: connection.name,
    };
  }

  return {
    protocol,
    host: connection.host,
    user: protocol === "ssh" ? connection.user : undefined,
    port: connection.port,
    session: connection.name,
  };
}

export function normalizeOptionalLogPath(logPath: string | undefined, fallbackPath: string) {
  if (logPath === undefined) {
    return undefined;
  }

  return logPath.trim() || fallbackPath;
}