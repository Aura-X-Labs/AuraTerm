<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { type as getOsType } from "@tauri-apps/plugin-os";
import TerminalComponent from "./TerminalComponent.vue";
import ConnectDialog from "./ConnectDialog.vue";
import BookmarkSidebar from "./BookmarkSidebar.vue";
import SettingsDialog from "./SettingsDialog.vue";
import AboutDialog from "./AboutDialog.vue";
import TerminalInputBar from "./TerminalInputBar.vue";
import { DEFAULT_SETTINGS, type AppSettings, type QuickButton, type SerialHistoryItem } from "./settings";
import type {
  ConnectResult,
  ConnectionProtocol,
  SavedConnection,
  SerialConfig,
  SerialConnectionState,
  SessionConfig,
  TerminalHandle,
} from "./types";
import logoUrl from "./logo.png";
import "./App.css";

interface Tab {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}

type AppMenuId = "file" | "help";

let nextTabId = 1;

function formatSerialFrame(serialConfig: SerialConfig) {
  const parity = serialConfig.parity === "none" ? "N" : serialConfig.parity === "even" ? "E" : "O";
  return `${serialConfig.dataBits}${parity}${serialConfig.stopBits}`;
}

const tabs = ref<Tab[]>([{ id: "tab-0", title: "Local Shell", session: { protocol: "local" } }]);
const activeTabId = ref("tab-0");
const osType = ref("windows");
const showConnectDialog = ref(false);
const connectDialogProtocol = ref<ConnectionProtocol>("ssh");
const settings = ref<AppSettings>(DEFAULT_SETTINGS);
const showSettings = ref(false);
const showAbout = ref(false);
const sidebarOpen = ref(false);
const sidebarRefreshToken = ref(0);
const showNewTabMenu = ref(false);
const isWindowFocused = ref(true);
const serialConnectionStates = ref<Record<string, SerialConnectionState>>({});
const openMenuId = ref<AppMenuId | null>(null);
const settingsRef = ref<AppSettings>(DEFAULT_SETTINGS);
const menuBarRef = ref<HTMLDivElement | null>(null);
const termRefs = new Map<string, TerminalHandle>();
const cleanupFns: Array<() => void> = [];

watch(settings, (value) => {
  settingsRef.value = value;
}, { deep: true, immediate: true });

watch(openMenuId, (menuId, _previous, onCleanup) => {
  if (!menuId) {
    return;
  }

  const handlePointerDown = (event: PointerEvent) => {
    const target = event.target;
    if (!(target instanceof Node)) {
      return;
    }
    if (menuBarRef.value?.contains(target)) {
      return;
    }
    openMenuId.value = null;
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      openMenuId.value = null;
    }
  };

  document.addEventListener("pointerdown", handlePointerDown);
  window.addEventListener("keydown", handleKeyDown);

  onCleanup(() => {
    document.removeEventListener("pointerdown", handlePointerDown);
    window.removeEventListener("keydown", handleKeyDown);
  });
});

onMounted(async () => {
  try {
    cleanupFns.push(await listen("tauri://focus", () => {
      isWindowFocused.value = true;
    }));
    cleanupFns.push(await listen("tauri://blur", () => {
      isWindowFocused.value = false;
    }));
  } catch (error) {
    console.error("Failed to setup window focus listeners:", error);
  }

  try {
    osType.value = await getOsType();
  } catch (error) {
    console.error("Failed to detect OS:", error);
  }

  try {
    const loaded = await invoke<AppSettings>("get_settings");
    settings.value = { ...DEFAULT_SETTINGS, ...loaded };
  } catch {
    settings.value = DEFAULT_SETTINGS;
  }

  try {
    cleanupFns.push(await listen("show-about", () => {
      showAbout.value = true;
    }));
  } catch (error) {
    console.error("Failed to setup about listener:", error);
  }
});

onBeforeUnmount(() => {
  while (cleanupFns.length > 0) {
    const cleanup = cleanupFns.pop();
    cleanup?.();
  }
});

const activeTab = computed(() => tabs.value.find((tab) => tab.id === activeTabId.value));
const activeSerialConfig = computed<SerialConfig | null>(() => (
  activeTab.value?.session.protocol === "serial" ? activeTab.value.session.serialConfig : null
));
const activeSerialConnectionState = computed<SerialConnectionState | null>(() => {
  if (!activeTab.value || !activeSerialConfig.value) {
    return null;
  }
  return serialConnectionStates.value[activeTab.value.id] ?? "connecting";
});
const appClassName = computed(() => [
  "app-container",
  osType.value,
  isWindowFocused.value ? "focused" : "blurred",
]);

