import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CONNECTION_TEST_CODES,
  buildTestInvocation,
  runConnectionTest,
  summarizeTestReport,
  type ConnectionTestReport,
} from "../connectionTest";
import { setLanguage, t } from "../i18n";
import en from "../i18n/locales/en";
import zhCN from "../i18n/locales/zh-CN";
import type { ConnectResult, ConnectionProtocol } from "../types";
// The backend enum, read as text so the two lists cannot drift apart.
import connectionTestRs from "../../src-tauri/src/connection_test.rs?raw";

const tauri = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

const PROTOCOLS: ConnectionProtocol[] = ["ssh", "telnet", "serial", "rfc2217", "raw-tcp"];

/** Everything the dialog attaches to a connect result that a test must ignore. */
const SESSION_ONLY_FIELDS = {
  saveAs: "prod",
  saveGroup: "Production",
  logPath: "C:/logs/prod.log",
};

function sshResult(overrides: Partial<NonNullable<ConnectResult["sshConfig"]>> = {}): ConnectResult {
  return {
    protocol: "ssh",
    sshConfig: {
      host: "10.0.0.5",
      port: 22,
      user: "ops",
      authType: "password",
      agentForwarding: true,
      autoLoginRules: [{ expect: "$", response: "whoami" }],
      postConnectCommands: ["uptime"],
      autoReconnect: true,
      reconnectType: "tmux",
      ...overrides,
    },
    ...SESSION_ONLY_FIELDS,
  };
}

function serialResult(
  protocol: "serial" | "rfc2217" | "raw-tcp",
  overrides: Partial<NonNullable<ConnectResult["serialConfig"]>> = {},
): ConnectResult {
  return {
    protocol,
    serialConfig: {
      portName: protocol === "serial" ? "COM3" : "10.0.0.9:2217",
      baudRate: 115200,
      dataBits: 8,
      stopBits: 1,
      parity: "none",
      flowControl: "none",
      ...overrides,
    },
    ...SESSION_ONLY_FIELDS,
  };
}

function report(overrides: Partial<ConnectionTestReport> = {}): ConnectionTestReport {
  return {
    status: "success",
    code: "ok",
    detail: null,
    elapsedMs: 412,
    failedHop: null,
    hostKeys: [],
    authMethods: [],
    serverParams: null,
    signature: null,
    ...overrides,
  };
}

beforeEach(() => {
  setLanguage("en");
});

afterEach(() => {
  setLanguage("en");
});

