import { describe, expect, it } from "vitest";

import {
  buildConnectionLogContext,
  buildDefaultLogPath,
  joinLogPath,
  normalizeOptionalLogPath,
  renderLogFileNameTemplate,
  type LogTemplateContext,
} from "../logging";
import { DEFAULT_SETTINGS, type AppSettings } from "../settings";
import type { SavedConnection } from "../types";

/**
 * Unit tests for the log filename template renderer.
 *
 * The template is user-configurable and the resulting filename is fed straight
 * into the OS — so we must verify:
 *   - all documented tokens are substituted
 *   - path separators / reserved characters are sanitized
 *   - defaults kick in when the template is empty / whitespace
 *   - protocol-specific defaults (telnet port, serial baud) render correctly
 *   - joinLogPath keeps the user's path separator flavor (\\ vs /)
 */

function sshContext(overrides: Partial<LogTemplateContext> = {}): LogTemplateContext {
  return {
    protocol: "ssh",
    host: "example.com",
    user: "alice",
    port: 22,
    session: "prod",
    ...overrides,
  };
}

function settingsWithTemplate(template: string): AppSettings {
  return { ...DEFAULT_SETTINGS, logFileNameTemplate: template };
}

describe("renderLogFileNameTemplate", () => {
  it("substitutes all common SSH tokens", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}_{protocol}_{host}_{user}_{port}"),
      sshContext(),
    );
    expect(rendered).toBe("prod_ssh_example.com_alice_22");
  });

  it("falls back to default template when the provided template is empty", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("   "),
      sshContext(),
    );
    // Default is "{session}_{timestamp}" — {timestamp} is not a recognised token
    // so it is stripped to an empty segment; only {session} survives.
    expect(rendered.startsWith("prod_")).toBe(true);
  });

  it("uses DEFAULT_SETTINGS template when settings is undefined", () => {
    const rendered = renderLogFileNameTemplate(undefined, sshContext());
    expect(rendered.startsWith("prod_")).toBe(true);
  });

  it("sanitizes path separators and other reserved characters", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}"),
      sshContext({ session: "bad/path:name*?" }),
    );
    expect(rendered).not.toContain("/");
    expect(rendered).not.toContain(":");
    expect(rendered).not.toContain("*");
    expect(rendered).not.toContain("?");
    // Final string must not be empty even after sanitizing everything away.
    expect(rendered.length).toBeGreaterThan(0);
  });

  it("builds default SSH session name from user@host when session is empty", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}"),
      sshContext({ session: undefined }),
    );
    expect(rendered).toBe("alice@example.com");
  });

  it("builds default SSH session name from host alone when user is empty", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}"),
      sshContext({ session: undefined, user: undefined }),
    );
    expect(rendered).toBe("example.com");
  });

  it("renders telnet-specific default session with scheme and port", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}"),
      { protocol: "telnet", host: "router", port: 2323 },
    );
    // '//' becomes '__' and ':' becomes '_' after sanitizing.
    expect(rendered).toBe("telnet___router_2323");
  });

  it("renders serial default session using port name and baud rate", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}"),
      { protocol: "serial", serialPort: "/dev/ttyUSB0", baudRate: 115200 },
    );
    expect(rendered).toContain("ttyUSB0");
    expect(rendered).toContain("115200");
  });

  it("preserves unrecognised placeholders literally in the rendered name", () => {
    const rendered = renderLogFileNameTemplate(
      settingsWithTemplate("{session}-{unknown}-{host}"),
      sshContext(),
    );
    // '{unknown}' is not in the whitelist, so the replacement regex ignores it.
    // The trailing sanitiser permits '{' and '}' in the final filename, so the
    // literal token survives rather than being stripped or rewritten. We lock
    // this behaviour in to catch silent changes to the whitelist.
    expect(rendered).toBe("prod-{unknown}-example.com");
  });

  it("defaults numeric port for SSH vs telnet when port is missing", () => {
    const ssh = renderLogFileNameTemplate(
      settingsWithTemplate("{port}"),
      { protocol: "ssh", host: "h", user: "u" },
    );
    const telnet = renderLogFileNameTemplate(
      settingsWithTemplate("{port}"),
      { protocol: "telnet", host: "h" },
    );
    expect(ssh).toBe("22");
    expect(telnet).toBe("23");
  });
});

