import { useEffect, useRef, useState, type FormEvent } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SshConfig, TelnetConfig } from "./ConnectDialog";
import type { AppSettings } from "./settings";
import { DEFAULT_SETTINGS } from "./settings";
import "xterm/css/xterm.css";

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
  telnetConfig?: TelnetConfig;
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

export function TerminalComponent({ isActive, sshConfig, telnetConfig, settings }: TerminalComponentProps) {
  const effectiveSettings = settings ?? DEFAULT_SETTINGS;

  const terminalRef = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<string | null>(null);

  // 内部活跃的 SSH 配置（密码重试时会更新）
  const [activeSshConfig, setActiveSshConfig] = useState<SshConfig | undefined>(sshConfig);
  // 供 Effect 1 的 event handler 读取最新 SSH 配置（避免闭包捕获旧值）
  const activeSshConfigRef = useRef<SshConfig | undefined>(sshConfig);
  // Telnet 配置 ref（供 Effect 1 的 handlers 使用）
  const telnetConfigRef = useRef<TelnetConfig | undefined>(telnetConfig);

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

  // 同步 telnetConfig 到 ref
  useEffect(() => {
    telnetConfigRef.current = telnetConfig;
  }, [telnetConfig]);

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

    // 阻止浏览器/Tauri 拦截 Ctrl+C / Ctrl+D / Ctrl+Z / Ctrl+\ 等控制键，
    // 直接由 xterm 的 onData 处理并转发给 PTY
    terminal.attachCustomKeyEventHandler((e: KeyboardEvent) => {
      if (e.ctrlKey && ['c', 'd', 'z', '\\', 'a', 'e', 'k', 'l', 'r', 'u', 'w'].includes(e.key.toLowerCase())) {
        return true; // 让 xterm 处理，阻止浏览器默认行为
      }
      return true; // 所有键都转给 xterm
    });

    // 输入 handler：通过 ref 读取当前的连接类型
    const inputDisposable = terminal.onData((data) => {
      if (!ptyIdRef.current) return;
      if (activeSshConfigRef.current) {
        void invoke("write_ssh_pty_input", { id: ptyIdRef.current, data }).catch((e) => {
          console.error("write_ssh_pty_input failed", e);
        });
      } else if (telnetConfigRef.current) {
        void invoke("write_telnet_pty_input", { id: ptyIdRef.current, data }).catch((e) => {
          console.error("write_telnet_pty_input failed", e);
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
      } else if (telnetConfigRef.current) {
        void invoke("resize_telnet_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_telnet_pty failed", e);
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
      } else if (telnetConfigRef.current) {
        void invoke("resize_telnet_pty", { id: ptyIdRef.current, cols, rows }).catch((e) => {
          console.error("resize_telnet_pty on window resize failed", e);
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
      inputDisposable.dispose();
      resizeDisposable.dispose();
      // 关闭 PTY（如果 Effect 2 cleanup 已经清空了 ptyIdRef，这里不会重复关闭）
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        if (activeSshConfigRef.current) {
          void invoke("close_ssh_pty", { id }).catch(() => {});
        } else if (telnetConfigRef.current) {
          void invoke("close_telnet_pty", { id }).catch(() => {});
        } else {
          void invoke("close_pty", { id }).catch(() => {});
        }
      }
      terminal.dispose();
      term.current = null;
      fitAddon.current = null;
    };
  }, []); // 空依赖：xterm 只初始化一次

  // ─── Effect 2：管理连接生命周期（依赖 activeSshConfig / telnetConfig）──────
  useEffect(() => {
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;

    const connect = async () => {
      // 等待 xterm 初始化
      if (!term.current) {
        await new Promise<void>((resolve) => setTimeout(resolve, 50));
        if (disposed || !term.current) return;
      }

      unlistenOutput = await listen<PtyOutputEvent>("pty-output", (event) => {
        if (event.payload.id === ptyIdRef.current) {
          term.current?.write(event.payload.data);
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
        } else if (telnetConfig) {
          term.current?.writeln(
            `Connecting to telnet://${telnetConfig.host}:${telnetConfig.port}...`
          );
          await invoke<string>("start_telnet_pty", {
            id: newId,
            host: telnetConfig.host,
            port: telnetConfig.port,
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
      // 关闭当前连接
      if (ptyIdRef.current) {
        const id = ptyIdRef.current;
        ptyIdRef.current = null;
        if (activeSshConfig) {
          void invoke("close_ssh_pty", { id }).catch(() => {});
        } else if (telnetConfig) {
          void invoke("close_telnet_pty", { id }).catch(() => {});
        } else {
          void invoke("close_pty", { id }).catch(() => {});
        }
      }
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSshConfig, telnetConfig]); // 连接变化时重新连接

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

  // ─── 激活时调整大小并聚焦────────────────────────────────────────────────
  useEffect(() => {
    if (isActive) {
      setTimeout(() => {
        fitAddon.current?.fit();
        term.current?.focus();
      }, 50);
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
}