async function handleSaveSettings(newSettings: AppSettings) {
  await invoke("save_settings", { settings: newSettings }).catch(console.error);
  settings.value = newSettings;
  showSettings.value = false;
}

function persistSettingsSilently(newSettings: AppSettings) {
  settingsRef.value = newSettings;
  settings.value = newSettings;
  void invoke("save_settings", { settings: newSettings }).catch((error) => {
    console.error("save_settings failed", error);
  });
}

function rememberSerialConfig(serialConfig: SerialConfig) {
  const configKey = `${serialConfig.portName}|${serialConfig.baudRate}|${serialConfig.dataBits}|${serialConfig.stopBits}|${serialConfig.parity}|${serialConfig.flowControl}`;
  const historyItem: SerialHistoryItem = {
    id: crypto.randomUUID(),
    name: `${serialConfig.portName} · ${serialConfig.baudRate} ${formatSerialFrame(serialConfig)}`,
    portName: serialConfig.portName,
    baudRate: serialConfig.baudRate,
    dataBits: serialConfig.dataBits,
    stopBits: serialConfig.stopBits,
    parity: serialConfig.parity,
    flowControl: serialConfig.flowControl,
  };

  const current = settingsRef.value;
  const recentSerialConfigs = [
    historyItem,
    ...current.recentSerialConfigs.filter((item) => {
      const itemKey = `${item.portName}|${item.baudRate}|${item.dataBits}|${item.stopBits}|${item.parity}|${item.flowControl}`;
      return itemKey !== configKey;
    }),
  ].slice(0, 8);

  persistSettingsSilently({
    ...current,
    lastSerialConfig: historyItem,
    recentSerialConfigs,
  });
}

function sendToActiveTerminal(text: string) {
  const handle = termRefs.get(activeTabId.value);
  if (handle) {
    handle.sendData(text);
  }
}

async function handleButtonsChange(buttons: QuickButton[]) {
  const newSettings: AppSettings = { ...settings.value, quickButtons: buttons };
  await invoke("save_settings", { settings: newSettings }).catch(console.error);
  settings.value = newSettings;
}

function updateSerialConnectionState(tabId: string, state: SerialConnectionState) {
  if (serialConnectionStates.value[tabId] === state) {
    return;
  }
  serialConnectionStates.value = {
    ...serialConnectionStates.value,
    [tabId]: state,
  };
}

function setTerminalRef(tabId: string, instance: unknown) {
  const handle = instance as TerminalHandle | null;
  if (handle) {
    termRefs.set(tabId, handle);
    return;
  }
  termRefs.delete(tabId);
}

function selectTab(tabId: string) {
  activeTabId.value = tabId;
}

function handleTitlebarMouseDown(event: MouseEvent) {
  if (event.button !== 0) {
    return;
  }
  if ((event.target as HTMLElement).closest("[data-no-drag='true']")) {
    return;
  }
  void getCurrentWindow().startDragging().catch((error) => {
    console.error("startDragging failed", error);
  });
}

async function handleMinimize() {
  await getCurrentWindow().minimize().catch((error) => {
    console.error("minimize failed", error);
  });
}

async function handleToggleMaximize() {
  const appWindow = getCurrentWindow();
  const isMaximized = await appWindow.isMaximized().catch((error) => {
    console.error("isMaximized failed", error);
    return false;
  });
  if (isMaximized) {
    await appWindow.unmaximize().catch((error) => {
      console.error("unmaximize failed", error);
    });
    return;
  }
  await appWindow.maximize().catch((error) => {
    console.error("maximize failed", error);
  });
}

async function handleClose() {
  await getCurrentWindow().close().catch((error) => {
    console.error("close failed", error);
  });
}

async function handleExitApp() {
  openMenuId.value = null;
  await getCurrentWindow().close().catch((error) => {
    console.error("exit failed", error);
  });
}

function handleOpenAbout() {
  openMenuId.value = null;
  showAbout.value = true;
}

function toggleMenu(menuId: AppMenuId) {
  openMenuId.value = openMenuId.value === menuId ? null : menuId;
}

