import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TerminalComponent from "../TerminalComponent.vue";
import { DEFAULT_SETTINGS } from "../settings";
import type { SessionConfig } from "../types";

const mockState = vi.hoisted(() => {
  const listeners = new Map<string, Array<(event: { payload: unknown }) => void>>();
  const terminals: MockTerminal[] = [];

  class MockTerminal {
    options: Record<string, unknown>;
    cols = 80;
    rows = 24;
    unicode = { activeVersion: "" };
    written: string[] = [];
    writtenLines: string[] = [];
    selection = "";
    private dataHandler: ((data: string) => void) | null = null;
    private resizeHandler: ((size: { cols: number; rows: number }) => void) | null = null;
    private selectionHandler: (() => void) | null = null;

    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      terminals.push(this);
    }

    loadAddon() {}

    open() {}

    focus() {}

    dispose() {}

    attachCustomKeyEventHandler() {
      return true;
    }

    onSelectionChange(handler: () => void) {
      this.selectionHandler = handler;
      return { dispose() {} };
    }

    onData(handler: (data: string) => void) {
      this.dataHandler = handler;
      return { dispose() {} };
    }

    onResize(handler: (size: { cols: number; rows: number }) => void) {
      this.resizeHandler = handler;
      return { dispose() {} };
    }

    getSelection() {
      return this.selection;
    }

    paste(text: string) {
      this.dataHandler?.(text);
    }

    write(data: string) {
      this.written.push(data);
    }

    writeln(data: string) {
      this.writtenLines.push(data);
    }

    emitData(data: string) {
      this.dataHandler?.(data);
    }

    emitResize(cols: number, rows: number) {
      this.cols = cols;
      this.rows = rows;
      this.resizeHandler?.({ cols, rows });
    }

    emitSelectionChange() {
      this.selectionHandler?.();
    }
  }

  const startSshSession = vi.fn(async () => undefined);
  const writeSessionInput = vi.fn(async () => undefined);
  const resizeSession = vi.fn(async () => undefined);
  const closeSession = vi.fn(async () => undefined);
  const persistUpdatedSshPassword = vi.fn(async () => undefined);
  const saveTerminalLog = vi.fn(async () => "");
  const appendToLog = vi.fn(async () => undefined);
  const answerSshMfa = vi.fn(async () => undefined);
  const answerSshHostKeyMismatch = vi.fn(async () => undefined);
  const answerSshReconnectChoice = vi.fn(async () => undefined);

  const listen = vi.fn(async (eventName: string, handler: (event: { payload: unknown }) => void) => {
    const handlers = listeners.get(eventName) ?? [];
    handlers.push(handler);
    listeners.set(eventName, handlers);

    return () => {
      const current = listeners.get(eventName) ?? [];
      listeners.set(eventName, current.filter((candidate) => candidate !== handler));
    };
  });

  const emitEvent = (eventName: string, payload: unknown) => {
    for (const handler of listeners.get(eventName) ?? []) {
      handler({ payload });
    }
  };

  return {
    MockTerminal,
    answerSshHostKeyMismatch,
    answerSshMfa,
    answerSshReconnectChoice,
    appendToLog,
    closeSession,
    emitEvent,
    listen,
    listeners,
    persistUpdatedSshPassword,
    resizeSession,
    saveTerminalLog,
    startSshSession,
    terminals,
    writeSessionInput,
  };
});

