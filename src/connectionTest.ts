import { invoke } from "@tauri-apps/api/core";
import type { ConnectResult, ConnectionProtocol, SerialParams } from "./types";

/** Mirrors `TestStatus` in `connection_test.rs`. */
export type ConnectionTestStatus = "success" | "warning" | "failure";

/** Every code `ConnectionTestReport.code` can carry, plus the frontend-only
 *  `"error"` for an `invoke` that was rejected outright. Kept as a value so the
 *  tests can check each one has a translation in every locale. */
export const CONNECTION_TEST_CODES = [
  "ok",
  "resolveFailed",
  "refused",
  "timeout",
  "unreachable",
  "connectFailed",
  "peerClosed",
  "handshakeFailed",
  "handshakeTimeout",
  "hostKeyChanged",
  "authFailed",
  "authPartial",
  "authInteractiveRequired",
  "authInteractiveUntested",
  "authMethodUnsupported",
  "authTimeout",
  "passwordMissing",
  "keyInvalid",
  "agentUnavailable",
  "serialPortBusy",
  "serialPortNotFound",
  "serialPermissionDenied",
  "serialParamsRejected",
  "serialOpenFailed",
  "serialOpenTimeout",
  "rfc2217Refused",
  "error",
] as const;
export type ConnectionTestCode = typeof CONNECTION_TEST_CODES[number];

export type ServerSerialParams = { [K in keyof SerialParams]?: SerialParams[K] | null };

export interface ConnectionTestHostKey {
  /** Known-hosts scope, `host:port`. */
  label: string;
  status: "trusted" | "new" | "changed";
  fingerprint: string;
  /** The trusted fingerprint, only when it differs. */
  expectedFingerprint?: string | null;
}

/** Mirrors `ConnectionTestReport` in `connection_test.rs`. */
export interface ConnectionTestReport {
  status: ConnectionTestStatus;
  code: ConnectionTestCode;
  /** Raw backend message, in English. */
  detail?: string | null;
  elapsedMs: number;
  failedHop?: { index: number; label: string } | null;
  /** SSH: hops after the one the chain stopped at, never reached, as
   *  `user@host:port` in chain order (the target last). */
  untestedHops?: string[];
  hostKeys: ConnectionTestHostKey[];
  authMethods: string[];
  /** RFC 2217: only the settings the server explicitly confirmed; the rest
   *  arrive as `null`. */
  serverParams?: ServerSerialParams | null;
  signature?: string | null;
}

export type ConnectionTestCommand = "ssh_test_connection" | "telnet_test_connection" | "serial_test_connection";

/** The command and arguments that test `result`, shaped exactly like the
 *  matching `start*` call in `useTerminalSessionCommands` so a test and a real
 *  connection see the same values. `null` when `result` carries no config for
 *  its protocol. */
export function buildTestInvocation(
  result: ConnectResult,
): { command: ConnectionTestCommand; args: Record<string, unknown> } | null {
  switch (result.protocol) {
    case "ssh": {
      const ssh = result.sshConfig;
      if (!ssh) return null;
      return {
        command: "ssh_test_connection",
        args: {
          host: ssh.host,
          port: ssh.port,
          user: ssh.user,
          password: ssh.password ?? null,
          privateKey: ssh.privateKey ?? null,
          passphrase: ssh.passphrase ?? null,
          authType: ssh.authType ?? null,
          jumpHosts: ssh.jumpHosts ?? [],
        },
      };
    }
    case "telnet": {
      const telnet = result.telnetConfig;
      if (!telnet) return null;
      return { command: "telnet_test_connection", args: { host: telnet.host, port: telnet.port } };
    }
    case "serial":
    case "rfc2217":
    case "raw-tcp": {
      const serial = result.serialConfig;
      if (!serial) return null;
      return {
        command: "serial_test_connection",
        // No adoptServerParams / autoReconnect: the test always negotiates in
        // adopt mode and never reconnects.
        args: {
          portName: serial.portName,
          baudRate: serial.baudRate,
          dataBits: serial.dataBits,
          stopBits: serial.stopBits,
          parity: serial.parity,
          flowControl: serial.flowControl,
          transport: serial.transport ?? "local",
          host: serial.host ?? null,
          netPort: serial.netPort ?? null,
        },
      };
    }
  }
}

function errorReport(detail: string, elapsedMs: number): ConnectionTestReport {
  return {
    status: "failure",
    code: "error",
    detail,
    elapsedMs,
    failedHop: null,
    untestedHops: [],
    hostKeys: [],
    authMethods: [],
    serverParams: null,
    signature: null,
  };
}

/** Run the backend test for `result`. Never rejects: an `invoke` error comes
 *  back as a `code: "error"` failure report, so a caller that has gone away
 *  (the dialog closed mid-test) cannot leave an unhandled rejection behind. */