function stopDragPropagation(event: MouseEvent) {
  event.preventDefault();
  event.stopPropagation();
}

function handleNewLocalSession() {
  const newId = `tab-${nextTabId++}`;
  tabs.value = [...tabs.value, { id: newId, title: "Local Shell", session: { protocol: "local" } }];
  activeTabId.value = newId;
}

function openConnect(protocol: ConnectionProtocol) {
  connectDialogProtocol.value = protocol;
  showConnectDialog.value = true;
}

function handleCloseTab(id: string) {
  const previousTabs = tabs.value;
  const nextTabs = previousTabs.filter((tab) => tab.id !== id);

  if (activeTabId.value === id) {
    const index = previousTabs.findIndex((tab) => tab.id === id);
    activeTabId.value = nextTabs.length > 0 ? nextTabs[Math.max(0, index - 1)].id : "";
  }

  tabs.value = nextTabs;

  if (id in serialConnectionStates.value) {
    const nextStates = { ...serialConnectionStates.value };
    delete nextStates[id];
    serialConnectionStates.value = nextStates;
  }
}

async function handleConnectResult(result: ConnectResult) {
  const newId = `tab-${nextTabId++}`;
  const { protocol, sshConfig, telnetConfig, serialConfig, saveAs, saveGroup } = result;

  if (protocol === "ssh" && sshConfig) {
    tabs.value = [
      ...tabs.value,
      {
        id: newId,
        title: `${sshConfig.user}@${sshConfig.host}`,
        session: { protocol: "ssh", sshConfig },
        logPath: result.logPath,
      },
    ];
  } else if (protocol === "telnet" && telnetConfig) {
    tabs.value = [
      ...tabs.value,
      {
        id: newId,
        title: `telnet://${telnetConfig.host}:${telnetConfig.port}`,
        session: { protocol: "telnet", telnetConfig },
        logPath: result.logPath,
      },
    ];
  } else if (protocol === "serial" && serialConfig) {
    tabs.value = [
      ...tabs.value,
      {
        id: newId,
        title: `${serialConfig.portName} @ ${serialConfig.baudRate}`,
        session: { protocol: "serial", serialConfig },
        logPath: result.logPath,
      },
    ];
  }

  if (protocol === "serial" && serialConfig) {
    rememberSerialConfig(serialConfig);
    updateSerialConnectionState(newId, "connecting");
  }

  activeTabId.value = newId;
  showConnectDialog.value = false;

  if (!saveAs) {
    return;
  }

  const connection: SavedConnection = {
    id: crypto.randomUUID(),
    name: saveAs,
    group: saveGroup,
    protocol,
    host: protocol === "ssh" ? sshConfig!.host : protocol === "telnet" ? telnetConfig!.host : "",
    port: protocol === "ssh" ? sshConfig!.port : protocol === "telnet" ? telnetConfig!.port : 0,
    user: protocol === "ssh" ? sshConfig!.user : "",
    authType: protocol === "ssh" ? (sshConfig!.privateKey ? "key" : "password") : "none",
    password: protocol === "ssh" ? sshConfig!.password : undefined,
    privateKey: protocol === "ssh" ? sshConfig!.privateKey : undefined,
    portName: protocol === "serial" ? serialConfig!.portName : undefined,
    baudRate: protocol === "serial" ? serialConfig!.baudRate : undefined,
    dataBits: protocol === "serial" ? serialConfig!.dataBits : undefined,
    stopBits: protocol === "serial" ? serialConfig!.stopBits : undefined,
    parity: protocol === "serial" ? serialConfig!.parity : undefined,
    flowControl: protocol === "serial" ? serialConfig!.flowControl : undefined,
    createdAt: Date.now(),
  };

  try {
    await invoke("save_connection", { connection });
    sidebarRefreshToken.value += 1;
    sidebarOpen.value = true;
  } catch (error) {
    console.error("Failed to save connection", error);
  }
}

