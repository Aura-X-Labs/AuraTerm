<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { type as getOsType } from "@tauri-apps/plugin-os";
import { open as openExternalUrl } from "@tauri-apps/plugin-shell";
import { useTerminalSearch } from "./composables/useTerminalSearch";
import { useTerminalSessionCommands } from "./composables/useTerminalSessionCommands";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { OutputRuleEngine } from "./outputRules";
import { decodeControlCharacters } from "./snippets";
import { ShellIntegration } from "./shellIntegration";
import type {
  SerialConnectionState,
  SessionConfig,
  SshConfig,
  TerminalHandle,
  TerminalSearchResults,
  ZmodemTransferEvent,
} from "./types";
import "@xterm/xterm/css/xterm.css";
import "./styles/shell-integration.css";

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

interface SshReconnectSessionPromptEvent {
  id: string;
  tool: string;
  sessions: string[];
}

interface SshHostKeyMismatchPromptEvent {
  id: string;
  host: string;
  port: number;
  expectedFingerprint: string;
  observedFingerprint: string;
}

interface ReconnectSessionPromptState {
  id: string;
  tool: string;
  sessions: string[];
}

const props = defineProps<{
  sessionId: string;
  isVisible: boolean;
  isFocused: boolean;
  session: SessionConfig;
  logPath?: string;
  settings?: AppSettings;
  /** When true and this terminal is focused, typed input is also emitted via
   *  `broadcastInput` so the parent can fan it out to other panes (MultiExec). */
  broadcast?: boolean;
}>();

const emit = defineEmits<{
  serialConnectionStateChange: [state: SerialConnectionState];
  sessionUpdate: [session: SessionConfig];
  sshPasswordUpdated: [];
  searchResultsChange: [results: TerminalSearchResults];
  broadcastInput: [data: string];
  sshConnected: [];
}>();