describe("buildTestInvocation", () => {
  it("maps an SSH password connection with the same defaults as start_ssh_session", () => {
    const invocation = buildTestInvocation(sshResult());
    expect(invocation).toEqual({
      command: "ssh_test_connection",
      args: {
        host: "10.0.0.5",
        port: 22,
        user: "ops",
        password: null,
        privateKey: null,
        passphrase: null,
        authType: "password",
        jumpHosts: [],
      },
    });
  });

  it("passes the key, passphrase and jump hosts through untouched", () => {
    const jumpHosts = [
      { id: "j1", host: "bastion", port: 2222, user: "jump", authType: "agent" as const },
      { id: "j2", host: "inner", port: 22, user: "root", authType: "password" as const, password: "pw" },
    ];
    const invocation = buildTestInvocation(sshResult({
      authType: "key",
      privateKey: "-----BEGIN OPENSSH PRIVATE KEY-----\n...",
      passphrase: "secret",
      password: "ignored-by-backend",
      jumpHosts,
    }));
    expect(invocation?.command).toBe("ssh_test_connection");
    expect(invocation?.args).toMatchObject({
      authType: "key",
      privateKey: "-----BEGIN OPENSSH PRIVATE KEY-----\n...",
      passphrase: "secret",
      password: "ignored-by-backend",
      jumpHosts,
    });
  });

  it("keeps an SSH host exactly as typed, so the known-hosts scope matches a real connection", () => {
    const invocation = buildTestInvocation(sshResult({ host: " 10.0.0.5 " }));
    expect(invocation?.args.host).toBe(" 10.0.0.5 ");
  });

  it("sends a null authType when the connection did not name one", () => {
    const result = sshResult();
    delete result.sshConfig!.authType;
    expect(buildTestInvocation(result)?.args.authType).toBeNull();
  });

  it("maps telnet to host and port only", () => {
    const invocation = buildTestInvocation({
      protocol: "telnet",
      telnetConfig: { host: "switch-1", port: 2323 },
      ...SESSION_ONLY_FIELDS,
    });
    expect(invocation).toEqual({ command: "telnet_test_connection", args: { host: "switch-1", port: 2323 } });
  });

  it("defaults a local serial port to the local transport with no network endpoint", () => {
    const invocation = buildTestInvocation(serialResult("serial"));
    expect(invocation).toEqual({
      command: "serial_test_connection",
      args: {
        portName: "COM3",
        baudRate: 115200,
        dataBits: 8,
        stopBits: 1,
        parity: "none",
        flowControl: "none",
        transport: "local",
        host: null,
        netPort: null,
      },
    });
  });

  it.each(["rfc2217", "raw-tcp"] as const)(
    "sends transport, host and port for %s, but never adopt/reconnect flags",
    (protocol) => {
      const invocation = buildTestInvocation(serialResult(protocol, {
        transport: protocol,
        host: "10.0.0.9",
        netPort: 4001,
        adoptServerParams: false,
        autoReconnect: true,
        parity: "even",
        dataBits: 7,
        flowControl: "hardware",
      }));
      expect(invocation?.command).toBe("serial_test_connection");
      expect(invocation?.args).toMatchObject({
        transport: protocol,
        host: "10.0.0.9",
        netPort: 4001,
        parity: "even",
        dataBits: 7,
        flowControl: "hardware",
      });
      expect(invocation?.args).not.toHaveProperty("adoptServerParams");
      expect(invocation?.args).not.toHaveProperty("autoReconnect");
    },
  );

  it.each([
    ["ssh", sshResult()],
    ["telnet", { protocol: "telnet", telnetConfig: { host: "h", port: 23 }, ...SESSION_ONLY_FIELDS } as ConnectResult],
    ["serial", serialResult("serial")],
    ["rfc2217", serialResult("rfc2217", { transport: "rfc2217", host: "h", netPort: 2217 })],
    ["raw-tcp", serialResult("raw-tcp", { transport: "raw-tcp", host: "h", netPort: 4001 })],
  ])("never forwards session-only fields for %s", (_protocol, result) => {
    const args = buildTestInvocation(result)?.args ?? {};
    for (const field of ["saveAs", "saveGroup", "logPath", "cols", "rows", "id", "agentForwarding",
      "autoLoginRules", "postConnectCommands", "autoReconnect", "reconnectType", "adoptServerParams"]) {
      expect(args, field).not.toHaveProperty(field);
    }
  });

  it("returns null when the result carries no config for its protocol", () => {
    expect(buildTestInvocation({ protocol: "ssh" })).toBeNull();
    expect(buildTestInvocation({ protocol: "telnet" })).toBeNull();
    expect(buildTestInvocation({ protocol: "rfc2217" })).toBeNull();
  });
});