function handleBookmarkConnect(connection: SavedConnection) {
  const newId = `tab-${nextTabId++}`;
  const protocol = connection.protocol ?? "ssh";

  let tab: Tab;
  if (protocol === "serial" && connection.portName && connection.baudRate) {
    const serialConfig: SerialConfig = {
      portName: connection.portName,
      baudRate: connection.baudRate,
      dataBits: connection.dataBits ?? 8,
      stopBits: connection.stopBits ?? 1,
      parity: connection.parity ?? "none",
      flowControl: connection.flowControl ?? "none",
    };
    rememberSerialConfig(serialConfig);
    tab = {
      id: newId,
      title: `${connection.portName} @ ${connection.baudRate}`,
      session: { protocol: "serial", serialConfig },
    };
  } else if (protocol === "telnet") {
    tab = {
      id: newId,
      title: `telnet://${connection.host}:${connection.port}`,
      session: {
        protocol: "telnet",
        telnetConfig: {
          host: connection.host,
          port: connection.port,
        },
      },
    };
  } else {
    tab = {
      id: newId,
      title: `${connection.user}@${connection.host}`,
      session: {
        protocol: "ssh",
        sshConfig: {
          host: connection.host,
          port: connection.port,
          user: connection.user,
          password: connection.password,
          privateKey: connection.authType === "key" ? connection.privateKey : undefined,
        },
      },
    };
  }

  if (tab.session.protocol === "serial") {
    updateSerialConnectionState(newId, "connecting");
  }

  tabs.value = [...tabs.value, tab];
  activeTabId.value = newId;
}
</script>