function stripAnsi(value: string): string {
  return value
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    .replace(/\x1b\[[0-9;?]*[A-Za-z~]/g, "")
    .replace(/\x1b[^\[\]]/g, "")
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n");
}

function isAuthError(errorText: string): boolean {
  const lower = errorText.toLowerCase();
  return (
    lower.includes("authentication failed")
    || lower.includes("auth failed")
    || lower.includes("incorrect password")
    || lower.includes("permission denied")
  );
}

function usesPasswordAuth(config: SshConfig): boolean {
  return (config.authType ?? (config.privateKey ? "key" : "password")) === "password";
}

function writePasswordRetryPrompt(target: Terminal | null) {
  target?.writeln("\r\n[Authentication failed] Incorrect password. Please enter a new password to retry.");
}

function formatLogTimestampParts(date = new Date()) {
  const pad = (value: number) => String(value).padStart(2, "0");
  const yyyy = String(date.getFullYear());
  const MM = pad(date.getMonth() + 1);
  const dd = pad(date.getDate());
  const HH = pad(date.getHours());
  const mm = pad(date.getMinutes());
  const ss = pad(date.getSeconds());
  const unix = String(Math.floor(date.getTime() / 1000));
  return {
    yyyy,
    MM,
    dd,
    HH,
    mm,
    ss,
    unix,
    date: `${yyyy}${MM}${dd}`,
    time: `${HH}${mm}${ss}`,
    timestamp: `${yyyy}${MM}${dd}_${HH}${mm}${ss}`,
  };
}

function resolveLogPathPlaceholders(path: string) {
  const parts = formatLogTimestampParts();
  return path
    .replace(/\{timestamp\}/g, parts.timestamp)
    .replace(/\{datetime\}/g, parts.timestamp)
    .replace(/\{date\}/g, parts.date)
    .replace(/\{time\}/g, parts.time)
    .replace(/\{yyyy\}/g, parts.yyyy)
    .replace(/\{MM\}/g, parts.MM)
    .replace(/\{dd\}/g, parts.dd)
    .replace(/\{HH\}/g, parts.HH)
    .replace(/\{mm\}/g, parts.mm)
    .replace(/\{ss\}/g, parts.ss)
    .replace(/\{unix\}/g, parts.unix);
}

function buildTerminalTheme(settings: AppSettings) {
  return { ...settings.theme };
}

const effectiveSettings = computed(() => props.settings ?? DEFAULT_SETTINGS);
const settingsRef = ref<AppSettings>(effectiveSettings.value);
const terminalRootRef = ref<HTMLDivElement | null>(null);
const ptyId = ref<string | null>(null);
const osType = ref("unknown");
const logPathRef = ref<string | undefined>(props.logPath);
const actualLogPath = ref<string | undefined>(undefined);
const activeSession = ref<SessionConfig>(props.session);
const activeSessionRef = ref<SessionConfig>(props.session);
const showRetryOverlay = ref(false);
const retryPassword = ref("");
const mfaEvent = ref<SshMfaPromptEvent | null>(null);
const mfaResponses = ref<string[]>([]);
const showMfaOverlay = ref(false);
const reconnectPrompt = ref<ReconnectSessionPromptState | null>(null);
const selectedReconnectSession = ref("");
const hostKeyPrompt = ref<SshHostKeyMismatchPromptEvent | null>(null);
const showHostKeyOverlay = ref(false);
const manualReconnectPending = ref(false);
const sessionRevision = ref(0);
const activeSshConfig = computed(() => (
  activeSession.value.protocol === "ssh" ? activeSession.value.sshConfig : undefined
));
const terminalRef = ref<Terminal | null>(null);
const searchAddonRef = ref<SearchAddon | null>(null);

let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let searchAddon: SearchAddon | null = null;
let webglAddon: WebglAddon | null = null;
// True once WebGL has hard-failed for this terminal (unavailable, or a context
// loss with no successful restore). Once set, we stop retrying WebGL for the
// lifetime of this terminal and stay on the DOM renderer.
let webglDisabled = false;
let terminalCleanup: (() => void) | null = null;
let shellIntegration: ShellIntegration | null = null;
let stopSessionWatch: (() => void) | null = null;
let pendingInputBuffer = "";
let pendingInputTimer: number | null = null;
// Plain (non-reactive) accumulator backing the on-demand "Save log" action.
// Nothing renders it, so a ref would only add reactive-setter overhead on every
// pty-output chunk.
let logBuffer = "";
let pendingLogBuffer = "";
let pendingLogPath: string | null = null;
let pendingLogTimer: number | null = null;
const outputRuleEngine = new OutputRuleEngine();
const zmodemTransfer = ref<ZmodemTransferEvent | null>(null);
const zmodemFileInput = ref<HTMLInputElement | null>(null);
const zmodemBusy = ref(false);

const LOG_FLUSH_INTERVAL_MS = 180;
const LOG_FLUSH_MAX_CHUNK = 4096;
// Bound the in-memory "Save log" buffer so a long-running / high-volume session
// cannot grow it without limit. When exceeded we keep the most recent
// SAVED_LOG_MAX_CHARS characters; the slack lets it grow a little past the cap
// before trimming, so we don't re-slice a multi-MB string on every chunk.
const SAVED_LOG_MAX_CHARS = 4_000_000;
const SAVED_LOG_TRIM_SLACK = 1_000_000;

const {
  writeSessionInput,
  writeSessionBytes,
  resizeSession,
  closeSession,
  getSshReconnectType,
  startSshSession,
  startTelnetSession,
  startSerialSession,
  startLocalSession,
  startZmodemSend,
  cancelZmodem,
  persistUpdatedSshPassword,
  saveTerminalLog,
  appendToLog,
  answerSshMfa,
  answerSshHostKeyMismatch,
  answerSshReconnectChoice,
} = useTerminalSessionCommands({
  onSshPasswordUpdated: () => {
    emit("sshPasswordUpdated");
  },
});

const {
  clearSearch,
  clearSearchActiveDecoration,
  runSearch,
} = useTerminalSearch({
  terminal: terminalRef,
  searchAddon: searchAddonRef,
  onResultsChange: (results) => {
    emit("searchResultsChange", results);
  },
});

function clearPendingInputFlush() {
  if (pendingInputTimer !== null) {
    window.clearTimeout(pendingInputTimer);
    pendingInputTimer = null;
  }
}

function flushPendingInput() {
  clearPendingInputFlush();
  if (!pendingInputBuffer || !ptyId.value) {
    pendingInputBuffer = "";
    return;
  }
  const payload = pendingInputBuffer;
  pendingInputBuffer = "";
  void writeSessionInput(ptyId.value, payload, activeSessionRef.value).catch((error) => {
    console.error("input failed", error);
  });
}

function queueTerminalInput(data: string) {
  pendingInputBuffer += data;
  if (pendingInputTimer !== null) {
    return;
  }
  // Micro-batch keystrokes to reduce IPC pressure and backend queue jitter.
  pendingInputTimer = window.setTimeout(() => {
    flushPendingInput();
  }, 10);
}

function clearPendingLogFlush() {
  if (pendingLogTimer !== null) {
    window.clearTimeout(pendingLogTimer);
    pendingLogTimer = null;
  }
}

function flushPendingLog(force = false) {
  if (!pendingLogBuffer || !pendingLogPath) {
    pendingLogBuffer = "";
    pendingLogPath = null;
    clearPendingLogFlush();
    return;
  }

  if (!force && pendingLogBuffer.length < LOG_FLUSH_MAX_CHUNK) {
    return;
  }

  const path = pendingLogPath;
  const payload = pendingLogBuffer;
  pendingLogBuffer = "";
  pendingLogPath = null;
  clearPendingLogFlush();

  void appendToLog(path, payload).catch((error) => {
    console.error("append_to_log failed", error);
  });
}

function queueLogChunk(path: string, chunk: string) {
  if (!chunk) {
    return;
  }

  if (pendingLogPath && pendingLogPath !== path) {
    flushPendingLog(true);
  }

  pendingLogPath = path;
  pendingLogBuffer += chunk;

  if (pendingLogBuffer.length >= LOG_FLUSH_MAX_CHUNK) {
    flushPendingLog(true);
    return;
  }

  if (pendingLogTimer !== null) {
    return;
  }

  pendingLogTimer = window.setTimeout(() => {
    pendingLogTimer = null;
    flushPendingLog(true);
  }, LOG_FLUSH_INTERVAL_MS);
}

watch(effectiveSettings, (value) => {
  settingsRef.value = value;
}, { deep: true, immediate: true });

watch(() => props.logPath, (value) => {
  logPathRef.value = value;
});

watch(() => props.session, (session) => {
  activeSession.value = session;
  showRetryOverlay.value = false;
  retryPassword.value = "";
  mfaEvent.value = null;
  mfaResponses.value = [];
  showMfaOverlay.value = false;
  reconnectPrompt.value = null;
  selectedReconnectSession.value = "";
  hostKeyPrompt.value = null;
  showHostKeyOverlay.value = false;
  manualReconnectPending.value = false;
  sessionRevision.value += 1;
  if (session.protocol === "serial") {
    notifySerialConnectionStateChange("connecting");
  }
}, { deep: true });

watch(activeSession, (session) => {
  activeSessionRef.value = session;
}, { deep: true, immediate: true });

watch(effectiveSettings, (value) => {
  if (!terminal) {
    return;
  }
  terminal.options.fontSize = value.fontSize;
  terminal.options.fontFamily = value.fontFamily;
  terminal.options.scrollback = value.scrollback;
  terminal.options.theme = buildTerminalTheme(value);
  fitAddon?.fit();
}, { deep: true });

watch(() => props.isFocused, (isFocused) => {
  if (!isFocused || !props.isVisible) {
    return;
  }
  setTimeout(() => {
    fitAddon?.fit();
    terminal?.focus();
  }, 0);
});

/**
 * Attach the WebGL (GPU) renderer to this terminal.
 *
 * Only the visible terminal needs fast rendering, and every tab keeps its
 * TerminalComponent mounted (hidden via CSS — see App.vue's `v-for="tab in
 * tabs"`), so loading WebGL for all terminals at once would exhaust the
 * browser's ~16 simultaneous WebGL context limit and trigger context-loss
 * storms. We therefore attach on show and detach on hide, keeping the number
 * of live GL contexts ≈ the number of on-screen panes.
 */
function attachRenderer() {
  if (!terminal || webglAddon || webglDisabled) {
    return;
  }
  if (effectiveSettings.value.rendererMode === "dom") {
    return;
  }
  try {
    const addon = new WebglAddon();
    // If the GPU context is lost (browser reclaimed it, driver reset, etc.),
    // dispose and fall back to the DOM renderer rather than leaving a blank or
    // frozen terminal. Don't retry WebGL afterwards for this terminal.
    addon.onContextLoss(() => {
      detachRenderer();
      webglDisabled = true;
    });
    terminal.loadAddon(addon); // must run after terminal.open()
    webglAddon = addon;
  } catch {
    // WebGL unavailable in this webview (e.g. some WKWebGTK/WKWebView builds).
    // Stay on the DOM renderer and stop retrying for this terminal.
    webglDisabled = true;
  }
}

function detachRenderer() {
  webglAddon?.dispose();
  webglAddon = null;
}

watch(() => props.isVisible, (isVisible) => {
  if (!isVisible) {
    // Release the GPU context while hidden so it doesn't count against the
    // browser's WebGL context limit.
    detachRenderer();
    return;
  }

  attachRenderer();
  setTimeout(() => {
    fitAddon?.fit();
    if (props.isFocused) {
      terminal?.focus();
    }
  }, 0);
});

// React to the user changing the renderer preference at runtime.
watch(() => effectiveSettings.value.rendererMode, (mode) => {
  if (mode === "dom") {
    detachRenderer();
    return;
  }
  // Switching back to a GPU mode re-enables WebGL; attach now if visible.
  webglDisabled = false;
  if (props.isVisible) {
    attachRenderer();
  }
});

function notifySerialConnectionStateChange(state: SerialConnectionState) {
  if (activeSessionRef.value.protocol === "serial") {
    emit("serialConnectionStateChange", state);
  }
}

async function saveLog(tabTitle: string) {
  const plain = stripAnsi(logBuffer);
  return saveTerminalLog(plain, tabTitle);
}

function sendData(text: string) {
  if (!ptyId.value) {
    return;
  }
  const data = text.endsWith("\n") ? text : `${text}\n`;
  queueTerminalInput(data);
}

// Raw passthrough used by broadcast fan-out: write to this session exactly as
// received, without sendData's newline append. No-op until the pty is live so
// keystrokes aren't buffered for a not-yet-connected target pane.
function writeInput(data: string) {
  if (!ptyId.value) {
    return;
  }
  queueTerminalInput(data);
}

function fit() {
  fitAddon?.fit();
}

function focus() {
  terminal?.focus();
}

function previousCommand() {
  return Boolean(shellIntegration?.previous());
}

function nextCommand() {
  return Boolean(shellIntegration?.next());
}

function rerunLastCommand() {
  const command = shellIntegration?.lastCommand()?.command;
  if (!command) return false;
  sendData(command);
  return true;
}

async function copyLastCommand() {
  const command = shellIntegration?.lastCommand()?.command;
  if (!command) return false;
  await navigator.clipboard.writeText(command);
  return true;
}

async function reconnectSshSession() {
  if (activeSessionRef.value.protocol !== "ssh" || !terminal) {
    return;
  }

  manualReconnectPending.value = false;
  ptyId.value = props.sessionId;
  terminal.writeln("\r\n[Reconnecting...]");

  try {
    await startSshSession(
      props.sessionId,
      activeSessionRef.value.sshConfig,
      terminal.cols ?? 80,
      terminal.rows ?? 24,
    );
  } catch (error) {
    const errorText = String(error);
    ptyId.value = null;
    if (isAuthError(errorText) && usesPasswordAuth(activeSessionRef.value.sshConfig)) {
      writePasswordRetryPrompt(terminal);
      showRetryOverlay.value = true;
    } else {
      terminal.writeln(`\r\n[Reconnect failed] ${errorText}`);
      manualReconnectPending.value = true;
      terminal.writeln("\r\n[Press r or R to reconnect]");
    }
  }
}

defineExpose<TerminalHandle>({
  saveLog,
  sendData,
  writeInput,
  fit,
  focus,
  findNext: (text, options) => runSearch("next", text, options),
  findPrevious: (text, options) => runSearch("previous", text, options),
  clearSearch,
  clearSearchActiveDecoration,
  previousCommand,
  nextCommand,
  rerunLastCommand,
  copyLastCommand,
});

onMounted(() => {
  if (!terminalRootRef.value) {
    return;
  }

  try {
    osType.value = getOsType();
  } catch (error) {
    console.error("Failed to detect OS:", error);
  }

  terminal = new Terminal({
    cursorBlink: true,
    cursorStyle: "block",
    fontFamily: effectiveSettings.value.fontFamily,
    fontSize: effectiveSettings.value.fontSize,
    scrollback: effectiveSettings.value.scrollback,
    theme: buildTerminalTheme(effectiveSettings.value),
    convertEol: false,
    allowProposedApi: true,
    macOptionIsMeta: true,
  });

  fitAddon = new FitAddon();
  // highlightLimit:0 means _highlightAllMatches() exits immediately (0 >= 0 → break).
  // This eliminates the O(n_matches) buffer scan + decoration-object creation that
  // froze WebKit while still allowing the active match highlight.
  searchAddon = new SearchAddon({ highlightLimit: 0 });
  terminalRef.value = terminal;
  searchAddonRef.value = searchAddon;
  const unicode11Addon = new Unicode11Addon();
  // Make URLs in terminal output clickable. window.open is unreliable inside the
  // Tauri webview, so route the click through the shell plugin to launch the OS
  // default browser instead.
  const webLinksAddon = new WebLinksAddon((event, uri) => {
    // Honor the same modifier-click convention as editors/browsers: a plain
    // click only opens when the link is not part of a text selection gesture.
    if (event.button !== 0) {
      return;
    }
    void openExternalUrl(uri).catch((error) => {
      console.error("failed to open link", error);
    });
  });
  terminal.loadAddon(fitAddon);
  terminal.loadAddon(searchAddon);
  terminal.loadAddon(unicode11Addon);
  terminal.loadAddon(webLinksAddon);
  terminal.open(terminalRootRef.value);
  // The isVisible watch is not immediate, so attach the renderer here for a
  // terminal that is already visible on first paint.
  if (props.isVisible) {
    attachRenderer();
  }
  shellIntegration = new ShellIntegration(terminal as unknown as ConstructorParameters<typeof ShellIntegration>[0]);
  // Activate Unicode 11 so emoji and wide characters (e.g. 💰) are treated as
  // 2 columns wide, preventing them from overlapping the following character.
  terminal.unicode.activeVersion = "11";
  fitAddon.fit();
  if (props.isFocused) {
    terminal.focus();
  }

  terminal.attachCustomKeyEventHandler((event: KeyboardEvent) => {
    if (event.type !== "keydown") {
      return true;
    }

    if (event.ctrlKey && event.key === "c" && settingsRef.value.ctrlCCopy) {
      const selection = terminal?.getSelection();
      if (selection) {
        return false;
      }
    }

    if (event.ctrlKey && event.key === "v" && settingsRef.value.ctrlVPaste) {
      // Prevent default browser paste
      event.preventDefault();
      
      // Use xterm's paste method to avoid duplication
      void navigator.clipboard.readText().then((text) => {
        if (text && terminal) {
          // Use xterm's built-in paste which properly triggers onData
          terminal.paste(text);
        }
      }).catch((error) => {
        console.error("clipboard read failed", error);
      });
      
      return false; // Prevent default handling
    }

    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key === "ArrowUp") {
      event.preventDefault();
      previousCommand();
      return false;
    }
    if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key === "ArrowDown") {
      event.preventDefault();
      nextCommand();
      return false;
    }

    return true;
  });

  const selectionDisposable = terminal.onSelectionChange(() => {
    if (!settingsRef.value.ctrlCCopy) {
      return;
    }
    const selection = terminal?.getSelection();
    if (!selection) {
      return;
    }
    void navigator.clipboard.writeText(selection).catch((error) => {
      console.error("clipboard write (selection) failed", error);
    });
  });

  const terminalElement = terminalRootRef.value;

  const handleMiddleMouseDown = (event: MouseEvent) => {
    if (event.button !== 1 || !settingsRef.value.middleClickPaste) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const handleMiddleClick = (event: MouseEvent) => {
    if (event.button !== 1 || !settingsRef.value.middleClickPaste) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    void navigator.clipboard.readText().then((text) => {
      if (!text || !ptyId.value) {
        return;
      }
      void writeSessionInput(ptyId.value, text, activeSessionRef.value).catch((error) => {
        console.error("middle paste failed", error);
      });
    }).catch((error) => {
      console.error("clipboard read (middle click) failed", error);
    });
  };

  terminalElement.addEventListener("mousedown", handleMiddleMouseDown);
  terminalElement.addEventListener("auxclick", handleMiddleClick);

  const inputDisposable = terminal.onData((data) => {
    if (manualReconnectPending.value) {
      if (data === "r" || data === "R") {
        void reconnectSshSession();
      }
      return;
    }

    if (!ptyId.value) {
      return;
    }
    const shouldNormalizeBackspace = activeSessionRef.value.protocol === "local"
      && ["macos", "linux"].includes(osType.value)
      && data === "\x08";
    const normalizedData = shouldNormalizeBackspace ? "\x7f" : data;
    shellIntegration?.handleInput(normalizedData);
    queueTerminalInput(normalizedData);
    // MultiExec: when broadcasting, the focused terminal is the source — hand the
    // keystroke to the parent to fan out to the other panes. writeInput on the
    // targets does not re-trigger onData, so there is no feedback loop.
    if (props.broadcast && props.isFocused) {
      emit("broadcastInput", normalizedData);
    }
  });

  const resizeDisposable = terminal.onResize(({ cols, rows }) => {
    if (!ptyId.value) {
      return;
    }
    void resizeSession(ptyId.value, cols, rows, activeSessionRef.value).catch((error) => {
      console.error("resize failed", error);
    });
  });

  const handleWindowResize = () => {
    if (!props.isVisible) {
      return;
    }
    fitAddon?.fit();
    if (!ptyId.value || !terminal) {
      return;
    }
    const cols = terminal.cols ?? 80;
    const rows = terminal.rows ?? 24;
    void resizeSession(ptyId.value, cols, rows, activeSessionRef.value).catch((error) => {
      console.error("resize on window resize failed", error);
    });
  };

  window.addEventListener("resize", handleWindowResize);

  terminalCleanup = () => {
    flushPendingInput();
    flushPendingLog(true);
    window.removeEventListener("resize", handleWindowResize);
    terminalElement.removeEventListener("mousedown", handleMiddleMouseDown);
    terminalElement.removeEventListener("auxclick", handleMiddleClick);
    selectionDisposable.dispose();
    inputDisposable.dispose();
    resizeDisposable.dispose();
    if (ptyId.value) {
      const id = ptyId.value;
      ptyId.value = null;
      void closeSession(id, activeSessionRef.value).catch(() => {});
    }
      detachRenderer();
      terminal?.dispose();
      shellIntegration?.dispose();
      shellIntegration = null;
    terminal = null;
    terminalRef.value = null;
    fitAddon = null;
    searchAddon = null;
    searchAddonRef.value = null;
  };

  const sessionKey = computed(() => {
    const s = activeSession.value;
    switch (s.protocol) {
      case "local":
        return `local:${s.cwd || ""}:rev:${sessionRevision.value}`;
      case "ssh":
        return `ssh:${s.sshConfig.user}@${s.sshConfig.host}:${s.sshConfig.port}:${s.sshConfig.privateKey ? "key" : "password"}:rev:${sessionRevision.value}`;
      case "telnet":
        return `telnet:${s.telnetConfig.host}:${s.telnetConfig.port}:rev:${sessionRevision.value}`;
      case "serial":
        return `serial:${s.serialConfig.portName}:${s.serialConfig.baudRate}:${s.serialConfig.dataBits}:${s.serialConfig.stopBits}:${s.serialConfig.parity}:${s.serialConfig.flowControl}:rev:${sessionRevision.value}`;
      default:
        return "unknown";
    }
  });

  stopSessionWatch = watch(sessionKey, (_newKey, _oldKey, onCleanup) => {
    const session = activeSession.value;
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let unlistenMfa: UnlistenFn | null = null;
    let unlistenSshConnected: UnlistenFn | null = null;
    let unlistenReconnectPrompt: UnlistenFn | null = null;
    let unlistenHostKeyMismatch: UnlistenFn | null = null;
    let unlistenSerialConnected: UnlistenFn | null = null;
    let unlistenZmodem: UnlistenFn | null = null;

    const cleanup = () => {
      disposed = true;
      flushPendingLog(true);
      unlistenOutput?.();
      unlistenExit?.();
      unlistenMfa?.();
      unlistenSshConnected?.();
      unlistenReconnectPrompt?.();
      unlistenHostKeyMismatch?.();
      unlistenSerialConnected?.();
      unlistenZmodem?.();
      if (ptyId.value) {
        const id = ptyId.value;
        ptyId.value = null;
        void closeSession(id, session).catch((err) => {
          console.error(`[Terminal] Failed to close session ${id}:`, err);
        });
      }
    };

    onCleanup(cleanup);

    const connect = async () => {
      if (!terminal) {
        console.warn("[Terminal] Cannot connect: terminal not initialized");
        return;
      }

      flushPendingLog(true);
      logBuffer = "";
      outputRuleEngine.reset();
      if (logPathRef.value) {
        const trimmedPath = logPathRef.value.trim();
        const resolvedPath = resolveLogPathPlaceholders(trimmedPath);
        actualLogPath.value = resolvedPath.endsWith(".log") ? resolvedPath : `${resolvedPath}.log`;
      } else {
        actualLogPath.value = undefined;
      }


      // Per-session event names (see `util::session_event` on the Rust side):
      // the backend emits to `pty-output:<sessionId>`, so only this component's
      // listener is woken — no global broadcast + id-filter fan-out across panes.
      unlistenOutput = await listen<PtyOutputEvent>(`pty-output:${props.sessionId}`, (event) => {
        if (!terminal) {
          return;
        }
        const host = activeSessionRef.value.protocol === "ssh"
          ? activeSessionRef.value.sshConfig.host
          : undefined;
        const processed = outputRuleEngine.process(
          event.payload.data,
          settingsRef.value.outputRules,
          host,
        );
        terminal.write(processed.rendered, () => shellIntegration?.processOutput(event.payload.data));
        for (const match of processed.matches) {
          if (match.rule.bell) {
            terminal.write("\x07");
          }
          if (match.rule.notify && "Notification" in window && Notification.permission === "granted") {
            new Notification(`AuraTerm: ${match.rule.name}`, {
              body: `${host ? `${host}: ` : ""}${match.text}`,
            });
          }
          if (match.response && ptyId.value) {
            void writeSessionInput(
              ptyId.value,
              decodeControlCharacters(match.response),
              activeSessionRef.value,
            );
          }
        }
        logBuffer += event.payload.data;
        if (logBuffer.length > SAVED_LOG_MAX_CHARS + SAVED_LOG_TRIM_SLACK) {
          // Keep only the most recent window; drop the oldest output.
          logBuffer = logBuffer.slice(logBuffer.length - SAVED_LOG_MAX_CHARS);
        }
        if (actualLogPath.value) {
          const plain = stripAnsi(event.payload.data);
          queueLogChunk(actualLogPath.value, plain);
        }
      });

      unlistenZmodem = await listen<ZmodemTransferEvent>(`zmodem:${props.sessionId}`, (event) => {
        zmodemTransfer.value = event.payload;
        zmodemBusy.value = event.payload.status === "started" || event.payload.status === "progress";
        if (event.payload.status === "failed") {
          terminal?.writeln(`\r\n[Zmodem failed] ${event.payload.message ?? "Unknown error"}`);
        } else if (event.payload.status === "completed") {
          terminal?.writeln(`\r\n[Zmodem ${event.payload.direction} completed] ${event.payload.localPath ?? event.payload.fileName ?? ""}`);
          window.setTimeout(() => {
            if (zmodemTransfer.value?.status === "completed") zmodemTransfer.value = null;
          }, 4000);
        }
      });

      unlistenExit = await listen<PtyExitEvent>(`pty-exit:${props.sessionId}`, (event) => {
        if (!terminal) {
          return;
        }
        const message = event.payload.message;
        if (activeSessionRef.value.protocol === "serial") {
          notifySerialConnectionStateChange("closed");
        }
        if (activeSessionRef.value.protocol === "ssh"
          && isAuthError(message)
          && usesPasswordAuth(activeSessionRef.value.sshConfig)) {
          ptyId.value = null;
          writePasswordRetryPrompt(terminal);
          showRetryOverlay.value = true;
          return;
        }
        terminal.writeln(`\r\n[Session exited] ${message}`);
        if (activeSessionRef.value.protocol === "ssh") {
          const reconnectType = getSshReconnectType(activeSessionRef.value.sshConfig);
          if (reconnectType === "manual") {
            ptyId.value = null;
            manualReconnectPending.value = true;
            terminal.writeln("\r\n[Press r or R to reconnect]");
          }
        }
      });

      unlistenSshConnected = await listen<{ id: string }>(`ssh-connected:${props.sessionId}`, () => {
        terminal?.writeln("\r\n[Connected]");
        emit("sshConnected");
      });

      unlistenSerialConnected = await listen<{ id: string }>(`serial-connected:${props.sessionId}`, () => {
        notifySerialConnectionStateChange("connected");
        terminal?.writeln("\r\n[Connected]");
      });

      unlistenMfa = await listen<SshMfaPromptEvent>(`ssh-mfa-prompt:${props.sessionId}`, (event) => {
        mfaEvent.value = event.payload;
        mfaResponses.value = new Array(event.payload.prompts.length).fill("");
        showMfaOverlay.value = true;
      });

      unlistenReconnectPrompt = await listen<SshReconnectSessionPromptEvent>(`ssh-reconnect-session-prompt:${props.sessionId}`, (event) => {
        reconnectPrompt.value = event.payload;
        selectedReconnectSession.value = event.payload.sessions[0] ?? "";
      });

      unlistenHostKeyMismatch = await listen<SshHostKeyMismatchPromptEvent>(`ssh-host-key-mismatch-prompt:${props.sessionId}`, (event) => {
        hostKeyPrompt.value = event.payload;
        showHostKeyOverlay.value = true;
      });

      if (disposed || !terminal) {
        unlistenOutput?.();
        unlistenExit?.();
        unlistenMfa?.();
        unlistenSshConnected?.();
        unlistenReconnectPrompt?.();
        unlistenHostKeyMismatch?.();
        unlistenSerialConnected?.();
        unlistenZmodem?.();
        return;
      }

      const newId = props.sessionId;
  const cols = terminal.cols ?? 80;
  const rows = terminal.rows ?? 24;
      ptyId.value = newId;

      try {
        switch (session.protocol) {
          case "ssh":
            const reconnectType = getSshReconnectType(session.sshConfig);
            terminal.writeln(`Connecting to ${session.sshConfig.user}@${session.sshConfig.host}:${session.sshConfig.port}...`);
            terminal.writeln(`\x1b[33m[Reconnect mode: ${reconnectType}]\x1b[0m`);
            await startSshSession(newId, session.sshConfig, cols, rows);
            break;
          case "telnet":
            terminal.writeln(`Connecting to telnet://${session.telnetConfig.host}:${session.telnetConfig.port}...`);
            await startTelnetSession(newId, session.telnetConfig.host, session.telnetConfig.port, cols, rows);
            terminal.writeln("\r\n[Connected]");
            break;
          case "serial":
            notifySerialConnectionStateChange("connecting");
            terminal.writeln(`Opening serial port ${session.serialConfig.portName} @ ${session.serialConfig.baudRate}...`);
            await startSerialSession(newId, session.serialConfig);
            break;
          case "local":
            terminal.writeln("Starting local shell PTY...");
            // Use session cwd, or fall back to startup directory from command line
            const cwd = session.cwd ?? window.getStartupDir?.() ?? undefined;
            await startLocalSession(newId, cols, rows, cwd);
            break;
        }
      } catch (error) {
        if (disposed || !terminal) {
          return;
        }
        ptyId.value = null;
        const errorText = String(error);
        if (session.protocol === "serial") {
          notifySerialConnectionStateChange("error");
        }
        if (session.protocol === "ssh" && isAuthError(errorText) && usesPasswordAuth(session.sshConfig)) {
          writePasswordRetryPrompt(terminal);
          showRetryOverlay.value = true;
        } else {
          terminal.writeln(`\r\n[Failed to start session] ${errorText}`);
        }
        console.error("connect failed", error);
      }
    };

    void connect();
  }, { immediate: true, deep: true });
});