describe("runConnectionTest", () => {
  it("returns the backend report as-is", async () => {
    const backend = report({ code: "authFailed", status: "failure", authMethods: ["publickey"] });
    tauri.invoke.mockResolvedValueOnce(backend);
    await expect(runConnectionTest(sshResult())).resolves.toBe(backend);
    expect(tauri.invoke).toHaveBeenCalledOnce();
    expect(tauri.invoke).toHaveBeenCalledWith("ssh_test_connection", expect.objectContaining({ host: "10.0.0.5" }));
  });

  it("turns a rejected invoke into a failure report instead of throwing", async () => {
    tauri.invoke.mockRejectedValueOnce("Unsupported SSH auth type: bogus");
    const result = await runConnectionTest(sshResult());
    expect(result).toMatchObject({
      status: "failure",
      code: "error",
      detail: "Unsupported SSH auth type: bogus",
      hostKeys: [],
      authMethods: [],
    });
    expect(result.elapsedMs).toBeGreaterThanOrEqual(0);
  });

  it("stringifies an Error rejection", async () => {
    tauri.invoke.mockRejectedValueOnce(new Error("IPC down"));
    const result = await runConnectionTest(serialResult("serial"));
    expect(result.code).toBe("error");
    expect(result.detail).toContain("IPC down");
  });

  it("does not call the backend when there is nothing to test", async () => {
    const result = await runConnectionTest({ protocol: "telnet" });
    expect(result).toMatchObject({ status: "failure", code: "error" });
    expect(tauri.invoke).not.toHaveBeenCalled();
  });
});

