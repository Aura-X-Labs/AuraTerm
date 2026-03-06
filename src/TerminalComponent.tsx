import { useEffect, useRef, useState, forwardRef, useImperativeHandle, type FormEvent } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SshConfig, TelnetConfig, SerialConfig } from "./ConnectDialog";
import type { AppSettings } from "./settings";
import { DEFAULT_SETTINGS } from "./settings";
import "xterm/css/xterm.css";

export type SessionConfig =
  | { protocol: "local" }
  | { protocol: "ssh"; sshConfig: SshConfig }
  | { protocol: "telnet"; telnetConfig: TelnetConfig }
  | { protocol: "serial"; serialConfig: SerialConfig };

export interface TerminalHandle {
  saveLog: (tabTitle: string) => Promise<string>;
  sendData: (text: string) => void;
}

function stripAnsi(str: string): string {
  return str
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?]*[A-Za-z~]/g, "")
    .replace(/\x1b[^\[\]]/g, "")
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n");
}

interface PtyOutputEvent {
  id: string;
  data: string;
}

interface PtyExitEvent {
  id: string;
  message: string;
}

interface SshMfaPrompt {
  text: string;
  echo: boolean;
}

interface SshMfaPromptEvent {
  id: string;
  name: string;
  instruction: string;
  prompts: SshMfaPrompt[];
}

interface TerminalComponentProps {
  isActive: boolean;
  session: SessionConfig;
  logPath?: string;
  settings?: AppSettings;
}

function isAuthError(errStr: string): boolean {
  const lower = errStr.toLowerCase();
  return (
    lower.includes("authentication failed") ||
    lower.includes("auth failed") ||
    lower.includes("incorrect password") ||
    lower.includes("permission denied")
  );
}

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