onBeforeUnmount(() => {
  flushPendingInput();
  flushPendingLog(true);
  stopSessionWatch?.();
  terminalCleanup?.();
});

function handlePasswordRetry(event: Event) {
  event.preventDefault();
  if (!activeSshConfig.value) {
    return;
  }
  const newConfig: SshConfig = { ...activeSshConfig.value, password: retryPassword.value };
  showRetryOverlay.value = false;
  retryPassword.value = "";
  activeSession.value = { protocol: "ssh", sshConfig: newConfig };
  emit("sessionUpdate", activeSession.value);
  void persistUpdatedSshPassword(newConfig);
  // overlay 卸载后把焦点交还给 xterm，避免用户必须再点一下终端才能输入
  void nextTick(() => terminal?.focus());
}

function handleMfaSubmit(event: Event) {
  event.preventDefault();
  if (!mfaEvent.value || !ptyId.value) {
    return;
  }
  void answerSshMfa(ptyId.value, mfaResponses.value);
  showMfaOverlay.value = false;
}

function updateMfaResponse(index: number, value: string) {
  const nextResponses = [...mfaResponses.value];
  nextResponses[index] = value;
  mfaResponses.value = nextResponses;
}

function inputValue(event: Event) {
  return (event.target as HTMLInputElement).value;
}

