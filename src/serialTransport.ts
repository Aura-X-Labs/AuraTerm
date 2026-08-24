import type { SerialConfig, SerialParams, SerialStatus, SerialTransport } from "./types";

/** The port IANA assigns to RFC 2217, and what ser2net examples use. */
export const RFC2217_DEFAULT_PORT = 2217;

/** Offered in the status bar's baud picker. Whatever the session is actually
 *  running at is always added to this, so an unusual rate stays selectable. */
export const COMMON_BAUD_RATES = [
  1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600,
];

export function serialTransportOf(config: Pick<SerialConfig, "transport">): SerialTransport {
  return config.transport ?? "local";
}

export function isNetworkSerialTransport(transport: SerialTransport | undefined): boolean {
  return transport === "rfc2217" || transport === "raw-tcp";
}

/** The label a network serial session carries in `portName`.
 *
 *  Keeping the endpoint in `portName` is deliberate: tab titles, bookmark
 *  subtitles, the AI context line and the `{serialPort}` log placeholder all
 *  read that one field, and none of them had to learn about transports. */
export function serialTargetLabel(
  transport: SerialTransport,
  portName: string,
  host?: string,
  netPort?: number,
): string {
  if (!isNetworkSerialTransport(transport)) {
    return portName;
  }
  const trimmedHost = host?.trim();
  if (!trimmedHost) {
    return portName;
  }
  return `${trimmedHost}:${netPort ?? RFC2217_DEFAULT_PORT}`;
}

interface ParsedSerialUrl {
  transport: SerialTransport;
  host: string;
  port: number;
}

/** Parse an endpoint the user pasted into the host field.
 *
 *  `rfc2217://host:port` is pyserial's URL form, which is what people copy out
 *  of a wiki or a colleague's message. `telnet://` and `tcp://` are accepted
 *  too, and a bare `host:port` is treated as whichever transport is already
 *  selected by the caller. */
export function parseSerialEndpoint(raw: string): ParsedSerialUrl | null {
  const value = raw.trim();
  if (!value) {
    return null;
  }

  const withScheme = /^([a-z0-9+.-]+):\/\/(.+)$/i.exec(value);
  if (!withScheme) {
    return null;
  }

  const [, scheme, rest] = withScheme;
  const transport: SerialTransport | null = (() => {
    switch (scheme.toLowerCase()) {
      case "rfc2217":
        return "rfc2217";
      case "telnet":
        return "rfc2217";
      case "tcp":
      case "socket":
        return "raw-tcp";
      default:
        return null;
    }
  })();

  if (!transport) {
    return null;
  }

  const authority = rest.split("/")[0];
  const separator = authority.lastIndexOf(":");
  if (separator <= 0) {
    return { transport, host: authority, port: RFC2217_DEFAULT_PORT };
  }

  const host = authority.slice(0, separator);
  const port = parseInt(authority.slice(separator + 1), 10);
  return {
    transport,
    host,
    port: Number.isFinite(port) && port > 0 ? port : RFC2217_DEFAULT_PORT,
  };
}

/** `115200 8N1` — the shorthand every serial tool prints. */
export function formatSerialParams(params: SerialParams): string {
  const parity = params.parity === "none" ? "N" : params.parity === "even" ? "E" : "O";
  return `${params.baudRate} ${params.dataBits}${parity}${params.stopBits}`;
}

/** Terminal banner lines for a serial status update.
 *
 *  These go into the terminal rather than a toast because they explain what the
 *  session in front of you is actually doing — a parameter block the server
 *  clamped, or a handshake it never answered — and that belongs in the
 *  scrollback next to the output it explains. Returns an empty list when there
 *  is nothing worth saying. */
export function describeSerialStatus(status: SerialStatus): string[] {
  const lines: string[] = [];
  const warn = (text: string) => `\x1b[33m${text}\x1b[0m`;

  // A handshake still in flight — which every reconnect passes through — has
  // nothing to report yet. Saying "the server refused" here would be a lie that
  // corrects itself a moment later, on every single reconnect.
  if (status.transport === "rfc2217" && status.negotiationSettled) {
    if (!status.rfc2217Negotiated) {
      lines.push(warn(
        "[RFC 2217] The server did not accept COM-PORT-OPTION. Line parameters were "
        + "not applied; the session continues as a plain byte pipe. Set the port up on "
        + "the device server, or switch this session to raw TCP.",
      ));
    } else {
      const effective = formatSerialParams(status.effective);
      const requested = formatSerialParams(status.requested);
      if (effective !== requested) {
        // The server is free to clamp. Showing the requested value as if it had
        // taken effect is how an afternoon disappears.
        lines.push(warn(`[RFC 2217] Server applied ${effective}, not the requested ${requested}.`));
      } else {
        lines.push(`\x1b[2m[RFC 2217] Port set to ${effective}.\x1b[0m`);
      }

      if (!status.binaryNegotiated) {
        lines.push(warn(
          "[RFC 2217] The server refused BINARY mode. 8-bit payloads (ZMODEM transfers, "
          + "firmware uploads) may be corrupted on this link.",
        ));
      }
    }
  }

  const { framing, parity, overrun, breakDetected } = status.lineErrors;
  if (framing || parity) {
    lines.push(warn(
      `[Serial] ${framing ? "Framing" : "Parity"} errors on the line — the baud rate is `
      + "probably wrong.",
    ));
  } else if (overrun) {
    lines.push(warn("[Serial] Receive overrun — bytes were dropped."));
  }
  if (breakDetected) {
    lines.push(warn("[Serial] BREAK detected on the line."));
  }

  return lines;
}

/** Whether an endpoint stays on this machine.
 *
 *  RFC 2217 carries no authentication and no encryption at all, so reaching one
 *  across a network means anything on the path can read the console and inject
 *  keystrokes. Over loopback — the far side of an SSH local forward — that is
 *  no longer true, which is why the distinction is worth drawing in the UI. */
export function isLoopbackHost(host: string): boolean {
  const value = host.trim().toLowerCase().replace(/^\[|\]$/g, "");
  return value === "localhost"
    || value === "::1"
    || value === "0:0:0:0:0:0:0:1"
    || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(value);
}