<template>
  <div :class="appClassName">
    <div class="titlebar" @mousedown="handleTitlebarMouseDown">
      <div v-if="osType !== 'windows'" class="titlebar-controls" aria-label="Window controls" data-no-drag="true">
        <button
          class="titlebar-control-btn titlebar-control-close"
          type="button"
          aria-label="Close"
          @mousedown="stopDragPropagation"
          @click="handleClose"
        />
        <button
          class="titlebar-control-btn titlebar-control-minimize"
          type="button"
          aria-label="Minimize"
          @mousedown="stopDragPropagation"
          @click="handleMinimize"
        />
        <button
          class="titlebar-control-btn titlebar-control-maximize"
          type="button"
          aria-label="Maximize"
          @mousedown="stopDragPropagation"
          @click="handleToggleMaximize"
        />
      </div>

      <div v-if="osType === 'windows'" class="titlebar-windows-main">
        <div class="titlebar-logo" data-no-drag="true">
          <img :src="logoUrl" alt="logo">
        </div>
        <div ref="menuBarRef" class="titlebar-menubar" data-no-drag="true">
          <div class="titlebar-menu-group" @mouseenter="openMenuId ? (openMenuId = 'file') : null">
            <button
              class="titlebar-menu-btn"
              :class="{ open: openMenuId === 'file' }"
              type="button"
              @mousedown="stopDragPropagation"
              @click="toggleMenu('file')"
            >
              File
            </button>
            <div v-if="openMenuId === 'file'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" @click="handleExitApp">
                <span>Exit</span>
                <span class="titlebar-menu-item-hint">Alt+F4</span>
              </button>
            </div>
          </div>

          <div class="titlebar-menu-group" @mouseenter="openMenuId ? (openMenuId = 'help') : null">
            <button
              class="titlebar-menu-btn"
              :class="{ open: openMenuId === 'help' }"
              type="button"
              @mousedown="stopDragPropagation"
              @click="toggleMenu('help')"
            >
              Help
            </button>
            <div v-if="openMenuId === 'help'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" @click="handleOpenAbout">
                <span>About AuraTerm</span>
              </button>
            </div>
          </div>
        </div>
        <div class="titlebar-title center">AuraTerm</div>
      </div>

      <div v-else class="titlebar-title">AuraTerm</div>

      <div v-if="osType === 'windows'" class="titlebar-controls-win" aria-label="Window controls" data-no-drag="true">
        <button
          class="titlebar-control-win-btn"
          type="button"
          aria-label="Minimize"
          @mousedown="stopDragPropagation"
          @click="handleMinimize"
        >
          &#xE921;
        </button>
        <button
          class="titlebar-control-win-btn"
          type="button"
          aria-label="Maximize"
          @mousedown="stopDragPropagation"
          @click="handleToggleMaximize"
        >
          &#xE922;
        </button>
        <button
          class="titlebar-control-win-btn close"
          type="button"
          aria-label="Close"
          @mousedown="stopDragPropagation"
          @click="handleClose"
        >
          &#xE8BB;
        </button>
      </div>
    </div>

    <div class="tab-bar">
      <button
        class="tab-new-btn bookmark-toggle-btn"
        :class="{ active: sidebarOpen }"
        title="Bookmarks"
        style="margin-right: 4px"
        @click="sidebarOpen = !sidebarOpen"
      >
        🔖
      </button>

      <div
        v-for="tab in tabs"
        :key="tab.id"
        class="tab-item"
        :class="{ active: activeTabId === tab.id }"
        @click="selectTab(tab.id)"
      >
        <span class="tab-title">{{ tab.title }}</span>
        <button class="tab-close-btn" title="Close Tab" @click.stop="handleCloseTab(tab.id)">×</button>
      </div>

      <button class="tab-new-btn" title="New Tab" @click="showNewTabMenu = true">+</button>
      <button class="tab-new-btn" title="Settings" style="margin-left: auto" @click="showSettings = true">&#x2699;</button>
    </div>

    <div class="workspace">
      <BookmarkSidebar v-if="sidebarOpen" :refresh-token="sidebarRefreshToken" @connect="handleBookmarkConnect" />

      <div class="terminal-wrapper">
        <div class="terminal-container">
          <div
            v-if="tabs.length === 0"
            style="display: flex; justify-content: center; align-items: center; height: 100%; color: #666"
          >
            No open tabs. Click + to open a new tab.
          </div>
          <TerminalComponent
            v-for="tab in tabs"
            v-else
            :key="tab.id"
            :ref="(instance) => setTerminalRef(tab.id, instance)"
            :is-active="activeTabId === tab.id"
            :session="tab.session"
            :log-path="tab.logPath"
            :settings="settings"
            @serial-connection-state-change="(state) => updateSerialConnectionState(tab.id, state)"
          />
        </div>

        <TerminalInputBar
          :quick-buttons="settings.quickButtons"
          @send="sendToActiveTerminal"
          @buttons-change="handleButtonsChange"
        />

        <div v-if="activeTab && activeSerialConfig && activeSerialConnectionState" class="terminal-statusbar">
          <div class="terminal-statusbar-left">
            <span class="terminal-status-indicator" :class="activeSerialConnectionState" />
            <span>{{ activeSerialConfig.portName }}</span>
            <span class="terminal-status-pill">{{ activeSerialConnectionState }}</span>
          </div>
          <div class="terminal-statusbar-right">
            <span>{{ activeSerialConfig.baudRate }} baud</span>
            <span>{{ formatSerialFrame(activeSerialConfig) }}</span>
            <span>{{ activeSerialConfig.flowControl }}</span>
          </div>
        </div>
      </div>
    </div>

    <SettingsDialog v-if="showSettings" :initial="settings" @save="handleSaveSettings" @cancel="showSettings = false" />
    <AboutDialog v-if="showAbout" @close="showAbout = false" />

    <div v-if="showNewTabMenu" class="newtab-overlay" @click="showNewTabMenu = false">
      <div class="newtab-dialog" @click.stop>
        <div class="newtab-dialog-title">New Session</div>
        <div class="newtab-options">
          <button class="newtab-option-btn" @click="showNewTabMenu = false; handleNewLocalSession()">
            <span class="newtab-option-icon">🖥</span>
            <span class="newtab-option-label">Local Shell</span>
            <span class="newtab-option-desc">Open a local terminal session</span>
          </button>
          <button class="newtab-option-btn" @click="showNewTabMenu = false; openConnect('ssh')">
            <span class="newtab-option-icon">🔗</span>
            <span class="newtab-option-label">SSH</span>
            <span class="newtab-option-desc">Connect to a remote shell over SSH</span>
          </button>
          <button class="newtab-option-btn" @click="showNewTabMenu = false; openConnect('telnet')">
            <span class="newtab-option-icon">🌐</span>
            <span class="newtab-option-label">Telnet</span>
            <span class="newtab-option-desc">Open a TCP terminal session</span>
          </button>
          <button class="newtab-option-btn" @click="showNewTabMenu = false; openConnect('serial')">
            <span class="newtab-option-icon">🔌</span>
            <span class="newtab-option-label">Serial</span>
            <span class="newtab-option-desc">Enumerate and connect to a serial device</span>
          </button>
        </div>
      </div>
    </div>

    <ConnectDialog
      v-if="showConnectDialog"
      :initial-protocol="connectDialogProtocol"
      :last-serial-config="settings.lastSerialConfig"
      :recent-serial-configs="settings.recentSerialConfigs"
      @connect="handleConnectResult"
      @cancel="showConnectDialog = false"
    />
  </div>
</template>