function handleCancelMfa() {
  showMfaOverlay.value = false;
  if (!ptyId.value) {
    return;
  }
  void answerSshMfa(ptyId.value, mfaEvent.value?.prompts.map(() => "") ?? []);
}

function submitReconnectChoice(sessionName: string | null) {
  const prompt = reconnectPrompt.value;
  reconnectPrompt.value = null;
  selectedReconnectSession.value = "";

  if (!prompt) {
    return;
  }

  void answerSshReconnectChoice(prompt.id, sessionName).catch((error) => {
    console.error("Failed to answer reconnect session prompt", error);
  });
}

function handleAttachSelectedReconnectSession() {
  submitReconnectChoice(selectedReconnectSession.value || null);
}

function handleSkipReconnectSession() {
  submitReconnectChoice(null);
}

function submitHostKeyDecision(trustNewFingerprint: boolean) {
  const prompt = hostKeyPrompt.value;
  hostKeyPrompt.value = null;
  showHostKeyOverlay.value = false;

  if (!prompt) {
    return;
  }

  void answerSshHostKeyMismatch(prompt.id, trustNewFingerprint).catch((error) => {
    console.error("Failed to answer host key mismatch prompt", error);
  });
}

function handleTrustHostKey() {
  submitHostKeyDecision(true);
}

