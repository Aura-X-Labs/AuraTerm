import { useEffect, useRef, useState, forwardRef, useImperativeHandle, type FormEvent } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SshConfig } from "./ConnectDialog";
import type { AppSettings } from "./settings";
import { DEFAULT_SETTINGS } from "./settings";
import "xterm/css/xterm.css";

// ─── Public handle exposed via ref ─────────────────────────────────────────
export interface TerminalHandle {
  /** Strip ANSI codes and save buffered output to ~/AuraTerm/logs/. Returns the saved path. */
  saveLog: (tabTitle: string) => Promise<string>;
  /** Write raw data to the active PTY (e.g. a command string with trailing \n). */
  sendData: (text: string) => void;
}

/** Removes ANSI / VT escape sequences from a string. */
function stripAnsi(str: string): string {
  return str
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "") // OSC sequences
    .replace(/\x1b\[[0-9;?]*[A-Za-z~]/g, "")            // CSI sequences
    .replace(/\x1b[^[\]]/g, "")                          // other ESC sequences
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

interface TerminalComponentProps {
  isActive: boolean;
  sshConfig?: SshConfig;
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

export const TerminalComponent = forwardRef<TerminalHandle, TerminalComponentProps>(
function TerminalComponent({ isActive, sshConfig, logPath, settings }, ref) {
  const effectiveSettings = settings ?? DEFAULT_SETTINGS;

  // 始终保持最新的 settings，供 Effect 1 内的事件处理器使用（避免闭包读取旧值）
  const settingsRef = useRef<AppSettings>(effectiveSettings);
  useEffect(() => {
    settingsRef.current = effectiveSettings;
  }, [settings]);

  const terminalRef = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<string | null>(null);
  /** Accumulates raw PTY output for the current session. */
  const logBufferRef = useRef<string>("");
  /** Base log path template provided by the caller (no timestamp, no extension). */
  const logPathRef = useRef<string | undefined>(logPath);
  useEffect(() => { logPathRef.current = logPath; }, [logPath]);
  /** Actual log file path for the current session (base + timestamp + .log). */
  const actualLogPathRef = useRef<string | undefined>(undefined);

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
      if (activeSshConfigRef.current) {
        void invoke("write_ssh_pty_input", { id, data }).catch((e) => {
          console.error("write_ssh_pty_input sendData failed", e);
        });
      } else {
        void invoke("write_pty_input", { id, data }).catch((e) => {
          console.error("write_pty_input sendData failed", e);
        });
      }
    },
  }));

  // 内部活跃的 SSH 配置（密码重试时会更新）
  const [activeSshConfig, setActiveSshConfig] = useState<SshConfig | undefined>(sshConfig);
  // 供 Effect 1 的 event handler 读取最新 SSH 配置（避免闭包捕获旧值）
  const activeSshConfigRef = useRef<SshConfig | undefined>(sshConfig);

  // 密码重试 overlay
  const [showRetryOverlay, setShowRetryOverlay] = useState(false);
  const [retryPassword, setRetryPassword] = useState("");

  // 当 prop sshConfig 变化时（新连接），重置内部状态
  useEffect(() => {
    setActiveSshConfig(sshConfig);
    setShowRetryOverlay(false);
    setRetryPassword("");
  }, [sshConfig]);

  // 同步 activeSshConfig 到 ref，供 Effect 1 的 handlers 使用
  useEffect(() => {
    activeSshConfigRef.current = activeSshConfig;
  }, [activeSshConfig]);

  // ─── Effect 1：初始化 xterm（只在 mount/unmount 时执行一次）────────────────
  useEffect(() => {
    if (!terminalRef.current) return;

    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: effectiveSettings.fontFamily,
      fontSize: effectiveSettings.fontSize,
      scrollback: effectiveSettings.scrollback,
      theme: {
        background: effectiveSettings.theme.background,
        foreground: effectiveSettings.theme.foreground,
        cursor: effectiveSettings.theme.cursor,
      },
    });

    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(terminalRef.current);
    fit.fit();

    term.current = terminal;
    fitAddon.current = fit;

    // ─── 自定义键盘事件处理：Ctrl+C / Ctrl+V ─────────────────────────────────
    terminal.attachCustomKeyEventHandler((event: KeyboardEvent) => {
      if (event.type !== "keydown") return true;

      // Ctrl+C：有选中文本时消费事件（不发送 ^C 给 PTY），复制由 onSelectionChange 已处理
      if (event.ctrlKey && event.key === "c" && settingsRef.current.ctrlCCopy) {
        const selection = terminal.getSelection();
        if (selection) {
          // 选中内容已经由 onSelectionChange 写入剪贴板，直接消费按键即可
          return false;
        }
      }

      // Ctrl+V：从剪贴板粘贴到 PTY
      if (event.ctrlKey && event.key === "v" && settingsRef.current.ctrlVPaste) {
        void navigator.clipboard.readText().then((text) => {
          if (!text || !ptyIdRef.current) return;
          const id = ptyIdRef.current;
          if (activeSshConfigRef.current) {
            void invoke("write_ssh_pty_input", { id, data: text }).catch((e) => {
              console.error("write_ssh_pty_input paste failed", e);
            });
          } else {
            void invoke("write_pty_input", { id, data: text }).catch((e) => {
              console.error("write_pty_input paste failed", e);
            });
          }
        }).catch((e) => {
          console.error("clipboard read failed", e);
        });
        return false;
      }

      return true;
    });

    // ─── 选中即复制：每当 selection 变化时，若有内容则写入剪贴板 ──────────────
    const selectionDisposable = terminal.onSelectionChange(() => {
      if (!settingsRef.current.ctrlCCopy) return;
      const selection = terminal.getSelection();
      if (!selection) return;
      void navigator.clipboard.writeText(selection).catch((e) => {
        console.error("clipboard write (selection) failed", e);
      });
    });

    // ─── 鼠标中键粘贴 ──────────────────────────────────────────────────────
    const terminalEl = terminalRef.current;

    // mousedown 阶段就 preventDefault，阻止系统/浏览器弹出粘贴菜单
    const handleMiddleMouseDown = (event: MouseEvent) => {
      if (event.button !== 1) return;
      if (!settingsRef.current.middleClickPaste) return;
      event.preventDefault();
      event.stopPropagation();
    };

    // auxclick 在中键 release 后触发，此时执行实际粘贴
    const handleMiddleClick = (event: MouseEvent) => {
      if (event.button !== 1) return;
      if (!settingsRef.current.middleClickPaste) return;
      event.preventDefault();
      event.stopPropagation();
      void navigator.clipboard.readText().then((text) => {
        if (!text || !ptyIdRef.current) return;
        const id = ptyIdRef.current;
        if (activeSshConfigRef.current) {
          void invoke("write_ssh_pty_input", { id, data: text }).catch((e) => {
            console.error("write_ssh_pty_input middle paste failed", e);
          });
        } else {
          void invoke("write_pty_input", { id, data: text }).catch((e) => {
            console.error("write_pty_input middle paste failed", e);
          });
        }
      }).catch((e) => {
        console.error("clipboard read (middle click) failed", e);
      });
    };

    terminalEl.addEventListener("mousedown", handleMiddleMouseDown);
    terminalEl.addEventListener("auxclick", handleMiddleClick);

    // 输入 handler：通过 ref 读取当前的 activeSshConfig
    const inputDisposable = terminal.onData((data) => {
      if (!ptyIdRef.current) return;
      if (activeSshConfigRef.current) {
        void invoke("write_ssh_pty_input", { id: ptyIdRef.current, data }).catch((e) => {
          console.error("write_ssh_pty_input failed", e);
        });
      } else {
        void invoke("write_pty_input", { id: ptyIdRef.current, data }).catch((e) => {
          console.error("write_pty_input failed", e);
        });
      }
    });

    // resize handler
    const resizeDisposable = terminal.onResize(({ cols, rows }) => {
      if (!ptyIdRef.current) return;
      if (activeSshConfigRef.current) {
        void invoke("resize_ssh_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_ssh_pty failed", e);
        });
      } else {
        void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_pty failed", e);
        });
      }
    });

    const handleWindowResize = () => {
      fit.fit();
      if (!ptyIdRef.current) return;
      const cols = terminal.cols ?? 80;
      const rows = terminal.rows ?? 24;
      if (activeSshConfigRef.current) {
        void invoke("resize_ssh_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_ssh_pty on window resize failed", e);
        });
      } else {
        void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_pty on window resize failed", e);
        });
      }
    };
    window.addEventListener("resize", handleWindowResize);

    return () => {
      window.removeEventListener("resize", handleWindowResize);
      terminalEl.removeEventListener("mousedown", handleMiddleMouseDown);
      terminalEl.removeEventListener("auxclick", handleMiddleClick);
      selectionDisposable.dispose();
      inputDisposable.dispose();
      resizeDisposable.dispose();
      // 关闭 PTY（如果 Effect 2 cleanup 已经清空了 ptyIdRef，这里不会重复关闭）
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        if (activeSshConfigRef.current) {
          void invoke("close_ssh_pty", { id }).catch(() => {});
        } else {
          void invoke("close_pty", { id }).catch(() => {});
        }
      }
      terminal.dispose();
      term.current = null;
      fitAddon.current = null;
    };
  }, []); // 空依赖：xterm 只初始化一次

  // ─── Effect 2：管理连接生命周期（依赖 activeSshConfig）────────────────────
  useEffect(() => {
    // xterm 可能还未初始化（Effect 1 尚未运行完）
    // 用 setTimeout 0 确保在 Effect 1 之后执行
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;

    const connect = async () => {
      // 等待 xterm 初始化
      if (!term.current) {
        await new Promise<void>((resolve) => setTimeout(resolve, 50));
        if (disposed || !term.current) return;
      }

      // Reset log buffer and derive a fresh timestamped log path for this session
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
          // 持续追加写入日志文件（过滤 ANSI 后）
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
          term.current?.writeln(`\r\n[PTY exited] ${event.payload.message}`);
        }
      });

      if (disposed) {
        unlistenOutput();
        unlistenExit();
        return;
      }

      const cols = term.current?.cols ?? 80;
      const rows = term.current?.rows ?? 24;
      const newId = crypto.randomUUID();
      ptyIdRef.current = newId;

      try {
        if (activeSshConfig) {
          term.current?.writeln(
            `Connecting to ${activeSshConfig.user}@${activeSshConfig.host}:${activeSshConfig.port}...`
          );
          await invoke<string>("start_ssh_pty", {
            id: newId,
            host: activeSshConfig.host,
            port: activeSshConfig.port,
            user: activeSshConfig.user,
            password: activeSshConfig.password ?? null,
            privateKey: activeSshConfig.privateKey ?? null,
            cols,
            rows,
          });
          term.current?.writeln("\r\n[Connected]");
        } else {
          term.current?.writeln("Starting local shell PTY...");
          await invoke<string>("start_pty", { id: newId, cols, rows });
        }
      } catch (error) {
        if (disposed) return;
        ptyIdRef.current = null;
        const errStr = String(error);
        term.current?.writeln(`\r\n[Failed to start PTY] ${errStr}`);
        console.error("connect failed", error);

        // 如果是 SSH 认证失败，显示密码重试 overlay
        if (activeSshConfig && isAuthError(errStr)) {
          setShowRetryOverlay(true);
        }
      }
    };

    void connect();

    return () => {
      disposed = true;
      unlistenOutput?.();
      unlistenExit?.();
      // 关闭当前 PTY 连接（activeSshConfig 变化时触发，用于关闭旧连接）
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        if (activeSshConfig) {
          void invoke("close_ssh_pty", { id }).catch(() => {});
        } else {
          void invoke("close_pty", { id }).catch(() => {});
        }
      }
    };
  }, [activeSshConfig]); // activeSshConfig 变化时重新连接（密码重试、新连接）


  // ─── 动态应用 settings 变化（不重启 PTY）─────────────────────────────────
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

  // ─── 激活时调整大小──────────────────────────────────────────────────────
  useEffect(() => {
    if (isActive) {
      setTimeout(() => {
        fitAddon.current?.fit();
      }, 0);
    }
  }, [isActive]);

  // ─── 密码重试处理────────────────────────────────────────────────────────
  const handlePasswordRetry = (e: FormEvent) => {
    e.preventDefault();
    if (!activeSshConfig) return;
    const newConfig: SshConfig = { ...activeSshConfig, password: retryPassword };
    setShowRetryOverlay(false);
    setRetryPassword("");
    setActiveSshConfig(newConfig);
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
    </div>
  );
});
