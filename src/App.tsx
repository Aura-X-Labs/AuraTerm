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
    getCurrentWindow().startDragging().catch((error) => {
      console.error("startDragging failed", error);
    });
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