function handleRejectHostKey() {
  submitHostKeyDecision(false);
}

function formatTransferBytes(value: number) {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  return `${(value / 1024 / 1024).toFixed(1)} MiB`;
}

const zmodemProgress = computed(() => {
  const transfer = zmodemTransfer.value;
  if (!transfer?.totalBytes) return null;
  return Math.min(100, Math.round((transfer.transferredBytes / transfer.totalBytes) * 100));
});

async function handleZmodemFileSelected(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0];
  if (!file || !ptyId.value) return;
  zmodemBusy.value = true;
  try {
    const data = Array.from(new Uint8Array(await file.arrayBuffer()));
    const wire = await startZmodemSend(ptyId.value, file.name, data);
    await writeSessionBytes(ptyId.value, wire, activeSessionRef.value);
  } catch (error) {
    zmodemTransfer.value = {
      id: props.sessionId,
      direction: "upload",
      status: "failed",
      transferredBytes: 0,
      message: String(error),
    };
    zmodemBusy.value = false;
  } finally {
    if (zmodemFileInput.value) zmodemFileInput.value.value = "";
  }
}

async function handleCancelZmodem() {
  if (!ptyId.value) return;
  try {
    const wire = await cancelZmodem(ptyId.value);
    await writeSessionBytes(ptyId.value, wire, activeSessionRef.value);
  } finally {
    zmodemTransfer.value = null;
    zmodemBusy.value = false;
  }
}
</script>