export const TerminalComponent = forwardRef<TerminalHandle, TerminalComponentProps>(
function TerminalComponent({ isActive, session, logPath, settings }, ref) {
  const effectiveSettings = settings ?? DEFAULT_SETTINGS;

  const settingsRef = useRef<AppSettings>(effectiveSettings);
  useEffect(() => {
    settingsRef.current = effectiveSettings;
  }, [settings, effectiveSettings]);

  const terminalRef = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<string | null>(null);
  const logBufferRef = useRef<string>("");
  const logPathRef = useRef<string | undefined>(logPath);
  useEffect(() => {
    logPathRef.current = logPath;
  }, [logPath]);
  const actualLogPathRef = useRef<string | undefined>(undefined);

  const [activeSession, setActiveSession] = useState<SessionConfig>(session);
  const activeSessionRef = useRef<SessionConfig>(session);

  const activeSshConfig = activeSession.protocol === "ssh" ? activeSession.sshConfig : undefined;

  const [showRetryOverlay, setShowRetryOverlay] = useState(false);
  const [retryPassword, setRetryPassword] = useState("");
  const [mfaEvent, setMfaEvent] = useState<SshMfaPromptEvent | null>(null);
  const [mfaResponses, setMfaResponses] = useState<string[]>([]);
  const [showMfaOverlay, setShowMfaOverlay] = useState(false);

  useEffect(() => {
    setActiveSession(session);
    setShowRetryOverlay(false);
    setRetryPassword("");
    setMfaEvent(null);
    setMfaResponses([]);
    setShowMfaOverlay(false);
  }, [session]);

  useEffect(() => {
    activeSessionRef.current = activeSession;
  }, [activeSession]);

  useImperativeHandle(ref, () => ({
    saveLog: async (tabTitle: string) => {
      const plain = stripAnsi(logBufferRef.current);
      const path = await invoke<string>("save_terminal_log", {
        content: plain,
        tabName: tabTitle,
      });
      return path;
    },
    sendData: (text: string) => {
      if (!ptyIdRef.current) return;
      const id = ptyIdRef.current;
      const data = text.endsWith("\n") ? text : text + "\n";
      void writeSessionInput(id, data, activeSessionRef.current).catch((e) => {
        console.error("sendData failed", e);
      });
    },
  }));

  useEffect(() => {
    if (!terminalRef.current) return;

    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: "block",
      fontFamily: effectiveSettings.fontFamily,
      fontSize: effectiveSettings.fontSize,
      scrollback: effectiveSettings.scrollback,
      theme: {
        background: effectiveSettings.theme.background,
        foreground: effectiveSettings.theme.foreground,
        cursor: effectiveSettings.theme.cursor,
      },
      convertEol: true,
      allowProposedApi: true,
      macOptionIsMeta: true,
    });

    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(terminalRef.current);
    fit.fit();

    term.current = terminal;
    fitAddon.current = fit;

    terminal.attachCustomKeyEventHandler((event: KeyboardEvent) => {
      if (event.type !== "keydown") return true;

      if (event.ctrlKey && event.key === "c" && settingsRef.current.ctrlCCopy) {
        const selection = terminal.getSelection();
        if (selection) return false;
      }

      if (event.ctrlKey && event.key === "v" && settingsRef.current.ctrlVPaste) {
        void navigator.clipboard.readText().then((text) => {
          if (!text || !ptyIdRef.current) return;
          void writeSessionInput(ptyIdRef.current, text, activeSessionRef.current).catch((e) => {
            console.error("paste failed", e);
          });
        }).catch((e) => {
          console.error("clipboard read failed", e);
        });
        return false;
      }

      return true;
    });

    const selectionDisposable = terminal.onSelectionChange(() => {
      if (!settingsRef.current.ctrlCCopy) return;
      const selection = terminal.getSelection();
      if (!selection) return;
      void navigator.clipboard.writeText(selection).catch((e) => {
        console.error("clipboard write (selection) failed", e);
      });
    });

    const terminalEl = terminalRef.current;

    const handleMiddleMouseDown = (event: MouseEvent) => {
      if (event.button !== 1) return;
      if (!settingsRef.current.middleClickPaste) return;
      event.preventDefault();
      event.stopPropagation();
    };

    const handleMiddleClick = (event: MouseEvent) => {
      if (event.button !== 1) return;
      if (!settingsRef.current.middleClickPaste) return;
      event.preventDefault();
      event.stopPropagation();
      void navigator.clipboard.readText().then((text) => {
        if (!text || !ptyIdRef.current) return;
        void writeSessionInput(ptyIdRef.current, text, activeSessionRef.current).catch((e) => {
          console.error("middle paste failed", e);
        });
      }).catch((e) => {
        console.error("clipboard read (middle click) failed", e);
      });
    };

    terminalEl.addEventListener("mousedown", handleMiddleMouseDown);
    terminalEl.addEventListener("auxclick", handleMiddleClick);

    const inputDisposable = terminal.onData((data) => {
      if (!ptyIdRef.current) return;
      void writeSessionInput(ptyIdRef.current, data, activeSessionRef.current).catch((e) => {
        console.error("input failed", e);
      });
    });

    const resizeDisposable = terminal.onResize(({ cols, rows }) => {
      if (!ptyIdRef.current) return;
      void resizeSession(ptyIdRef.current, cols, rows, activeSessionRef.current).catch((e) => {
        console.error("resize failed", e);
      });
    });

    const handleWindowResize = () => {
      fit.fit();
      if (!ptyIdRef.current) return;
      const cols = terminal.cols ?? 80;
      const rows = terminal.rows ?? 24;
      void resizeSession(ptyIdRef.current, cols, rows, activeSessionRef.current).catch((e) => {
        console.error("resize on window resize failed", e);
      });
    };
    window.addEventListener("resize", handleWindowResize);

    return () => {
      window.removeEventListener("resize", handleWindowResize);
      terminalEl.removeEventListener("mousedown", handleMiddleMouseDown);
      terminalEl.removeEventListener("auxclick", handleMiddleClick);
      selectionDisposable.dispose();
      inputDisposable.dispose();
      resizeDisposable.dispose();
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        void closeSession(id, activeSessionRef.current).catch(() => {});
      }
      terminal.dispose();
      term.current = null;
      fitAddon.current = null;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let unlistenMfa: UnlistenFn | null = null;
    let unlistenConnected: UnlistenFn | null = null;

    const connect = async () => {
      if (!term.current) {
        await new Promise<void>((resolve) => setTimeout(resolve, 50));
        if (disposed || !term.current) return;
      }

      logBufferRef.current = "";
      if (logPathRef.current) {
        const now = new Date();
        const pad = (n: number) => String(n).padStart(2, "0");
        const ts = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}_${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
        actualLogPathRef.current = `${logPathRef.current}_${ts}.log`;
      } else {
        actualLogPathRef.current = undefined;
      }

      unlistenOutput = await listen<PtyOutputEvent>("pty-output", (event) => {
        if (event.payload.id === ptyIdRef.current) {
          term.current?.write(event.payload.data);
          logBufferRef.current += event.payload.data;
          if (actualLogPathRef.current) {
            const plain = stripAnsi(event.payload.data);
            void invoke("append_to_log", {
              path: actualLogPathRef.current,
              content: plain,
            }).catch((e) => console.error("append_to_log failed", e));
          }
        }
      });
      unlistenExit = await listen<PtyExitEvent>("pty-exit", (event) => {
        if (event.payload.id === ptyIdRef.current) {
          const msg = event.payload.message;
          term.current?.writeln(`\r\n[Session exited] ${msg}`);
          if (activeSessionRef.current.protocol === "ssh" && isAuthError(msg)) {
            ptyIdRef.current = null;
            setShowRetryOverlay(true);
          }
        }
      });
      unlistenConnected = await listen<{ id: string }>("ssh-connected", (event) => {
        if (event.payload.id === ptyIdRef.current) {
          term.current?.writeln("\r\n[Connected]");
        }
      });
      unlistenMfa = await listen<SshMfaPromptEvent>("ssh-mfa-prompt", (event) => {
        if (event.payload.id === ptyIdRef.current) {
          setMfaEvent(event.payload);
          setMfaResponses(new Array(event.payload.prompts.length).fill(""));
          setShowMfaOverlay(true);
        }
      });

      if (disposed) {
        unlistenOutput();
        unlistenExit();
        unlistenMfa();
        unlistenConnected?.();
        return;
      }

      const cols = term.current?.cols ?? 80;
      const rows = term.current?.rows ?? 24;
      const newId = crypto.randomUUID();
      ptyIdRef.current = newId;

      try {
        switch (activeSession.protocol) {
          case "ssh":
            term.current?.writeln(
              `Connecting to ${activeSession.sshConfig.user}@${activeSession.sshConfig.host}:${activeSession.sshConfig.port}...`
            );
            await invoke<string>("start_ssh_pty", {
              id: newId,
              host: activeSession.sshConfig.host,
              port: activeSession.sshConfig.port,
              user: activeSession.sshConfig.user,
              password: activeSession.sshConfig.password ?? null,
              privateKey: activeSession.sshConfig.privateKey ?? null,
              cols,
              rows,
            });
            break;
          case "telnet":
            term.current?.writeln(
              `Connecting to telnet://${activeSession.telnetConfig.host}:${activeSession.telnetConfig.port}...`
            );
            await invoke("start_telnet_session", {
              id: newId,
              host: activeSession.telnetConfig.host,
              port: activeSession.telnetConfig.port,
            });
            term.current?.writeln("\r\n[Connected]");
            break;
          case "serial":
            term.current?.writeln(
              `Opening serial port ${activeSession.serialConfig.portName} @ ${activeSession.serialConfig.baudRate}...`
            );
            await invoke("start_serial_session", {
              id: newId,
              portName: activeSession.serialConfig.portName,
              baudRate: activeSession.serialConfig.baudRate,
              dataBits: activeSession.serialConfig.dataBits,
              stopBits: activeSession.serialConfig.stopBits,
              parity: activeSession.serialConfig.parity,
              flowControl: activeSession.serialConfig.flowControl,
            });
            term.current?.writeln("\r\n[Connected]");
            break;
          case "local":
            term.current?.writeln("Starting local shell PTY...");
            await invoke<string>("start_pty", { id: newId, cols, rows });
            break;
        }
      } catch (error) {
        if (disposed) return;
        ptyIdRef.current = null;
        const errStr = String(error);
        term.current?.writeln(`\r\n[Failed to start session] ${errStr}`);
        console.error("connect failed", error);
      }
    };

    void connect();

    return () => {
      disposed = true;
      unlistenOutput?.();
      unlistenExit?.();
      unlistenMfa?.();
      unlistenConnected?.();
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        void closeSession(id, activeSession).catch(() => {});
      }
    };
  }, [activeSession]);

  useEffect(() => {
    if (!term.current || !settings) return;
    term.current.options.fontSize = settings.fontSize;
    term.current.options.fontFamily = settings.fontFamily;
    term.current.options.scrollback = settings.scrollback;
    term.current.options.theme = {
      background: settings.theme.background,
      foreground: settings.theme.foreground,
      cursor: settings.theme.cursor,
    };
    fitAddon.current?.fit();
  }, [settings]);

  useEffect(() => {
    if (isActive) {
      setTimeout(() => {
        fitAddon.current?.fit();
      }, 0);
    }
  }, [isActive]);

  const handlePasswordRetry = (e: FormEvent) => {
    e.preventDefault();
    if (!activeSshConfig) return;
    const newConfig: SshConfig = { ...activeSshConfig, password: retryPassword };
    setShowRetryOverlay(false);
    setRetryPassword("");
    setActiveSession({ protocol: "ssh", sshConfig: newConfig });
  };

  const handleMfaSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!mfaEvent || !ptyIdRef.current) return;
    void invoke("answer_ssh_mfa", {
      id: ptyIdRef.current,
      responses: mfaResponses,
    });
    setShowMfaOverlay(false);
  };

  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        height: "100%",
        display: isActive ? "flex" : "none",
        flexDirection: "column",
      }}
    >
      <div ref={terminalRef} style={{ flex: 1, minHeight: 0 }} />

      {showRetryOverlay && activeSshConfig && (
        <div className="password-retry-overlay">
          <div className="password-retry-dialog">
            <div className="password-retry-icon">🔐</div>
            <h3 className="password-retry-title">认证失败</h3>
            <p className="password-retry-desc">
              无法连接到 <strong>{activeSshConfig.user}@{activeSshConfig.host}</strong>，
              密码不正确，请重新输入。
            </p>
            <form onSubmit={handlePasswordRetry} className="password-retry-form">
              <input
                type="password"
                className="password-retry-input"
                value={retryPassword}
                onChange={(e) => setRetryPassword(e.target.value)}
                placeholder="输入密码"
                onKeyDown={(e) => e.stopPropagation()}
                autoFocus
              />
              <div className="password-retry-actions">
                <button
                  type="button"
                  className="password-retry-btn cancel"
                  onClick={() => setShowRetryOverlay(false)}
                >
                  取消
                </button>
                <button type="submit" className="password-retry-btn retry">
                  重试连接
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {showMfaOverlay && mfaEvent && (
        <div className="password-retry-overlay">
          <div className="password-retry-dialog">
            <div className="password-retry-icon">🛡️</div>
            <h3 className="password-retry-title">{mfaEvent.name || "需要两步验证"}</h3>
            <p className="password-retry-desc">{mfaEvent.instruction}</p>
            <form onSubmit={handleMfaSubmit} className="password-retry-form">
              {mfaEvent.prompts.map((prompt, index) => (
                <div key={index} style={{ marginBottom: "10px" }}>
                  <label style={{ display: "block", marginBottom: "5px", fontSize: "12px", opacity: 0.8 }}>
                    {prompt.text}
                  </label>
                  <input
                    type={prompt.echo ? "text" : "password"}
                    className="password-retry-input"
                    value={mfaResponses[index] || ""}
                    onChange={(e) => {
                      const newResponses = [...mfaResponses];
                      newResponses[index] = e.target.value;
                      setMfaResponses(newResponses);
                    }}
                    onKeyDown={(e) => {
                      e.stopPropagation();
                    }}
                    autoFocus={index === 0}
                    placeholder={prompt.text.replace(/[:：]\s*$/, "")}
                  />
                </div>
              ))}
              <div className="password-retry-actions">
                <button
                  type="button"
                  className="password-retry-btn cancel"
                  onClick={() => {
                    setShowMfaOverlay(false);
                    void invoke("answer_ssh_mfa", {
                      id: ptyIdRef.current,
                      responses: mfaEvent.prompts.map(() => ""),
                    });
                  }}
                >
                  取消
                </button>
                <button type="submit" className="password-retry-btn retry">
                  验证
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </div>
  );
});
