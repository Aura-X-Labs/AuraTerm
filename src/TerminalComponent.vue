<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { DEFAULT_SETTINGS, type AppSettings } from "./settings";
import type { SerialConnectionState, SessionConfig, SshConfig, TerminalHandle } from "./types";
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

const props = defineProps<{
  isActive: boolean;
  session: SessionConfig;
  logPath?: string;
  settings?: AppSettings;
}>();

const emit = defineEmits<{
  serialConnectionStateChange: [state: SerialConnectionState];
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
const activeSshConfig = computed(() => (
  activeSession.value.protocol === "ssh" ? activeSession.value.sshConfig : undefined
));

let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let terminalCleanup: (() => void) | null = null;
let stopSessionWatch: (() => void) | null = null;

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
  terminal.options.theme = {
    background: value.theme.background,
    foreground: value.theme.foreground,
    cursor: value.theme.cursor,
  };
  fitAddon?.fit();
}, { deep: true });

watch(() => props.isActive, (isActive) => {
  if (!isActive) {
    return;
  }
  setTimeout(() => {
    fitAddon?.fit();
  }, 0);
});

function notifySerialConnectionStateChange(state: SerialConnectionState) {
  if (activeSessionRef.value.protocol === "serial") {
    emit("serialConnectionStateChange", state);
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
  const id = ptyId.value;
  const data = text.endsWith("\n") ? text : `${text}\n`;
  void writeSessionInput(id, data, activeSessionRef.value).catch((error) => {
    console.error("sendData failed", error);
  });
}

defineExpose<TerminalHandle>({
  saveLog,
  sendData,
});

onMounted(() => {
  if (!terminalRootRef.value) {
    return;
  }

  terminal = new Terminal({
    cursorBlink: true,
    cursorStyle: "block",
    fontFamily: effectiveSettings.value.fontFamily,
    fontSize: effectiveSettings.value.fontSize,
    scrollback: effectiveSettings.value.scrollback,
    theme: {
      background: effectiveSettings.value.theme.background,
      foreground: effectiveSettings.value.theme.foreground,
      cursor: effectiveSettings.value.theme.cursor,
    },
    convertEol: true,
    allowProposedApi: true,
    macOptionIsMeta: true,
  });

  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(terminalRootRef.value);
  fitAddon.fit();

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
      void navigator.clipboard.readText().then((text) => {
        if (!text || !ptyId.value) {
          return;
        }
        void writeSessionInput(ptyId.value, text, activeSessionRef.value).catch((error) => {
          console.error("paste failed", error);
        });
      }).catch((error) => {
        console.error("clipboard read failed", error);
      });
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
    if (!ptyId.value) {
      return;
    }
    void writeSessionInput(ptyId.value, data, activeSessionRef.value).catch((error) => {
      console.error("input failed", error);
    });
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

  stopSessionWatch = watch(activeSession, (session, _previous, onCleanup) => {
    let disposed = false;
    let unlistenOutput: UnlistenFn | null = null;
    let unlistenExit: UnlistenFn | null = null;
    let unlistenMfa: UnlistenFn | null = null;
    let unlistenSshConnected: UnlistenFn | null = null;
    let unlistenSerialConnected: UnlistenFn | null = null;

    const cleanup = () => {
      disposed = true;
      unlistenOutput?.();
      unlistenExit?.();
      unlistenMfa?.();
      unlistenSshConnected?.();
      unlistenSerialConnected?.();
      if (ptyId.value) {
        const id = ptyId.value;
        ptyId.value = null;
        void closeSession(id, session).catch(() => {});
      }
    };

    onCleanup(cleanup);

    const connect = async () => {
      if (!terminal) {
        return;
      }

      logBuffer.value = "";
      if (logPathRef.value) {
        const now = new Date();
        const pad = (value: number) => String(value).padStart(2, "0");
        const timestamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}_${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
        actualLogPath.value = `${logPathRef.value}_${timestamp}.log`;
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
        terminal.writeln(`\r\n[Session exited] ${message}`);
        if (activeSessionRef.value.protocol === "serial") {
          notifySerialConnectionStateChange("closed");
        }
        if (activeSessionRef.value.protocol === "ssh" && isAuthError(message)) {
          ptyId.value = null;
          showRetryOverlay.value = true;
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

      if (disposed || !terminal) {
        unlistenOutput?.();
        unlistenExit?.();
        unlistenMfa?.();
        unlistenSshConnected?.();
        unlistenSerialConnected?.();
        return;
      }

      const cols = terminal.cols ?? 80;
      const rows = terminal.rows ?? 24;
      const newId = crypto.randomUUID();
      ptyId.value = newId;

      try {
        switch (session.protocol) {
          case "ssh":
            terminal.writeln(`Connecting to ${session.sshConfig.user}@${session.sshConfig.host}:${session.sshConfig.port}...`);
            await invoke("start_ssh_pty", {
              id: newId,
              host: session.sshConfig.host,
              port: session.sshConfig.port,
              user: session.sshConfig.user,
              password: session.sshConfig.password ?? null,
              privateKey: session.sshConfig.privateKey ?? null,
              cols,
              rows,
            });
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
            await invoke("start_pty", { id: newId, cols, rows });
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
        terminal.writeln(`\r\n[Failed to start session] ${errorText}`);
        console.error("connect failed", error);
      }
    };

    void connect();
  }, { immediate: true, deep: true });
});

onBeforeUnmount(() => {
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
</script>

<template>
  <div
    :style="{
      position: 'relative',
      width: '100%',
      height: '100%',
      display: isActive ? 'flex' : 'none',
      flexDirection: 'column',
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
  </div>
</template>