export async function runConnectionTest(result: ConnectResult): Promise<ConnectionTestReport> {
  const started = Date.now();
  const invocation = buildTestInvocation(result);
  if (!invocation) {
    return errorReport(`No ${result.protocol} settings to test`, 0);
  }
  try {
    return await invoke<ConnectionTestReport>(invocation.command, invocation.args);
  } catch (error) {
    return errorReport(String(error), Date.now() - started);
  }
}

export type TestSummaryTone = "success" | "warning" | "error";

export interface TestSummaryLine {
  tone: TestSummaryTone | "muted";
  text: string;
}

export interface TestSummary {
  tone: TestSummaryTone;
  title: string;
  lines: TestSummaryLine[];
}

type Translate = (key: string, params?: Record<string, string | number>) => string;

const OK_TITLE_KEYS: Record<ConnectionProtocol, string> = {
  ssh: "connect.testOk.ssh",
  telnet: "connect.testOk.telnet",
  serial: "connect.testOk.serial",
  rfc2217: "connect.testOk.rfc2217",
  "raw-tcp": "connect.testOk.rawTcp",
};

const STATUS_TONES: Record<ConnectionTestStatus, TestSummaryTone> = {
  success: "success",
  warning: "warning",
  failure: "error",
};

/** `115200 8N1, Flow Control: None` — each part only when the server confirmed
 *  it; framing only when all three of its fields were confirmed. */
function formatServerParams(params: ServerSerialParams, t: Translate): string {
  const parts: string[] = [];
  if (params.baudRate != null) {
    parts.push(String(params.baudRate));
  }
  if (params.dataBits != null && params.parity != null && params.stopBits != null) {
    parts.push(`${params.dataBits}${params.parity.charAt(0).toUpperCase()}${params.stopBits}`);
  }
  const text = parts.join(" ");
  if (params.flowControl == null) {
    return text;
  }
  // Not the form label (`connect.flowControl`): that one ends in a colon.
  const flow = t("connect.testServerFlow", { mode: t(`connect.${params.flowControl}`) });
  return text ? `${text}${t("connect.testServerParamsSeparator")}${flow}` : flow;
}

/** Turn a report into the dialog's localized result block. Pure; `t` is passed
 *  in so tests can drive it with either locale. */
export function summarizeTestReport(
  protocol: ConnectionProtocol,
  report: ConnectionTestReport,
  t: Translate,
): TestSummary {
  const titleKey = report.code === "ok" ? OK_TITLE_KEYS[protocol] : `connect.testResult.${report.code}`;
  let title = t(titleKey);
  // A code this build does not know yet (a newer backend) still reads as a sentence.
  if (title === titleKey) {
    title = t("connect.testResult.error");
  }

  const tone = STATUS_TONES[report.status] ?? "error";
  const lines: TestSummaryLine[] = [];
  if (report.failedHop) {
    // Same tone as the title: a jump host can stop the chain with a warning
    // (no password typed, a second factor) as well as with a failure.
    lines.push({
      tone,
      text: t("connect.testFailedAtHop", { index: report.failedHop.index, label: report.failedHop.label }),
    });
  }
  if (report.untestedHops?.length) {
    lines.push({
      tone: "muted",
      text: t("connect.testHopsUntested", { hops: report.untestedHops.join(", ") }),
    });
  }
  for (const hostKey of report.hostKeys ?? []) {
    if (hostKey.status === "trusted") {
      lines.push({ tone: "success", text: t("connect.testHostKeyTrusted", { label: hostKey.label }) });
    } else if (hostKey.status === "new") {
      lines.push({
        tone: "warning",
        text: t("connect.testHostKeyNew", { label: hostKey.label, fingerprint: hostKey.fingerprint }),
      });
    } else {
      lines.push({
        tone: "error",
        text: t("connect.testHostKeyChangedDetail", {
          label: hostKey.label,
          expected: hostKey.expectedFingerprint ?? "",
          fingerprint: hostKey.fingerprint,
        }),
      });
    }
  }
  if (report.authMethods?.length) {
    lines.push({ tone: "muted", text: t("connect.testAuthMethods", { methods: report.authMethods.join(", ") }) });
  }
  if (report.serverParams) {
    const params = formatServerParams(report.serverParams, t);
    if (params) {
      lines.push({ tone: "muted", text: t("connect.testServerParams", { params }) });
    }
  }
  if (report.signature) {
    lines.push({ tone: "muted", text: t("connect.testSignature", { signature: report.signature }) });
  }
  if (report.detail) {
    lines.push({ tone: "muted", text: report.detail });
  }

  return {
    tone,
    // No separator here: each locale's `testElapsed` carries its own spacing,
    // since a full-width parenthesis wants none after a full-width stop.
    title: `${title}${t("connect.testElapsed", { ms: report.elapsedMs })}`,
    lines,
  };
}