vi.mock("@xterm/xterm", () => ({
  Terminal: mockState.MockTerminal,
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));

vi.mock("@xterm/addon-search", () => ({
  SearchAddon: class {
    constructor(_options?: unknown) {}
  },
}));

vi.mock("@xterm/addon-unicode11", () => ({
  Unicode11Addon: class {},
}));

vi.mock("@xterm/addon-web-links", () => ({
  WebLinksAddon: class {
    constructor(_handler?: unknown) {}
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: mockState.listen,
}));

vi.mock("@tauri-apps/plugin-os", () => ({
  type: () => "macos",
}));

vi.mock("@tauri-apps/plugin-shell", () => ({
  open: vi.fn(async () => undefined),
}));

vi.mock("../composables/useTerminalSearch", () => ({
  useTerminalSearch: () => ({
    clearSearch: vi.fn(),
    clearSearchActiveDecoration: vi.fn(),
    runSearch: vi.fn(() => false),
  }),
}));

vi.mock("../composables/useTerminalSessionCommands", () => ({
  useTerminalSessionCommands: () => ({
    writeSessionInput: mockState.writeSessionInput,
    resizeSession: mockState.resizeSession,
    closeSession: mockState.closeSession,
    getSshReconnectType: (sshConfig: { reconnectType?: string; autoReconnect?: boolean }) => (
      sshConfig.reconnectType ?? (sshConfig.autoReconnect ? "tmux" : "manual")
    ),
    startSshSession: mockState.startSshSession,
    startTelnetSession: vi.fn(async () => undefined),
    startSerialSession: vi.fn(async () => undefined),
    startLocalSession: vi.fn(async () => undefined),
    persistUpdatedSshPassword: mockState.persistUpdatedSshPassword,
    saveTerminalLog: mockState.saveTerminalLog,
    appendToLog: mockState.appendToLog,
    answerSshMfa: mockState.answerSshMfa,
    answerSshHostKeyMismatch: mockState.answerSshHostKeyMismatch,
    answerSshReconnectChoice: mockState.answerSshReconnectChoice,
  }),
}));

describe("TerminalComponent", () => {
  beforeEach(() => {
    mockState.listeners.clear();
    mockState.terminals.length = 0;
    mockState.listen.mockClear();
    mockState.startSshSession.mockClear();
    mockState.writeSessionInput.mockClear();
    mockState.resizeSession.mockClear();
    mockState.closeSession.mockClear();
    mockState.persistUpdatedSshPassword.mockClear();
    mockState.saveTerminalLog.mockClear();
    mockState.appendToLog.mockClear();
    mockState.answerSshMfa.mockClear();
    mockState.answerSshHostKeyMismatch.mockClear();
    mockState.answerSshReconnectChoice.mockClear();
  });

  it("accepts reconnect success events after a manual reconnect", async () => {
    const session: SessionConfig = {
      protocol: "ssh",
      sshConfig: {
        host: "example.com",
        port: 22,
        user: "root",
        reconnectType: "manual",
      },
    };

    const wrapper = mount(TerminalComponent, {
      props: {
        sessionId: "ssh-session-1",
        isVisible: true,
        isFocused: true,
        session,
        settings: DEFAULT_SETTINGS,
      },
    });

    await flushPromises();

    const terminal = mockState.terminals[0];
    expect(terminal).toBeDefined();
    expect(mockState.startSshSession).toHaveBeenCalledTimes(1);

    mockState.emitEvent("pty-exit:ssh-session-1", {
      id: "ssh-session-1",
      message: "Connection dropped",
    });
    await flushPromises();

    expect(terminal.writtenLines).toContain("\r\n[Press r or R to reconnect]");

    terminal.emitData("r");
    await flushPromises();

    expect(mockState.startSshSession).toHaveBeenCalledTimes(2);
    expect(terminal.writtenLines).toContain("\r\n[Reconnecting...]");

    mockState.emitEvent("ssh-connected:ssh-session-1", { id: "ssh-session-1" });
    await flushPromises();

    expect(terminal.writtenLines).toContain("\r\n[Connected]");

    wrapper.unmount();
  });

  it("uses one output rule for highlighting and automatic response", async () => {
    const session: SessionConfig = {
      protocol: "ssh",
      sshConfig: { host: "prod-01", port: 22, user: "ops", reconnectType: "manual" },
    };
    const settings = {
      ...DEFAULT_SETTINGS,
      outputRules: [{
        id: "ready",
        name: "Ready",
        enabled: true,
        pattern: "READY (\\d+)",
        isRegex: true,
        caseSensitive: true,
        scope: "global" as const,
        hosts: [],
        foreground: "#00ff00",
        bell: false,
        notify: false,
        autoResponse: "continue $1\\n",
        cooldownMs: 1000,
      }],
    };

    const wrapper = mount(TerminalComponent, {
      props: {
        sessionId: "ssh-session-rule",
        isVisible: true,
        isFocused: true,
        session,
        settings,
      },
    });
    await flushPromises();

    mockState.emitEvent("pty-output:ssh-session-rule", { id: "ssh-session-rule", data: "READY 7\r\n" });
    await flushPromises();

    const terminal = mockState.terminals[0];
    expect(terminal.written.join("")).toContain("\x1b[38;2;0;255;0mREADY 7\x1b[0m");
    expect(mockState.writeSessionInput).toHaveBeenCalledWith("ssh-session-rule", "continue 7\n", session);
    wrapper.unmount();
  });
});
