<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { type as getOsType } from "@tauri-apps/plugin-os";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import { isReconnectEnabled, normalizeReconnectType } from "./types";
import type { ReconnectType, SavedConnection, SerialConnectionState, SessionConfig, SshConfig, TerminalHandle } from "./types";
import "xterm/css/xterm.css";

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
}>();

const emit = defineEmits<{
  serialConnectionStateChange: [state: SerialConnectionState];
  sessionUpdate: [session: SessionConfig];
  sshPasswordUpdated: [];
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

const effectiveSettings = computed(() => props.settings ?? DEFAULT_SETTINGS);
const settingsRef = ref<AppSettings>(effectiveSettings.value);
const terminalRootRef = ref<HTMLDivElement | null>(null);
const ptyId = ref<string | null>(null);
const osType = ref("unknown");
const logBuffer = ref("");
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
const manualReconnectPending = ref(false);
const activeSshConfig = computed(() => (
  activeSession.value.protocol === "ssh" ? activeSession.value.sshConfig : undefined
));

let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let terminalCleanup: (() => void) | null = null;
let stopSessionWatch: (() => void) | null = null;
let pendingInputBuffer = "";
let pendingInputTimer: number | null = null;

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
  manualReconnectPending.value = false;
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

watch(() => props.isVisible, (isVisible) => {
  if (!isVisible) {
    return;
  }

  setTimeout(() => {
    fitAddon?.fit();
    if (props.isFocused) {
      terminal?.focus();
    }
  }, 0);
});

function notifySerialConnectionStateChange(state: SerialConnectionState) {
  if (activeSessionRef.value.protocol === "serial") {
    emit("serialConnectionStateChange", state);
  }
}

async function persistUpdatedSshPassword(sshConfig: SshConfig) {
  if (!sshConfig.savedConnectionId) {
    return;
  }

  try {
    const connections = await invoke<SavedConnection[]>("get_connections");
    const existing = connections.find((connection) => connection.id === sshConfig.savedConnectionId);
    if (!existing) {
      return;
    }

    await invoke("save_connection", {
      connection: {
        ...existing,
        password: sshConfig.password,
      },
    });
    emit("sshPasswordUpdated");
  } catch (error) {
    console.error("Failed to persist updated SSH password", error);
  }
}

async function saveLog(tabTitle: string) {
  const plain = stripAnsi(logBuffer.value);
  return invoke<string>("save_terminal_log", {
    content: plain,
    tabName: tabTitle,
  });
}

function sendData(text: string) {
  if (!ptyId.value) {
    return;
  }
  const data = text.endsWith("\n") ? text : `${text}\n`;
  queueTerminalInput(data);
}

function fit() {
  fitAddon?.fit();
}

function focus() {
  terminal?.focus();
}

function getSshReconnectType(sshConfig: SshConfig): ReconnectType {
  return normalizeReconnectType(sshConfig);
}

async function startSshSession(sessionId: string, sshConfig: SshConfig) {
  if (!terminal) {
    return;
  }

  const reconnectType = getSshReconnectType(sshConfig);
  const cols = terminal.cols ?? 80;
  const rows = terminal.rows ?? 24;

  ptyId.value = sessionId;
  await invoke("start_ssh_pty", {
    id: sessionId,
    host: sshConfig.host,
    port: sshConfig.port,
    user: sshConfig.user,
    password: sshConfig.password ?? null,
    privateKey: sshConfig.privateKey ?? null,
    cols,
    rows,
    autoReconnect: isReconnectEnabled(reconnectType),
    reconnectType,
  });
}

async function reconnectSshSession() {
  if (activeSessionRef.value.protocol !== "ssh" || !terminal) {
    return;
  }

  manualReconnectPending.value = false;
  terminal.writeln("\r\n[Reconnecting...]");

  try {
    await startSshSession(props.sessionId, activeSessionRef.value.sshConfig);
  } catch (error) {
    const errorText = String(error);
    ptyId.value = null;
    if (isAuthError(errorText)) {
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
  fit,
  focus,
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
  terminal.loadAddon(fitAddon);
  terminal.open(terminalRootRef.value);
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
    queueTerminalInput(normalizedData);
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
    terminal?.dispose();
    terminal = null;
    fitAddon = null;
  };

  const sessionKey = computed(() => {
    const s = activeSession.value;
    switch (s.protocol) {
      case "local":
        return `local:${s.cwd || ""}`;
      case "ssh":
        // Include password/key in key to restart if auth changes
        return `ssh:${s.sshConfig.user}@${s.sshConfig.host}:${s.sshConfig.port}:${s.sshConfig.password || ""}:${s.sshConfig.privateKey || ""}`;
      case "telnet":
        return `telnet:${s.telnetConfig.host}:${s.telnetConfig.port}`;
      case "serial":
        return `serial:${s.serialConfig.portName}:${s.serialConfig.baudRate}:${s.serialConfig.dataBits}:${s.serialConfig.stopBits}:${s.serialConfig.parity}:${s.serialConfig.flowControl}`;
      default:
        return "unknown";
    }
  });

  stopSessionWatch = watch(sessionKey, (newKey, oldKey, onCleanup) => {
    console.log(`[Terminal] Session key changed: ${oldKey} -> ${newKey}`);
    const session = activeSession.value;
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let unlistenMfa: UnlistenFn | null = null;
    let unlistenSshConnected: UnlistenFn | null = null;
    let unlistenReconnectPrompt: UnlistenFn | null = null;
    let unlistenSerialConnected: UnlistenFn | null = null;

    const cleanup = () => {
      console.log(`[Terminal] Cleaning up session: ${newKey}`);
      disposed = true;
      unlistenOutput?.();
      unlistenExit?.();
      unlistenMfa?.();
      unlistenSshConnected?.();
      unlistenReconnectPrompt?.();
      unlistenSerialConnected?.();
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

      console.log(`[Terminal] Starting new session: ${newKey}`);
      logBuffer.value = "";
      if (logPathRef.value) {
        const trimmedPath = logPathRef.value.trim();
        const resolvedPath = resolveLogPathPlaceholders(trimmedPath);
        actualLogPath.value = resolvedPath.endsWith(".log") ? resolvedPath : `${resolvedPath}.log`;
      } else {
        actualLogPath.value = undefined;
      }


      unlistenOutput = await listen<PtyOutputEvent>("pty-output", (event) => {
        if (event.payload.id !== ptyId.value || !terminal) {
          return;
        }
        terminal.write(event.payload.data);
        logBuffer.value += event.payload.data;
        if (actualLogPath.value) {
          const plain = stripAnsi(event.payload.data);
          void invoke("append_to_log", {
            path: actualLogPath.value,
            content: plain,
          }).catch((error) => {
            console.error("append_to_log failed", error);
          });
        }
      });

      unlistenExit = await listen<PtyExitEvent>("pty-exit", (event) => {
        if (event.payload.id !== ptyId.value || !terminal) {
          return;
        }
        const message = event.payload.message;
        if (activeSessionRef.value.protocol === "serial") {
          notifySerialConnectionStateChange("closed");
        }
        if (activeSessionRef.value.protocol === "ssh" && isAuthError(message)) {
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

      unlistenSshConnected = await listen<{ id: string }>("ssh-connected", (event) => {
        if (event.payload.id === ptyId.value) {
          terminal?.writeln("\r\n[Connected]");
        }
      });

      unlistenSerialConnected = await listen<{ id: string }>("serial-connected", (event) => {
        if (event.payload.id === ptyId.value) {
          notifySerialConnectionStateChange("connected");
          terminal?.writeln("\r\n[Connected]");
        }
      });

      unlistenMfa = await listen<SshMfaPromptEvent>("ssh-mfa-prompt", (event) => {
        if (event.payload.id !== ptyId.value) {
          return;
        }
        mfaEvent.value = event.payload;
        mfaResponses.value = new Array(event.payload.prompts.length).fill("");
        showMfaOverlay.value = true;
      });

      unlistenReconnectPrompt = await listen<SshReconnectSessionPromptEvent>("ssh-reconnect-session-prompt", (event) => {
        if (event.payload.id !== ptyId.value) {
          return;
        }

        reconnectPrompt.value = event.payload;
        selectedReconnectSession.value = event.payload.sessions[0] ?? "";
      });

      if (disposed || !terminal) {
        unlistenOutput?.();
        unlistenExit?.();
        unlistenMfa?.();
        unlistenSshConnected?.();
        unlistenReconnectPrompt?.();
        unlistenSerialConnected?.();
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
            await startSshSession(newId, session.sshConfig);
            break;
          case "telnet":
            terminal.writeln(`Connecting to telnet://${session.telnetConfig.host}:${session.telnetConfig.port}...`);
            await invoke("start_telnet_session", {
              id: newId,
              host: session.telnetConfig.host,
              port: session.telnetConfig.port,
            });
            terminal.writeln("\r\n[Connected]");
            break;
          case "serial":
            notifySerialConnectionStateChange("connecting");
            terminal.writeln(`Opening serial port ${session.serialConfig.portName} @ ${session.serialConfig.baudRate}...`);
            await invoke("start_serial_session", {
              id: newId,
              portName: session.serialConfig.portName,
              baudRate: session.serialConfig.baudRate,
              dataBits: session.serialConfig.dataBits,
              stopBits: session.serialConfig.stopBits,
              parity: session.serialConfig.parity,
              flowControl: session.serialConfig.flowControl,
            });
            break;
          case "local":
            terminal.writeln("Starting local shell PTY...");
            // Use session cwd, or fall back to startup directory from command line
            const cwd = session.cwd ?? window.getStartupDir?.() ?? undefined;
            await invoke("start_pty", { id: newId, cols, rows, cwd });
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
        if (session.protocol === "ssh" && isAuthError(errorText)) {
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
}

function handleMfaSubmit(event: Event) {
  event.preventDefault();
  if (!mfaEvent.value || !ptyId.value) {
    return;
  }
  void invoke("answer_ssh_mfa", {
    id: ptyId.value,
    responses: mfaResponses.value,
  });
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
  void invoke("answer_ssh_mfa", {
    id: ptyId.value,
    responses: mfaEvent.value?.prompts.map(() => "") ?? [],
  });
}

function submitReconnectChoice(sessionName: string | null) {
  const prompt = reconnectPrompt.value;
  reconnectPrompt.value = null;
  selectedReconnectSession.value = "";

  if (!prompt) {
    return;
  }

  void invoke("answer_ssh_reconnect_choice", {
    id: prompt.id,
    sessionName,
  }).catch((error) => {
    console.error("Failed to answer reconnect session prompt", error);
  });
}

function handleAttachSelectedReconnectSession() {
  submitReconnectChoice(selectedReconnectSession.value || null);
}

function handleSkipReconnectSession() {
  submitReconnectChoice(null);
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

    <div v-if="showRetryOverlay && activeSshConfig" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🔐</div>
        <h3 class="password-retry-title">Authentication Failed</h3>
        <p class="password-retry-desc">
          Could not connect to <strong>{{ activeSshConfig.user }}@{{ activeSshConfig.host }}</strong>.
          Incorrect password, please try again.
        </p>
        <form class="password-retry-form" @submit="handlePasswordRetry">
          <input
            v-model="retryPassword"
            type="password"
            class="password-retry-input"
            placeholder="Enter password"
            autofocus
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            @keydown.stop
          >
          <div class="password-retry-actions">
            <button type="button" class="password-retry-btn cancel" @click="showRetryOverlay = false">Cancel</button>
            <button type="submit" class="password-retry-btn retry">Retry</button>
          </div>
        </form>
      </div>
    </div>

    <div v-if="showMfaOverlay && mfaEvent" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🛡️</div>
        <h3 class="password-retry-title">{{ mfaEvent.name || 'MFA Required' }}</h3>
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
            <button type="button" class="password-retry-btn cancel" @click="handleCancelMfa">Cancel</button>
            <button type="submit" class="password-retry-btn retry">Verify</button>
          </div>
        </form>
      </div>
    </div>

    <div v-if="reconnectPrompt" class="password-retry-overlay">
      <div class="password-retry-dialog">
        <div class="password-retry-icon">🔁</div>
        <h3 class="password-retry-title">Attach Existing {{ reconnectPrompt.tool }}</h3>
        <p class="password-retry-desc">
          AuraTerm found detached remote sessions created by AuraTerm.
          Only sessions with the <strong>at-</strong> prefix are listed below.
        </p>
        <form class="password-retry-form" @submit.prevent="handleAttachSelectedReconnectSession">
          <label style="display: block; margin-bottom: 6px; font-size: 12px; opacity: 0.85">
            Select a session to attach:
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
            <button type="button" class="password-retry-btn cancel" @click="handleSkipReconnectSession">Create New</button>
            <button type="submit" class="password-retry-btn retry" :disabled="!selectedReconnectSession">Attach</button>
          </div>
        </form>
      </div>
    </div>

    <!-- Manual Reconnect Status Bar -->
    <div v-if="manualReconnectPending" class="reconnect-status-bar">
      <div class="reconnect-status-content">
        <span class="reconnect-status-icon">🔌</span>
        <span class="reconnect-status-text">连接已断开。按 <kbd>R</kbd> 重新连接。</span>
      </div>
      <button class="reconnect-status-btn" @click="reconnectSshSession">立即重连</button>
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
</style>