describe("summarizeTestReport", () => {
  it("gives every protocol its own success sentence, with the elapsed time", () => {
    const titles = PROTOCOLS.map((protocol) => summarizeTestReport(protocol, report(), t).title);
    expect(new Set(titles).size).toBe(PROTOCOLS.length);
    for (const title of titles) {
      expect(title).toMatch(/\(412 ms\)$/);
    }
    expect(titles[0]).toBe("Connected and authenticated. No session was opened. (412 ms)");
    expect(titles[4]).toBe("The port is reachable. (412 ms)");
  });

  it("maps status to tone", () => {
    expect(summarizeTestReport("ssh", report(), t).tone).toBe("success");
    expect(summarizeTestReport("ssh", report({ status: "warning", code: "passwordMissing" }), t).tone).toBe("warning");
    expect(summarizeTestReport("ssh", report({ status: "failure", code: "refused" }), t).tone).toBe("error");
  });

  it("uses the code's sentence for anything but ok", () => {
    const summary = summarizeTestReport("telnet", report({ status: "failure", code: "refused", elapsedMs: 3 }), t);
    expect(summary.title).toBe("Connection refused: nothing is listening on that port. (3 ms)");
    expect(summary.lines).toEqual([]);
  });

  it("falls back to the generic failure sentence for a code this build does not know", () => {
    const summary = summarizeTestReport(
      "ssh",
      report({ status: "failure", code: "somethingNew" as ConnectionTestReport["code"] }),
      t,
    );
    expect(summary.title).toBe("The test failed. (412 ms)");
    expect(summary.tone).toBe("error");
  });

  it("treats an unknown status as an error tone rather than crashing", () => {
    const summary = summarizeTestReport(
      "ssh",
      report({ status: "exploded" as ConnectionTestReport["status"], code: "error" }),
      t,
    );
    expect(summary.tone).toBe("error");
  });

  it("flags a new host key as a warning that carries the fingerprint", () => {
    const summary = summarizeTestReport("ssh", report({
      hostKeys: [{ label: "10.0.0.5:22", status: "new", fingerprint: "SHA256:abc" }],
    }), t);
    expect(summary.lines).toEqual([{
      tone: "warning",
      text: "10.0.0.5:22 is a new host. Its key SHA256:abc is not trusted yet and will be recorded on the first real connection.",
    }]);
  });

  it("shows a trusted host key as a success line", () => {
    const summary = summarizeTestReport("ssh", report({
      hostKeys: [{ label: "bastion:22", status: "trusted", fingerprint: "SHA256:ok" }],
    }), t);
    expect(summary.lines).toEqual([{ tone: "success", text: "Host key for bastion:22 is trusted." }]);
  });

  it("shows both fingerprints for a changed host key, as an error", () => {
    const summary = summarizeTestReport("ssh", report({
      status: "failure",
      code: "hostKeyChanged",
      hostKeys: [{
        label: "10.0.0.5:22",
        status: "changed",
        fingerprint: "SHA256:new",
        expectedFingerprint: "SHA256:old",
      }],
    }), t);
    expect(summary.tone).toBe("error");
    expect(summary.title).toContain("The host key does not match the trusted one.");
    expect(summary.lines).toHaveLength(1);
    expect(summary.lines[0].tone).toBe("error");
    expect(summary.lines[0].text).toContain("SHA256:old");
    expect(summary.lines[0].text).toContain("SHA256:new");
  });

  it("orders the extra lines: hop, host keys, methods, server params, signature, detail", () => {
    const summary = summarizeTestReport("rfc2217", report({
      status: "failure",
      code: "authFailed",
      failedHop: { index: 2, label: "jump@inner:22" },
      hostKeys: [
        { label: "bastion:22", status: "trusted", fingerprint: "SHA256:a" },
        { label: "inner:22", status: "new", fingerprint: "SHA256:b" },
      ],
      authMethods: ["publickey", "keyboard-interactive"],
      serverParams: { baudRate: 9600 },
      signature: "ser2net 4.3",
      detail: "raw backend text",
    }), t);
    expect(summary.lines.map((line) => line.tone)).toEqual(["error", "success", "warning", "muted", "muted", "muted", "muted"]);
    expect(summary.lines.map((line) => line.text)).toEqual([
      "At jump host 2 (jump@inner:22).",
      "Host key for bastion:22 is trusted.",
      "inner:22 is a new host. Its key SHA256:b is not trusted yet and will be recorded on the first real connection.",
      "Server offers: publickey, keyboard-interactive",
      "Server port settings: 9600",
      "Server: ser2net 4.3",
      "raw backend text",
    ]);
  });

  it("lists the hops the chain never reached, right after the hop it stopped at", () => {
    const summary = summarizeTestReport("ssh", report({
      status: "failure",
      code: "authFailed",
      failedHop: { index: 1, label: "jump@a:22" },
      untestedHops: ["jump@b:22", "ops@10.0.0.5:22"],
      hostKeys: [{ label: "a:22", status: "trusted", fingerprint: "SHA256:a" }],
    }), t);
    expect(summary.lines.slice(0, 2)).toEqual([
      { tone: "error", text: "At jump host 1 (jump@a:22)." },
      { tone: "muted", text: "Not reached, so not tested: jump@b:22, ops@10.0.0.5:22." },
    ]);

    setLanguage("zh-CN");
    const chinese = summarizeTestReport("ssh", report({ untestedHops: ["ops@10.0.0.5:22"] }), t);
    expect(chinese.lines).toEqual([{ tone: "muted", text: "未能到达，因此未测试：ops@10.0.0.5:22。" }]);
  });

  it("adds no untested line for an empty list", () => {
    const summary = summarizeTestReport("ssh", report({ untestedHops: [] }), t);
    expect(summary.lines).toEqual([]);
  });

  it("keeps an ASCII space before the elapsed time in English only", () => {
    expect(t("connect.testElapsed", { ms: 5 })).toBe(" (5 ms)");
    setLanguage("zh-CN");
    expect(t("connect.testElapsed", { ms: 5 })).toBe("（耗时 5 ms）");
    for (const protocol of PROTOCOLS) {
      const title = summarizeTestReport(protocol, report(), t).title;
      expect(title, protocol).not.toMatch(/\s（/);
      expect(title, protocol).toMatch(/（耗时 412 ms）$/);
    }
  });

  it("colours the jump host line like the result it belongs to", () => {
    const summary = summarizeTestReport("ssh", report({
      status: "warning",
      code: "passwordMissing",
      failedHop: { index: 1, label: "jump@a:22" },
    }), t);
    expect(summary.tone).toBe("warning");
    expect(summary.lines[0]).toEqual({ tone: "warning", text: "At jump host 1 (jump@a:22)." });
  });

  it("formats confirmed server settings as baud plus framing, and adds flow control", () => {
    const summary = summarizeTestReport("rfc2217", report({
      serverParams: { baudRate: 9600, dataBits: 8, parity: "none", stopBits: 1, flowControl: "hardware" },
    }), t);
    expect(summary.lines).toEqual([{ tone: "muted", text: "Server port settings: 9600 8N1, Flow Control: Hardware" }]);
  });

  it("uses the parity initial in the framing", () => {
    const summary = summarizeTestReport("rfc2217", report({
      serverParams: { baudRate: 19200, dataBits: 7, parity: "even", stopBits: 2 },
    }), t);
    expect(summary.lines[0].text).toBe("Server port settings: 19200 7E2");
  });

  it("leaves out framing unless all three of its fields were confirmed", () => {
    const summary = summarizeTestReport("rfc2217", report({
      serverParams: { baudRate: 9600, dataBits: 8, parity: "none" },
    }), t);
    expect(summary.lines[0].text).toBe("Server port settings: 9600");
  });

  it("shows flow control alone when it is the only confirmed setting", () => {
    const summary = summarizeTestReport("rfc2217", report({ serverParams: { flowControl: "software" } }), t);
    expect(summary.lines[0].text).toBe("Server port settings: Flow Control: Software");
  });

  it("skips the server settings line when nothing was confirmed", () => {
    const summary = summarizeTestReport("rfc2217", report({ serverParams: {} }), t);
    expect(summary.lines).toEqual([]);
  });

  it("tolerates a report without the array fields", () => {
    const partial = { status: "failure", code: "timeout", elapsedMs: 10000 } as ConnectionTestReport;
    const summary = summarizeTestReport("raw-tcp", partial, t);
    expect(summary.title).toContain("Timed out.");
    expect(summary.lines).toEqual([]);
  });

  it("follows the UI language", () => {
    setLanguage("zh-CN");
    const summary = summarizeTestReport("ssh", report({
      status: "warning",
      code: "passwordMissing",
      hostKeys: [{ label: "10.0.0.5:22", status: "new", fingerprint: "SHA256:abc" }],
      failedHop: { index: 1, label: "jump@b:22" },
    }), t);
    // No ASCII space between the full-width stop and the full-width parenthesis.
    expect(summary.title).toBe("未填写密码：只检查了可达性和主机密钥。（耗时 412 ms）");
    expect(summary.lines[0].text).toBe("出错位置：跳板机 1（jump@b:22）。");
    expect(summary.lines[1].text).toContain("SHA256:abc");
    expect(summary.lines[1].text).toContain("新主机");
  });

  it("localises the flow control label in the server settings line", () => {
    setLanguage("zh-CN");
    const alone = summarizeTestReport("rfc2217", report({ serverParams: { flowControl: "none" } }), t);
    expect(alone.lines[0].text).toBe(`服务器当前串口参数：流控 ${zhCN.connect.none}`);
    const full = summarizeTestReport("rfc2217", report({
      serverParams: { baudRate: 115200, dataBits: 8, parity: "none", stopBits: 1, flowControl: "hardware" },
    }), t);
    // Full-width comma, and no second colon from the form label.
    expect(full.lines[0].text).toBe(`服务器当前串口参数：115200 8N1，流控 ${zhCN.connect.hardware}`);
  });

  it("ignores settings the server sent back as null", () => {
    const summary = summarizeTestReport("rfc2217", report({
      serverParams: { baudRate: 9600, dataBits: null, parity: null, stopBits: null, flowControl: null },
    }), t);
    expect(summary.lines[0].text).toBe("Server port settings: 9600");
  });
});

