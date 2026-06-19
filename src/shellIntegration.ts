export interface ShellCommandRecord {
  id: number;
  command: string;
  startLine: number;
  endLine?: number;
  exitCode?: number;
  startedAt: number;
  finishedAt?: number;
  source: "osc133" | "heuristic";
}

interface MarkerLike {
  line: number;
  dispose?: () => void;
}

interface DecorationLike {
  onRender?: (callback: (element: HTMLElement) => void) => void;
  dispose?: () => void;
}

export interface ShellTerminalLike {
  parser?: {
    registerOscHandler?: (identifier: number, callback: (data: string) => boolean | Promise<boolean>) => { dispose: () => void };
  };
  registerMarker: (cursorYOffset?: number) => MarkerLike | undefined;
  registerDecoration?: (options: { marker: MarkerLike; x?: number; width?: number; layer?: "bottom" | "top" }) => DecorationLike | undefined;
  scrollToLine: (line: number) => void;
}

interface InternalCommand extends ShellCommandRecord {
  marker: MarkerLike;
  decoration?: DecorationLike;
}

const ANSI_PATTERN = /\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -\/]*[@-~]|\x1b./g;
const PROMPT_PATTERN = /(?:^|\s)(?:[\w.@:/~()[\]-]{0,80})[$#>%]\s*$/;

export class ShellIntegration {
  private commands: InternalCommand[] = [];
  private active: InternalCommand | null = null;
  private input = "";
  private outputTail = "";
  private oscSeen = false;
  private nextId = 1;
  private navigationIndex = -1;
  private readonly oscDisposable: { dispose: () => void };

  constructor(private readonly terminal: ShellTerminalLike, private readonly onChange?: (commands: ShellCommandRecord[]) => void) {
    this.oscDisposable = terminal.parser?.registerOscHandler?.(133, (data) => {
      this.handleOsc(data);
      return true;
    }) ?? { dispose() {} };
  }

  dispose() {
    this.oscDisposable.dispose();
    for (const command of this.commands) {
      command.decoration?.dispose?.();
      command.marker.dispose?.();
    }
  }

  handleInput(data: string) {
    if (!this.active) return;
    for (const character of data) {
      if (character === "\x7f" || character === "\x08") {
        this.input = this.input.slice(0, -1);
      } else if (character === "\r" || character === "\n") {
        if (!this.oscSeen) this.markExecuted();
      } else if (character >= " " && character !== "\x1b") {
        this.input += character;
      }
    }
  }

  processOutput(data: string) {
    if (this.oscSeen) return;
    const plain = data.replace(ANSI_PATTERN, "").replace(/\r/g, "");
    this.outputTail = (this.outputTail + plain).slice(-512);
    const lines = this.outputTail.split("\n");
    const last = lines[lines.length - 1] ?? "";
    if (PROMPT_PATTERN.test(last)) {
      if (this.active && this.active.finishedAt === undefined && this.active.command) this.finish(undefined);
      if (!this.active || this.active.finishedAt !== undefined) this.begin("heuristic");
      this.outputTail = last;
    }
  }

  previous(): ShellCommandRecord | null {
    const navigable = this.commands.filter((command) => command.marker.line >= 0);
    if (!navigable.length) return null;
    this.navigationIndex = this.navigationIndex < 0
      ? navigable.length - 1
      : Math.max(0, this.navigationIndex - 1);
    const command = navigable[this.navigationIndex];
    this.terminal.scrollToLine(command.marker.line);
    return publicRecord(command);
  }

  next(): ShellCommandRecord | null {
    const navigable = this.commands.filter((command) => command.marker.line >= 0);
    if (!navigable.length) return null;
    this.navigationIndex = this.navigationIndex < 0
      ? 0
      : Math.min(navigable.length - 1, this.navigationIndex + 1);
    const command = navigable[this.navigationIndex];
    this.terminal.scrollToLine(command.marker.line);
    return publicRecord(command);
  }

  lastCommand(): ShellCommandRecord | null {
    const command = [...this.commands].reverse().find((candidate) => candidate.command.trim());
    return command ? publicRecord(command) : null;
  }

  records(): ShellCommandRecord[] {
    return this.commands.map(publicRecord);
  }

  private handleOsc(data: string) {
    this.oscSeen = true;
    const [code, value] = data.split(";", 2);
    switch (code) {
      case "A":
        if (this.active && this.active.finishedAt === undefined && this.active.command) this.finish(undefined);
        break;
      case "B":
        this.begin("osc133");
        break;
      case "C":
        this.markExecuted();
        break;
      case "D": {
        const exitCode = value === undefined || value === "" ? undefined : Number.parseInt(value, 10);
        this.finish(Number.isFinite(exitCode) ? exitCode : undefined);
        break;
      }
    }
  }

  private begin(source: "osc133" | "heuristic") {
    const marker = this.terminal.registerMarker(0);
    if (!marker) return;
    this.input = "";
    this.navigationIndex = -1;
    this.active = {
      id: this.nextId++,
      command: "",
      startLine: marker.line,
      startedAt: Date.now(),
      source,
      marker,
    };
    this.commands.push(this.active);
    if (this.commands.length > 500) {
      const removed = this.commands.shift();
      removed?.decoration?.dispose?.();
      removed?.marker.dispose?.();
    }
    this.notify();
  }

  private markExecuted() {
    if (!this.active) this.begin(this.oscSeen ? "osc133" : "heuristic");
    if (!this.active) return;
    this.active.command = this.input.trim();
    this.input = "";
    this.notify();
  }

  private finish(exitCode?: number) {
    if (!this.active) return;
    if (!this.active.command) this.active.command = this.input.trim();
    const end = this.terminal.registerMarker(0);
    this.active.endLine = end?.line;
    end?.dispose?.();
    this.active.exitCode = exitCode;
    this.active.finishedAt = Date.now();
    const command = this.active;
    command.decoration = this.terminal.registerDecoration?.({ marker: command.marker, x: 0, width: 1, layer: "top" });
    command.decoration?.onRender?.((element) => {
      element.classList.add("shell-command-marker");
      element.classList.toggle("failed", exitCode !== undefined && exitCode !== 0);
      element.title = command.command
        ? `${command.command}${exitCode === undefined ? "" : ` (exit ${exitCode})`}`
        : "Shell command";
    });
    this.active = null;
    this.notify();
  }

  private notify() {
    this.onChange?.(this.records());
  }
}

function publicRecord(command: InternalCommand): ShellCommandRecord {
  const { marker: _marker, decoration: _decoration, ...record } = command;
  return { ...record };
}