<template>
  <div
    :style="{
      position: 'relative',
      width: '100%',
      height: '100%',
      display: isVisible ? 'flex' : 'none',
      flexDirection: 'column',
      backgroundColor: effectiveSettings.theme.background,
    }"
  >
    <div ref="terminalRootRef" :style="{ flex: 1, minHeight: 0 }" />

    <div v-if="zmodemTransfer" class="zmodem-transfer-bar" :class="zmodemTransfer.status">
      <input ref="zmodemFileInput" type="file" hidden @change="handleZmodemFileSelected">
      <div class="zmodem-transfer-main">
        <strong>Zmodem {{ zmodemTransfer.direction }}</strong>
        <span>
          {{ zmodemTransfer.fileName || (zmodemTransfer.direction === 'upload' ? $t('terminal.zmodemWaitingFile') : $t('terminal.zmodemReceivingMeta')) }}
        </span>
        <span v-if="zmodemTransfer.status !== 'detected'">
          {{ formatTransferBytes(zmodemTransfer.transferredBytes) }}
          <template v-if="zmodemTransfer.totalBytes"> / {{ formatTransferBytes(zmodemTransfer.totalBytes) }}</template>
        </span>
      </div>
      <div v-if="zmodemProgress !== null" class="zmodem-progress-track">
        <div class="zmodem-progress-fill" :style="{ width: `${zmodemProgress}%` }" />
      </div>
      <button
        v-if="zmodemTransfer.direction === 'upload' && zmodemTransfer.status === 'detected'"
        type="button"
        @click="zmodemFileInput?.click()"
      >{{ $t('terminal.selectFile') }}</button>
      <button v-if="zmodemBusy || zmodemTransfer.status === 'detected'" type="button" @click="handleCancelZmodem">{{ $t('common.cancel') }}</button>
      <span v-if="zmodemTransfer.status === 'failed'" class="zmodem-error">{{ zmodemTransfer.message }}</span>
    </div>

    <div v-if="showRetryOverlay && activeSshConfig" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🔐</div>
        <h3 class="password-retry-title">{{ $t('terminal.authFailedTitle') }}</h3>
        <p class="password-retry-desc">
          {{ $t('terminal.authFailedDescPre') }}<strong>{{ activeSshConfig.user }}@{{ activeSshConfig.host }}</strong>{{ $t('terminal.authFailedDescPost') }}
        </p>
        <form class="password-retry-form" @submit="handlePasswordRetry">
          <input
            v-model="retryPassword"
            type="password"
            class="password-retry-input"
            :placeholder="$t('terminal.enterPassword')"
            autofocus
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            @keydown.stop
          >
          <div class="password-retry-actions">
            <button type="button" class="password-retry-btn cancel" @click="showRetryOverlay = false">{{ $t('common.cancel') }}</button>
            <button type="submit" class="password-retry-btn retry">{{ $t('terminal.retry') }}</button>
          </div>
        </form>
      </div>
    </div>

    <div v-if="showMfaOverlay && mfaEvent" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🛡️</div>
        <h3 class="password-retry-title">{{ mfaEvent.name || $t('terminal.mfaRequired') }}</h3>
        <p class="password-retry-desc">{{ mfaEvent.instruction }}</p>
        <form class="password-retry-form" @submit="handleMfaSubmit">
          <div v-for="(prompt, index) in mfaEvent.prompts" :key="`${index}-${prompt.text}`" style="margin-bottom: 10px">
            <label style="display: block; margin-bottom: 5px; font-size: 12px; opacity: 0.8">{{ prompt.text }}</label>
            <input
              :type="prompt.echo ? 'text' : 'password'"
              class="password-retry-input"
              :value="mfaResponses[index] || ''"
              :autofocus="index === 0"
              :placeholder="prompt.text.replace(/[:：]\s*$/, '')"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              @input="updateMfaResponse(index, inputValue($event))"
              @keydown.stop
            >
          </div>
          <div class="password-retry-actions">
            <button type="button" class="password-retry-btn cancel" @click="handleCancelMfa">{{ $t('common.cancel') }}</button>
            <button type="submit" class="password-retry-btn retry">{{ $t('terminal.verify') }}</button>
          </div>
        </form>
      </div>
    </div>

    <div v-if="reconnectPrompt" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🔁</div>
        <h3 class="password-retry-title">{{ $t('terminal.attachExisting', { tool: reconnectPrompt.tool }) }}</h3>
        <p class="password-retry-desc">
          {{ $t('terminal.reconnectDesc') }}
        </p>
        <form class="password-retry-form" @submit.prevent="handleAttachSelectedReconnectSession">
          <label style="display: block; margin-bottom: 6px; font-size: 12px; opacity: 0.85">
            {{ $t('terminal.selectSessionToAttach') }}
          </label>
          <select
            v-model="selectedReconnectSession"
            class="password-retry-input"
            style="appearance: auto"
            @keydown.stop
          >
            <option v-for="sessionName in reconnectPrompt.sessions" :key="sessionName" :value="sessionName">
              {{ sessionName.startsWith('at-') ? 'at-' : '' }}{{ sessionName.startsWith('at-') ? sessionName.substring(3) : sessionName }}
            </option>
          </select>
          <div class="password-retry-actions">
            <button type="button" class="password-retry-btn cancel" @click="handleSkipReconnectSession">{{ $t('terminal.createNew') }}</button>
            <button type="submit" class="password-retry-btn retry" :disabled="!selectedReconnectSession">{{ $t('terminal.attach') }}</button>
          </div>
        </form>
      </div>
    </div>

    <div v-if="showHostKeyOverlay && hostKeyPrompt" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">⚠️</div>
        <h3 class="password-retry-title">{{ $t('terminal.hostKeyChangedTitle') }}</h3>
        <p class="password-retry-desc">
          {{ $t('terminal.hostKeyChangedDescPre') }}<strong>{{ hostKeyPrompt.host }}:{{ hostKeyPrompt.port }}</strong>{{ $t('terminal.hostKeyChangedDescPost') }}
        </p>
        <div class="host-key-fingerprint-block">
          <div class="host-key-fingerprint-label">{{ $t('terminal.knownFingerprint') }}</div>
          <div class="host-key-fingerprint-value">{{ hostKeyPrompt.expectedFingerprint }}</div>
          <div class="host-key-fingerprint-label" style="margin-top: 8px;">{{ $t('terminal.presentedFingerprint') }}</div>
          <div class="host-key-fingerprint-value">{{ hostKeyPrompt.observedFingerprint }}</div>
        </div>
        <div class="password-retry-actions">
          <button type="button" class="password-retry-btn cancel" @click="handleRejectHostKey">{{ $t('common.cancel') }}</button>
          <button type="button" class="password-retry-btn retry" @click="handleTrustHostKey">{{ $t('terminal.trustAndContinue') }}</button>
        </div>
      </div>
    </div>

    <!-- Manual Reconnect Status Bar -->
    <div v-if="manualReconnectPending" class="reconnect-status-bar">
      <div class="reconnect-status-content">
        <span class="reconnect-status-icon">🔌</span>
        <span class="reconnect-status-text">{{ $t('terminal.disconnectedPre') }}<kbd>R</kbd>{{ $t('terminal.disconnectedPost') }}</span>
      </div>
      <button class="reconnect-status-btn" @click="reconnectSshSession">{{ $t('terminal.reconnectNow') }}</button>
    </div>
  </div>
