import { useEffect, useRef } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { SshConfig } from "./ConnectDialog";
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
}

export function TerminalComponent({ isActive, sshConfig }: TerminalComponentProps) {
  const terminalRef = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);
  const ptyIdRef = useRef<string | null>(null);

  useEffect(() => {
    if (!terminalRef.current) return;

    // Initialize xterm.js
    term.current = new Terminal({
      cursorBlink: true,
      fontFamily: 'Consolas, "Courier New", monospace',
      fontSize: 14,
      theme: {
        background: '#000000',
        foreground: '#ffffff',
      }
    });

    fitAddon.current = new FitAddon();
    term.current.loadAddon(fitAddon.current);

    term.current.open(terminalRef.current);
    fitAddon.current.fit();

    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;

    const inputDisposable = term.current.onData((data) => {
      if (ptyIdRef.current) {
        if (sshConfig) {
          void invoke("write_ssh_pty_input", { id: ptyIdRef.current, data }).catch((error) => {
            console.error("write_ssh_pty_input failed", error);
          });
        } else {
          void invoke("write_pty_input", { id: ptyIdRef.current, data }).catch((error) => {
            console.error("write_pty_input failed", error);
          });
        }
      }
    });

    const resizeDisposable = term.current.onResize(({ cols, rows }) => {
      if (ptyIdRef.current) {
        if (sshConfig) {
          void invoke("resize_ssh_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
            console.error("resize_ssh_pty failed", error);
          });
        } else {
          void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
            console.error("resize_pty failed", error);
          });
        }
      }
    });

    const bindPty = async () => {
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
      try {
        const newId = crypto.randomUUID();
        ptyIdRef.current = newId;

        if (sshConfig) {
          term.current?.writeln(`Connecting to ${sshConfig.user}@${sshConfig.host}:${sshConfig.port}...`);
          await invoke<string>("start_ssh_pty", {
            id: newId,
            host: sshConfig.host,
            port: sshConfig.port,
            user: sshConfig.user,
            password: sshConfig.password || null,
            privateKey: sshConfig.privateKey || null,
            cols,
            rows,
          });
          term.current?.writeln('\r\n[Connected]');
        } else {
          term.current?.writeln('Starting local shell PTY...');
          await invoke<string>("start_pty", { id: newId, cols, rows });
        }
      } catch (error) {
        ptyIdRef.current = null;
        term.current?.writeln(`\r\n[Failed to start PTY] ${String(error)}`);
        console.error("start_pty failed", error);
      }
    };

    void bindPty();

    const handleResize = () => {
      fitAddon.current?.fit();
      if (ptyIdRef.current) {
        const cols = term.current?.cols ?? 80;
        const rows = term.current?.rows ?? 24;
        if (sshConfig) {
          void invoke("resize_ssh_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
            console.error("resize_ssh_pty on window resize failed", error);
          });
        } else {
          void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
            console.error("resize_pty on window resize failed", error);
          });
        }
      }
    };

    window.addEventListener('resize', handleResize);

    return () => {
      disposed = true;
      window.removeEventListener('resize', handleResize);
      inputDisposable.dispose();
      resizeDisposable.dispose();
      if (unlistenOutput) unlistenOutput();
      if (unlistenExit) unlistenExit();
      if (ptyIdRef.current) {
        if (sshConfig) {
          void invoke("close_ssh_pty", { id: ptyIdRef.current }).catch(() => {});
        } else {
          void invoke("close_pty", { id: ptyIdRef.current }).catch(() => {});
        }
      }
      term.current?.dispose();
    };
  }, [sshConfig]);

  // Make sure to resize when becoming active because display: none ruins size
  useEffect(() => {
    if (isActive) {
      setTimeout(() => {
        fitAddon.current?.fit();
      }, 0);
    }
  }, [isActive]);

  return (
    <div
      ref={terminalRef}
      style={{
        width: "100%",
        height: "100%",
        display: isActive ? "block" : "none"
      }}
    />
  );
}