// ── i18n coverage ─────────────────────────────────────────────────────────────

const OK_KEYS = ["ssh", "telnet", "serial", "rfc2217", "rawTcp"] as const;
const FLAT_TEST_KEYS = [
  "test", "testing", "testHint", "testRunning", "testElapsed", "testFailedAtHop", "testHopsUntested",
  "testHostKeyTrusted", "testHostKeyNew", "testHostKeyChangedDetail", "testAuthMethods",
  "testServerParams", "testServerFlow", "testServerParamsSeparator", "testSignature",
] as const;

function lookupRaw(catalog: unknown, key: string): string | undefined {
  let node: unknown = catalog;
  for (const part of key.split(".")) {
    if (node == null || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
}

function placeholders(text: string): string[] {
  return [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
}

function allNewKeys(): string[] {
  return [
    ...FLAT_TEST_KEYS.map((key) => `connect.${key}`),
    ...OK_KEYS.map((key) => `connect.testOk.${key}`),
    ...CONNECTION_TEST_CODES.filter((code) => code !== "ok").map((code) => `connect.testResult.${code}`),
  ];
}

describe("connection test i18n", () => {
  it.each(allNewKeys())("%s exists in both catalogs, translated, with the same placeholders", (key) => {
    const english = lookupRaw(en, key);
    const chinese = lookupRaw(zhCN, key);
    expect(english, `en ${key}`).toBeTypeOf("string");
    expect(chinese, `zh-CN ${key}`).toBeTypeOf("string");
    expect(english!.trim()).not.toBe("");
    expect(chinese!.trim()).not.toBe("");
    // Not merely copied over from English.
    expect(chinese).not.toBe(english);
    expect(placeholders(chinese!)).toEqual(placeholders(english!));
  });

  it("has a sentence for every code the backend can send, and nothing else", () => {
    const codes = CONNECTION_TEST_CODES.filter((code) => code !== "ok");
    expect(Object.keys(en.connect.testResult).sort()).toEqual([...codes].sort());
    expect(Object.keys(zhCN.connect.testResult).sort()).toEqual([...codes].sort());
    expect(Object.keys(en.connect.testOk).sort()).toEqual([...OK_KEYS].sort());
    expect(Object.keys(zhCN.connect.testOk).sort()).toEqual([...OK_KEYS].sort());
  });

  it.each(["en", "zh-CN"] as const)("summaries never fall back to a raw key in %s", (language) => {
    setLanguage(language);
    for (const code of CONNECTION_TEST_CODES) {
      for (const protocol of PROTOCOLS) {
        const summary = summarizeTestReport(protocol, report({ code, status: code === "ok" ? "success" : "failure" }), t);
        const label = `${language} ${protocol} ${code}`;
        expect(summary.title, label).not.toMatch(/^connect\.test/);
        if (code !== "error") {
          // A known code must get its own sentence, not the unknown-code fallback.
          expect(summary.title, label).not.toBe(`${t("connect.testResult.error")}${t("connect.testElapsed", { ms: 412 })}`);
        }
      }
    }
  });

  it("keeps the existing string key `connect.test` alongside the nested objects", () => {
    expect(t("connect.test")).toBe("Test");
    expect(t("connect.testing")).toBe("Testing…");
  });
});

// ── Backend parity ────────────────────────────────────────────────────────────

describe("CONNECTION_TEST_CODES", () => {
  it("matches the TestCode enum in connection_test.rs (camelCase), plus the frontend-only error", () => {
    const body = connectionTestRs.match(/pub enum TestCode\s*\{([\s\S]*?)\n\}/);
    expect(body, "TestCode enum not found").not.toBeNull();
    const variants = body![1]
      .split("\n")
      .map((line) => line.replace(/\/\/.*$/, "").trim())
      .filter(Boolean)
      .flatMap((line) => line.split(","))
      .map((part) => part.trim())
      .filter(Boolean)
      .map((variant) => variant.charAt(0).toLowerCase() + variant.slice(1));
    expect(variants.length).toBeGreaterThan(20);
    expect([...variants, "error"].sort()).toEqual([...CONNECTION_TEST_CODES].sort());
  });
});