</template>

<style scoped>
.reconnect-status-bar {
  position: absolute;
  bottom: 0;
  left: 0;
  right: 0;
  height: 36px;
  background: #3e3e42;
  border-top: 1px solid #555;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  z-index: 100;
  color: #fff;
  font-size: 13px;
  animation: slide-up 0.2s ease-out;
}

@keyframes slide-up {
  from { transform: translateY(100%); }
  to { transform: translateY(0); }
}

.reconnect-status-content {
  display: flex;
  align-items: center;
  gap: 8px;
}

.reconnect-status-icon {
  font-size: 16px;
}

.reconnect-status-text kbd {
  background: #252526;
  border: 1px solid #666;
  border-radius: 3px;
  padding: 1px 5px;
  font-family: inherit;
  font-size: 11px;
  box-shadow: 0 1px 0 rgba(0, 0, 0, 0.2);
  margin: 0 2px;
}

.reconnect-status-btn {
  background: #0078d4;
  color: white;
  border: none;
  border-radius: 4px;
  padding: 4px 12px;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.2s;
}

.reconnect-status-btn:hover {
  background: #0086ee;
}

.reconnect-status-btn:active {
  background: #006cc1;
}

.host-key-fingerprint-block {
  background: rgba(0, 0, 0, 0.28);
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 8px;
  padding: 10px;
  font-size: 12px;
  margin-top: 8px;
}

.host-key-fingerprint-label {
  opacity: 0.75;
  margin-bottom: 4px;
}

.host-key-fingerprint-value {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
  word-break: break-all;
}

.zmodem-transfer-bar {
  position: absolute;
  left: 12px;
  right: 12px;
  bottom: 12px;
  z-index: 120;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 12px;
  border: 1px solid var(--app-border);
  border-radius: 7px;
  background: var(--app-surface-2);
  color: var(--app-text);
  box-shadow: 0 6px 24px var(--app-shadow);
  font-size: 12px;
}

.zmodem-transfer-main {
  display: flex;
  align-items: center;
  gap: 9px;
  min-width: 0;
}

.zmodem-transfer-main span { color: var(--app-text-muted); }
.zmodem-progress-track { flex: 1; height: 4px; overflow: hidden; border-radius: 4px; background: var(--app-surface-3); }
.zmodem-progress-fill { height: 100%; background: var(--app-accent); transition: width 0.15s ease; }
.zmodem-transfer-bar button { border: 1px solid var(--app-border); border-radius: 5px; padding: 4px 8px; background: var(--app-surface-3); color: var(--app-text); cursor: pointer; }
.zmodem-error { color: var(--app-danger); }
</style>
