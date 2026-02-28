import { useEffect, useRef, type MouseEvent } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "xterm/css/xterm.css";

function App() {
  const terminalRef = useRef<HTMLDivElement>(null);
  const term = useRef<Terminal | null>(null);
  const fitAddon = useRef<FitAddon | null>(null);

  const handleTitlebarMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest(".titlebar-controls")) return;
    getCurrentWindow().startDragging().catch((error) => {
      console.error("startDragging failed", error);
    });
  };

  const handleMinimize = async () => {
    await getCurrentWindow().minimize().catch((error) => {
      console.error("minimize failed", error);
    });
  };

  const handleToggleMaximize = async () => {
    const window = getCurrentWindow();
    const isMaximized = await window.isMaximized().catch((error) => {
      console.error("isMaximized failed", error);
      return false;
    });
    if (isMaximized) {
      await window.unmaximize().catch((error) => {
        console.error("unmaximize failed", error);
      });
      return;
    }
    await window.maximize().catch((error) => {
      console.error("maximize failed", error);
    });
  };

  const handleClose = async () => {
    await getCurrentWindow().close().catch((error) => {
      console.error("close failed", error);
    });
  };

  const stopDragPropagation = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

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
      void invoke("write_pty_input", { data }).catch((error) => {
        console.error("write_pty_input failed", error);
      });
    });

    const resizeDisposable = term.current.onResize(({ cols, rows }) => {
      void invoke("resize_pty", { cols, rows }).catch((error) => {
        console.error("resize_pty failed", error);
      });
    });

    const bindPty = async () => {
      unlistenOutput = await listen<string>("pty-output", (event) => {
        term.current?.write(event.payload);
      });

      unlistenExit = await listen<string>("pty-exit", (event) => {
        term.current?.writeln(`\r\n[PTY exited] ${event.payload}`);
      });

      if (disposed) {
        unlistenOutput();
        unlistenExit();
        return;
      }

      const cols = term.current?.cols ?? 80;
      const rows = term.current?.rows ?? 24;
      await invoke("start_pty", { cols, rows });
    };

    void bindPty().catch((error) => {
      term.current?.writeln(`\r\n[Failed to start PTY] ${String(error)}`);
      console.error("start_pty failed", error);
    });

    const handleResize = () => {
      fitAddon.current?.fit();
      const cols = term.current?.cols ?? 80;
      const rows = term.current?.rows ?? 24;
      void invoke("resize_pty", { cols, rows }).catch((error) => {
        console.error("resize_pty on window resize failed", error);
      });
    };

    window.addEventListener('resize', handleResize);

    return () => {
      disposed = true;
      window.removeEventListener('resize', handleResize);
      inputDisposable.dispose();
      resizeDisposable.dispose();
      if (unlistenOutput) unlistenOutput();
      if (unlistenExit) unlistenExit();
      void invoke("close_pty").catch(() => {});
      term.current?.dispose();
    };
  }, []);

  return (
    <div className="app-container">
      <div className="titlebar" onMouseDown={handleTitlebarMouseDown}>
        <div className="titlebar-controls" aria-label="Window controls">
          <button
            className="titlebar-control-btn titlebar-control-close"
            onMouseDown={stopDragPropagation}
            onClick={handleClose}
            aria-label="Close"
            type="button"
          />
          <button
            className="titlebar-control-btn titlebar-control-minimize"
            onMouseDown={stopDragPropagation}
            onClick={handleMinimize}
            aria-label="Minimize"
            type="button"
          />
          <button
            className="titlebar-control-btn titlebar-control-maximize"
            onMouseDown={stopDragPropagation}
            onClick={handleToggleMaximize}
            aria-label="Maximize"
            type="button"
          />
        </div>
        <div className="titlebar-title">AuraTerm</div>
      </div>
      <div className="toolbar">
        <button>New Connection</button>
        <button>Settings</button>
      </div>
      <div className="terminal-container" ref={terminalRef}></div>
    </div>
  );
}

export default App;