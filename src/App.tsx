import { useEffect, useRef, type MouseEvent } from "react";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
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

    term.current.writeln('Welcome to AuraTerm!');
    term.current.writeln('This is a basic local terminal skeleton.');
    term.current.writeln('');
    term.current.write('$ ');

    // Basic echo for testing
    term.current.onData((data) => {
      const code = data.charCodeAt(0);
      if (code === 13) { // Enter
        term.current?.write('\r\n$ ');
      } else if (code === 127) { // Backspace
        term.current?.write('\b \b');
      } else {
        term.current?.write(data);
      }
    });

    const handleResize = () => {
      fitAddon.current?.fit();
    };

    window.addEventListener('resize', handleResize);

    return () => {
      window.removeEventListener('resize', handleResize);
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