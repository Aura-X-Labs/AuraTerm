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
import RemoteFileManager from "./RemoteFileManager.vue";
import {
  DEFAULT_SETTINGS,
  MAX_TERMINAL_FONT_SIZE,
  MIN_TERMINAL_FONT_SIZE,
  normalizeAppSettings,
  type AppSettings,
  type QuickButton,
  type SerialHistoryItem,
} from "./settings";
import { isReconnectEnabled, normalizeReconnectType } from "./types";
import type {
  ConnectResult,
  ConnectionProtocol,
  SavedConnection,
  SerialConfig,
  SerialConnectionState,
  SessionConfig,
  SshConfig,
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

type AppMenuId = "file" | "view" | "help";
type FileSubmenuId = "new-session" | "preferences";

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
const showRemoteFileManager = ref(false);
const isWindowFocused = ref(true);
const serialConnectionStates = ref<Record<string, SerialConnectionState>>({});
const openMenuId = ref<AppMenuId | null>(null);
const openFileSubmenuId = ref<FileSubmenuId | null>(null);
const draggedTabId = ref<string | null>(null);
const dragPreviewTabId = ref<string | null>(null);
const suppressTabClick = ref(false);
let pendingTabDrag: { tabId: string; startX: number; startY: number } | null = null;
let tabDragMoved = false;
const settingsRef = ref<AppSettings>(DEFAULT_SETTINGS);
const menuBarRef = ref<HTMLDivElement | null>(null);
const isFullscreen = ref(false);
const termRefs = new Map<string, TerminalHandle>();
const cleanupFns: Array<() => void> = [];

function closeOpenMenus() {
  openMenuId.value = null;
  openFileSubmenuId.value = null;
}

watch(settings, (value) => {
  settingsRef.value = value;
}, { deep: true, immediate: true });

watch(openMenuId, (menuId, _previous, onCleanup) => {
  if (menuId !== "file") {
    openFileSubmenuId.value = null;
  }

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

async function syncFullscreenState() {
  isFullscreen.value = await getCurrentWindow().isFullscreen().catch((error) => {
    console.error("isFullscreen failed", error);
    return false;
  });
}

function handleGlobalKeyDown(event: KeyboardEvent) {
  const hasPrimaryModifier = event.ctrlKey || event.metaKey;
  if (hasPrimaryModifier && !event.altKey) {
    const key = event.key;
    const isIncreaseShortcut = key === "+" || key === "=" || event.code === "NumpadAdd";
    const isDecreaseShortcut = key === "-" || key === "_" || event.code === "NumpadSubtract";
    const isResetShortcut = key === "0" || event.code === "Numpad0";

    if (isIncreaseShortcut) {
      event.preventDefault();
      handleIncreaseTerminalFontSize();
      return;
    }

    if (isDecreaseShortcut) {
      event.preventDefault();
      handleDecreaseTerminalFontSize();
      return;
    }

    if (isResetShortcut) {
      event.preventDefault();
      handleResetTerminalFontSize();
      return;
    }
  }

  const isFullscreenShortcut = event.key === "F11";
  if (isFullscreenShortcut) {
    event.preventDefault();
    void handleToggleFullScreen();
  }
}

onMounted(async () => {
  try {
    cleanupFns.push(await listen("tauri://focus", () => {
      isWindowFocused.value = true;
    }));
    cleanupFns.push(await listen("tauri://blur", () => {
      isWindowFocused.value = false;
    }));
    cleanupFns.push(await listen("tauri://resize", () => {
      void syncFullscreenState();
    }));
  } catch (error) {
    console.error("Failed to setup window focus listeners:", error);
  }

  try {
    osType.value = await getOsType();
  } catch (error) {
    console.error("Failed to detect OS:", error);
  }

  await syncFullscreenState();

  try {
    const loaded = await invoke<AppSettings>("get_settings");
    settings.value = normalizeAppSettings(loaded);
  } catch {
    settings.value = normalizeAppSettings();
  }

  try {
    cleanupFns.push(await listen("show-about", () => {
      showAbout.value = true;
    }));
  } catch (error) {
    console.error("Failed to setup about listener:", error);
  }

  try {
    cleanupFns.push(await listen("menu-open-settings", () => {
      handleOpenSettings();
    }));
    cleanupFns.push(await listen("menu-new-local", () => {
      handleNewLocalSessionFromMenu();
    }));
    cleanupFns.push(await listen("menu-new-ssh", () => {
      handleOpenConnectionFromMenu("ssh");
    }));
    cleanupFns.push(await listen("menu-new-telnet", () => {
      handleOpenConnectionFromMenu("telnet");
    }));
    cleanupFns.push(await listen("menu-new-serial", () => {
      handleOpenConnectionFromMenu("serial");
    }));
    cleanupFns.push(await listen("menu-close-tab", () => {
      handleCloseActiveTab();
    }));
    cleanupFns.push(await listen("menu-toggle-bookmarks", () => {
      handleToggleBookmarks();
    }));
    cleanupFns.push(await listen("menu-toggle-remote-files", () => {
      handleToggleRemoteFileManager();
    }));
    cleanupFns.push(await listen("menu-increase-font-size", () => {
      handleIncreaseTerminalFontSize();
    }));
    cleanupFns.push(await listen("menu-decrease-font-size", () => {
      handleDecreaseTerminalFontSize();
    }));
    cleanupFns.push(await listen("menu-reset-font-size", () => {
      handleResetTerminalFontSize();
    }));
  } catch (error) {
    console.error("Failed to setup menu listeners:", error);
  }

  window.addEventListener("keydown", handleGlobalKeyDown);
  cleanupFns.push(() => {
    window.removeEventListener("keydown", handleGlobalKeyDown);
  });
});

onBeforeUnmount(() => {
  cleanupTabPointerTracking();
  while (cleanupFns.length > 0) {
    const cleanup = cleanupFns.pop();
    cleanup?.();
  }
});

const activeTab = computed(() => tabs.value.find((tab) => tab.id === activeTabId.value));
const activeSshConfig = computed<SshConfig | null>(() => (
  activeTab.value?.session.protocol === "ssh" ? activeTab.value.session.sshConfig : null
));
const activeSerialConfig = computed<SerialConfig | null>(() => (
  activeTab.value?.session.protocol === "serial" ? activeTab.value.session.serialConfig : null
));
const activeSerialConnectionState = computed<SerialConnectionState | null>(() => {
  if (!activeTab.value || !activeSerialConfig.value) {
    return null;
  }
  return serialConnectionStates.value[activeTab.value.id] ?? "connecting";
});
const primaryShortcutLabel = computed(() => (osType.value === "macos" ? "Cmd" : "Ctrl"));
const appClassName = computed(() => [
  "app-container",
  osType.value,
  isWindowFocused.value ? "focused" : "blurred",
  draggedTabId.value ? "tab-dragging" : "",
]);

async function handleSaveSettings(newSettings: AppSettings) {
  const normalizedSettings = normalizeAppSettings(newSettings);
  await invoke("save_settings", { settings: normalizedSettings }).catch(console.error);
  settings.value = normalizedSettings;
  showSettings.value = false;
}

function persistSettingsSilently(newSettings: AppSettings) {
  const normalizedSettings = normalizeAppSettings(newSettings);
  settingsRef.value = normalizedSettings;
  settings.value = normalizedSettings;
  void invoke("save_settings", { settings: normalizedSettings }).catch((error) => {
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
  const newSettings = normalizeAppSettings({ ...settings.value, quickButtons: buttons });
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

function fitActiveTerminal() {
  const handle = termRefs.get(activeTabId.value);
  if (handle) {
    handle.fit();
  }
}

watch(() => settings.value.showInputBar, () => {
  setTimeout(fitActiveTerminal, 0);
});

watch(activeSshConfig, (value) => {
  if (!value) {
    showRemoteFileManager.value = false;
  }
});

function selectTab(tabId: string) {
  activeTabId.value = tabId;
}

function handleTabClick(tabId: string) {
  if (suppressTabClick.value) {
    suppressTabClick.value = false;
    return;
  }
  selectTab(tabId);
}

function moveTabToIndex(tabId: string, targetIndex: number) {
  const currentIndex = tabs.value.findIndex((tab) => tab.id === tabId);
  if (currentIndex < 0) {
    return;
  }

  const normalizedTargetIndex = Math.max(0, Math.min(targetIndex, tabs.value.length));
  let insertIndex = normalizedTargetIndex;
  if (currentIndex < normalizedTargetIndex) {
    insertIndex -= 1;
  }

  if (insertIndex === currentIndex) {
    return;
  }

  const nextTabs = [...tabs.value];
  const [movedTab] = nextTabs.splice(currentIndex, 1);
  nextTabs.splice(insertIndex, 0, movedTab);
  tabs.value = nextTabs;
}

function cleanupTabPointerTracking() {
  // No-op: pointer tracking is now via setPointerCapture on tab elements,
  // so there are no window-level listeners to remove.
}

function beginTabDrag(tabId: string) {
  draggedTabId.value = tabId;
  dragPreviewTabId.value = tabId;
  activeTabId.value = tabId;
  tabDragMoved = true;
  suppressTabClick.value = true;
}

function finishTabDrag() {
  const moved = tabDragMoved;
  cleanupTabPointerTracking();
  pendingTabDrag = null;
  tabDragMoved = false;
  draggedTabId.value = null;
  dragPreviewTabId.value = null;
  suppressTabClick.value = moved;

  if (moved) {
    window.setTimeout(() => {
      suppressTabClick.value = false;
    }, 0);
  }
}

function handleTabPointerDown(event: PointerEvent, tabId: string) {
  if (event.button !== 0) {
    return;
  }

  const target = event.target as HTMLElement | null;
  if (target?.closest(".tab-close-btn")) {
    return;
  }

  event.preventDefault();
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);

  pendingTabDrag = {
    tabId,
    startX: event.clientX,
    startY: event.clientY,
  };
  tabDragMoved = false;
}

function handleTabPointerMove(event: PointerEvent, tabId: string) {
  if (!pendingTabDrag || pendingTabDrag.tabId !== tabId) {
    return;
  }

  if (!draggedTabId.value) {
    const deltaX = event.clientX - pendingTabDrag.startX;
    const deltaY = event.clientY - pendingTabDrag.startY;
    if (Math.hypot(deltaX, deltaY) < 6) {
      return;
    }

    beginTabDrag(pendingTabDrag.tabId);
  }

  // Use bounding rects of all tabs to find the drop target while pointer is captured
  const allTabEls = document.querySelectorAll<HTMLElement>(".tab-item[data-tab-id]");
  let targetTabEl: HTMLElement | null = null;
  for (const el of allTabEls) {
    const r = el.getBoundingClientRect();
    if (event.clientX >= r.left && event.clientX <= r.right && event.clientY >= r.top && event.clientY <= r.bottom) {
      targetTabEl = el;
      break;
    }
  }

  if (!targetTabEl || !draggedTabId.value) {
    return;
  }

  const hoveredTabId = targetTabEl.dataset.tabId;
  if (!hoveredTabId) {
    return;
  }

  const targetIndex = tabs.value.findIndex((tab) => tab.id === hoveredTabId);
  if (targetIndex < 0) {
    return;
  }

  const bounds = targetTabEl.getBoundingClientRect();
  const insertAfter = event.clientX > bounds.left + bounds.width / 2;
  moveTabToIndex(draggedTabId.value, targetIndex + (insertAfter ? 1 : 0));
  dragPreviewTabId.value = hoveredTabId;
}

function handleTabPointerUp(_event: PointerEvent, tabId: string) {
  if (!pendingTabDrag || pendingTabDrag.tabId !== tabId) {
    return;
  }
  finishTabDrag();
}

function handleTabPointerCancel(tabId: string) {
  if (!pendingTabDrag || pendingTabDrag.tabId !== tabId) {
    return;
  }
  pendingTabDrag = null;
  tabDragMoved = false;
  draggedTabId.value = null;
  dragPreviewTabId.value = null;
  suppressTabClick.value = false;
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
  closeOpenMenus();
  await getCurrentWindow().close().catch((error) => {
    console.error("exit failed", error);
  });
}

function handleOpenAbout() {
  closeOpenMenus();
  showAbout.value = true;
}

function handleOpenSettings() {
  closeOpenMenus();
  showSettings.value = true;
}

function adjustTerminalFontSize(delta: number) {
  closeOpenMenus();
  const nextFontSize = Math.min(
    MAX_TERMINAL_FONT_SIZE,
    Math.max(MIN_TERMINAL_FONT_SIZE, settingsRef.value.fontSize + delta),
  );

  if (nextFontSize === settingsRef.value.fontSize) {
    return;
  }

  persistSettingsSilently({
    ...settingsRef.value,
    fontSize: nextFontSize,
  });
}

function handleIncreaseTerminalFontSize() {
  adjustTerminalFontSize(1);
}

function handleDecreaseTerminalFontSize() {
  adjustTerminalFontSize(-1);
}

function handleResetTerminalFontSize() {
  closeOpenMenus();
  if (settingsRef.value.fontSize === DEFAULT_SETTINGS.fontSize) {
    return;
  }
  persistSettingsSilently({
    ...settingsRef.value,
    fontSize: DEFAULT_SETTINGS.fontSize,
  });
}

async function handleToggleFullScreen() {
  closeOpenMenus();
  const appWindow = getCurrentWindow();
  const nextFullscreen = !(await appWindow.isFullscreen().catch((error) => {
    console.error("isFullscreen failed", error);
    return false;
  }));
  await appWindow.setFullscreen(nextFullscreen).catch((error) => {
    console.error("setFullscreen failed", error);
  });
  isFullscreen.value = nextFullscreen;
}

function toggleRemoteFileManager() {
  if (!activeSshConfig.value) {
    return;
  }
  showRemoteFileManager.value = !showRemoteFileManager.value;
}

function handleOpenNewTabMenu() {
  closeOpenMenus();
  showNewTabMenu.value = true;
}

function handleNewLocalSessionFromMenu() {
  closeOpenMenus();
  handleNewLocalSession();
}

function handleOpenConnectionFromMenu(protocol: ConnectionProtocol) {
  closeOpenMenus();
  openConnect(protocol);
}

function handleToggleBookmarks() {
  closeOpenMenus();
  sidebarOpen.value = !sidebarOpen.value;
}

function handleToggleRemoteFileManager() {
  closeOpenMenus();
  toggleRemoteFileManager();
}

function handleCloseActiveTab() {
  closeOpenMenus();
  if (!activeTab.value) {
    return;
  }
  handleCloseTab(activeTab.value.id);
}

function toggleMenu(menuId: AppMenuId) {
  const nextMenuId = openMenuId.value === menuId ? null : menuId;
  openMenuId.value = nextMenuId;
  if (nextMenuId !== "file") {
    openFileSubmenuId.value = null;
  }
}

function toggleFileSubmenu(submenuId: FileSubmenuId) {
  openFileSubmenuId.value = openFileSubmenuId.value === submenuId ? null : submenuId;
}

function stopDragPropagation(event: MouseEvent) {
  event.stopPropagation();
}

function handleNewLocalSession() {
  const newId = `tab-${nextTabId++}`;
  const cwd = window.getStartupDir?.() ?? undefined;
  tabs.value = [...tabs.value, { id: newId, title: "Local Shell", session: { protocol: "local", cwd } }];
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

  const reconnectType = protocol === "ssh" ? normalizeReconnectType(sshConfig) : undefined;

  const connection: SavedConnection = {
    id: crypto.randomUUID(),
    name: saveAs,
    group: saveGroup,
    logPath: result.logPath,
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
    autoReconnect: protocol === "ssh" && reconnectType ? isReconnectEnabled(reconnectType) : undefined,
    reconnectType,
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
      logPath: connection.logPath,
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
      logPath: connection.logPath,
    };
  } else {
    const reconnectType = normalizeReconnectType(connection);
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
          autoReconnect: isReconnectEnabled(reconnectType),
          reconnectType,
        },
      },
      logPath: connection.logPath,
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
              <div
                class="titlebar-menu-submenu-group"
                @mouseenter="openFileSubmenuId = 'new-session'"
              >
                <button class="titlebar-menu-item titlebar-menu-item--submenu" type="button" @click="toggleFileSubmenu('new-session')">
                  <span>New Session</span>
                  <span class="titlebar-menu-item-arrow">›</span>
                </button>
                <div v-if="openFileSubmenuId === 'new-session'" class="titlebar-menu-submenu" @mousedown="stopDragPropagation">
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleNewLocalSessionFromMenu">
                    <span>Local Shell</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('ssh')">
                    <span>SSH</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('telnet')">
                    <span>Telnet</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('serial')">
                    <span>Serial</span>
                  </button>
                </div>
              </div>
              <button class="titlebar-menu-item" type="button" :disabled="!activeTab" @click="handleCloseActiveTab">
                <span>Close Tab</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+W</span>
              </button>
              <div class="titlebar-menu-separator" />
              <div
                class="titlebar-menu-submenu-group"
                @mouseenter="openFileSubmenuId = 'preferences'"
              >
                <button class="titlebar-menu-item titlebar-menu-item--submenu" type="button" @click="toggleFileSubmenu('preferences')">
                  <span>Preferences</span>
                  <span class="titlebar-menu-item-arrow">›</span>
                </button>
                <div v-if="openFileSubmenuId === 'preferences'" class="titlebar-menu-submenu" @mousedown="stopDragPropagation">
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenSettings">
                    <span>Settings</span>
                  </button>
                </div>
              </div>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleExitApp">
                <span>Exit</span>
                <span class="titlebar-menu-item-hint">Alt+F4</span>
              </button>
            </div>
          </div>

          <div class="titlebar-menu-group" @mouseenter="openMenuId ? (openMenuId = 'view') : null">
            <button
              class="titlebar-menu-btn"
              :class="{ open: openMenuId === 'view' }"
              type="button"
              @mousedown="stopDragPropagation"
              @click="toggleMenu('view')"
            >
              View
            </button>
            <div v-if="openMenuId === 'view'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" @click="handleToggleBookmarks">
                <span>{{ sidebarOpen ? 'Hide Bookmarks' : 'Show Bookmarks' }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" :disabled="!activeSshConfig" @click="handleToggleRemoteFileManager">
                <span>{{ showRemoteFileManager ? 'Hide Remote Files' : 'Show Remote Files' }}</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleIncreaseTerminalFontSize">
                <span>Increase Terminal Font Size</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}++</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleDecreaseTerminalFontSize">
                <span>Decrease Terminal Font Size</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+-</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleResetTerminalFontSize">
                <span>Reset Terminal Font Size</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+0</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleToggleFullScreen">
                <span>{{ isFullscreen ? 'Exit Full Screen' : 'Full Screen' }}</span>
                <span class="titlebar-menu-item-hint">F11</span>
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
      <TransitionGroup
        name="tab-sort"
        tag="div"
        :class="['tab-strip', { 'is-dragging': draggedTabId }]"
      >
        <button
          key="__bookmark__"
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
          :data-tab-id="tab.id"
          :class="{ active: activeTabId === tab.id, dragging: draggedTabId === tab.id }"
          @pointerdown="handleTabPointerDown($event, tab.id)"
          @pointermove="handleTabPointerMove($event, tab.id)"
          @pointerup="handleTabPointerUp($event, tab.id)"
          @pointercancel="handleTabPointerCancel(tab.id)"
          @click="handleTabClick(tab.id)"
        >
          <span class="tab-title">{{ tab.title }}</span>
          <button class="tab-close-btn" title="Close Tab" @click.stop="handleCloseTab(tab.id)">×</button>
        </div>

        <button key="__newtab__" class="tab-new-btn" type="button" title="New Tab" @mousedown.stop @click="handleOpenNewTabMenu">+</button>
      </TransitionGroup>

      <div class="tab-bar-actions">
        <button
          v-if="activeSshConfig"
          class="tab-new-btn tab-files-btn"
          :class="{ active: showRemoteFileManager }"
          type="button"
          title="Remote Files"
          @mousedown.stop
          @click.stop="toggleRemoteFileManager"
        >
          📁
        </button>
        <button class="tab-new-btn tab-settings-btn" type="button" title="Settings" @mousedown.stop @click.stop="handleOpenSettings">&#x2699;</button>
      </div>
    </div>

    <div class="workspace">
      <BookmarkSidebar v-if="sidebarOpen" :refresh-token="sidebarRefreshToken" :settings="settings" @connect="handleBookmarkConnect" />

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
            :session-id="tab.id"
            :is-active="activeTabId === tab.id"
            :session="tab.session"
            :log-path="tab.logPath"
            :settings="settings"
            @serial-connection-state-change="(state) => updateSerialConnectionState(tab.id, state)"
          />
        </div>

        <TerminalInputBar
          v-if="settings.showInputBar"
          :quick-buttons="settings.quickButtons"
          @send="sendToActiveTerminal"
          @buttons-change="handleButtonsChange"
          @resize="fitActiveTerminal"
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

      <RemoteFileManager
        v-if="showRemoteFileManager && activeTab && activeSshConfig"
        :key="activeTab.id"
        :session-id="activeTab.id"
        :ssh-config="activeSshConfig"
        @close="showRemoteFileManager = false"
      />
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
      :settings="settings"
      @connect="handleConnectResult"
      @cancel="showConnectDialog = false"
    />
  </div>
</template>