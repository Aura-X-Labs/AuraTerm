import { useEffect, useRef, useState } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
}

export function TerminalComponent({ isActive }: TerminalComponentProps) {
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

    term.current.writeln('Starting local shell PTY...');

    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;

    const inputDisposable = term.current.onData((data) => {
      if (ptyIdRef.current) {
        void invoke("write_pty_input", { id: ptyIdRef.current, data }).catch((error) => {
          console.error("write_pty_input failed", error);
        });
      }
    });

    const resizeDisposable = term.current.onResize(({ cols, rows }) => {
      if (ptyIdRef.current) {
        void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
          console.error("resize_pty failed", error);
        });
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
        const id = await invoke<string>("start_pty", { cols, rows });
        ptyIdRef.current = id;
      } catch (error) {
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
        void invoke("resize_pty", { id: ptyIdRef.current, cols, rows }).catch((error) => {
          console.error("resize_pty on window resize failed", error);
        });
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
        void invoke("close_pty", { id: ptyIdRef.current }).catch(() => {});
      }
      term.current?.dispose();
    };
  }, []);

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