describe("joinLogPath", () => {
  it("detects Windows-style directories and emits backslash separators", () => {
    const joined = joinLogPath("C:\\Users\\alice\\logs", "session");
    expect(joined).toBe("C:\\Users\\alice\\logs\\session.log");
  });

  it("uses forward slashes for POSIX-style directories", () => {
    const joined = joinLogPath("/var/log/auraterm", "session");
    expect(joined).toBe("/var/log/auraterm/session.log");
  });

  it("strips trailing path separators before appending the file name", () => {
    expect(joinLogPath("/tmp/logs/", "x")).toBe("/tmp/logs/x.log");
    expect(joinLogPath("C:\\tmp\\", "x")).toBe("C:\\tmp\\x.log");
  });

  it("falls back to the default log directory when input is blank", () => {
    const joined = joinLogPath("   ", "x");
    expect(joined.endsWith("x.log")).toBe(true);
    expect(joined).toContain(DEFAULT_SETTINGS.logSavePath);
  });
});

describe("buildDefaultLogPath", () => {
  it("composes directory and rendered file name into a full path", () => {
    const settings = settingsWithTemplate("{session}");
    settings.logSavePath = "/tmp/logs";
    const path = buildDefaultLogPath(settings, sshContext());
    expect(path).toBe("/tmp/logs/prod.log");
  });

  it("uses default log save path when settings is undefined", () => {
    const path = buildDefaultLogPath(undefined, sshContext());
    expect(path.endsWith(".log")).toBe(true);
    expect(path).toContain(DEFAULT_SETTINGS.logSavePath);
  });
});

describe("buildConnectionLogContext", () => {
  it("includes user/host/port for SSH connections", () => {
    const conn: SavedConnection = {
      id: "c1",
      name: "My Box",
      protocol: "ssh",
      host: "h.example.com",
      port: 2222,
      user: "bob",
      authType: "password",
      createdAt: 0,
    };
    expect(buildConnectionLogContext(conn)).toEqual({
      protocol: "ssh",
      host: "h.example.com",
      user: "bob",
      port: 2222,
      session: "My Box",
    });
  });

  it("omits user for telnet connections", () => {
    const conn: SavedConnection = {
      id: "c2",
      name: "Router",
      protocol: "telnet",
      host: "10.0.0.1",
      port: 23,
      user: "",
      authType: "none",
      createdAt: 0,
    };
    const ctx = buildConnectionLogContext(conn);
    expect(ctx.user).toBeUndefined();
    expect(ctx.host).toBe("10.0.0.1");
    expect(ctx.port).toBe(23);
  });

  it("uses serial port name and baud rate for serial connections", () => {
    const conn: SavedConnection = {
      id: "c3",
      name: "Arduino",
      protocol: "serial",
      host: "",
      port: 0,
      user: "",
      authType: "none",
      createdAt: 0,
      portName: "/dev/ttyACM0",
      baudRate: 9600,
    };
    expect(buildConnectionLogContext(conn)).toEqual({
      protocol: "serial",
      serialPort: "/dev/ttyACM0",
      baudRate: 9600,
      session: "Arduino",
    });
  });
});

describe("normalizeOptionalLogPath", () => {
  it("returns undefined when input is undefined", () => {
    expect(normalizeOptionalLogPath(undefined, "/fallback.log")).toBeUndefined();
  });

  it("returns fallback when input is blank", () => {
    expect(normalizeOptionalLogPath("   ", "/fallback.log")).toBe("/fallback.log");
  });

  it("passes through non-empty input unchanged", () => {
    expect(normalizeOptionalLogPath("/custom.log", "/fallback.log")).toBe(
      "/custom.log",
    );
  });
});
