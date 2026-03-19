<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
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
import { usePaneLayout, type PaneAxis, type PaneLayoutTab } from "./usePaneLayout";
import {
  DEFAULT_SETTINGS,
  deriveUiTheme,
  MAX_INPUT_HISTORY,
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

type Tab = PaneLayoutTab;

interface TabContextMenuState {
  x: number;
  y: number;
  tabId: string;
}

type AppMenuId = "file" | "view" | "help";
type FileSubmenuId = "new-session" | "preferences";

let nextTabId = 1;

// NATO 字母表，用于生成唯一的标签页后缀
const NATO_ALPHABET = [
  "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
  "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
  "quebec", "romeo", "sierra", "tango", "uniform", "victor", "whiskey",
  "xray", "yankee", "zulu",
];
const TAB_TITLE_SUFFIX_PATTERN = new RegExp(`^(.*) – ((?:${NATO_ALPHABET.join("|")})|\\d+)$`, "i");

function stripGeneratedTabSuffix(title: string) {
  const match = title.match(TAB_TITLE_SUFFIX_PATTERN);
  return match ? match[1] : title;
}

/**
 * 生成唯一的标签页标题
 * 格式：优先使用书签名，其次回退到协议默认标题。
 */
function generateUniqueTabTitle(bookmarkName: string | undefined, baseTitle: string): string {
  const displayName = bookmarkName?.trim() || baseTitle;

  // 找出所有同名标签页（不带后缀的）
  const sameBaseTabs = tabs.value.filter(t => {
    return stripGeneratedTabSuffix(t.title) === displayName;
  });

  // 如果没有同名，直接返回
  if (sameBaseTabs.length === 0) {
    return displayName;
  }

  // 找出已使用的后缀索引
  const usedIndexes = new Set<number>();
  const usedNumericSuffixes = new Set<number>();
  for (const t of sameBaseTabs) {
    const match = t.title.match(TAB_TITLE_SUFFIX_PATTERN);
    if (match) {
      const suffix = match[2].toLowerCase();
      const idx = NATO_ALPHABET.indexOf(suffix);
      if (idx >= 0) {
        usedIndexes.add(idx);
        continue;
      }

      const numericSuffix = Number.parseInt(suffix, 10);
      if (Number.isFinite(numericSuffix)) {
        usedNumericSuffixes.add(numericSuffix);
      }
    }
  }

  // 找到下一个可用的字母
  for (let i = 0; i < NATO_ALPHABET.length; i++) {
    if (!usedIndexes.has(i)) {
      return `${displayName} – ${NATO_ALPHABET[i]}`;
    }
  }

  // 如果26个字母用完了，回退到数字后缀
  let nextNumericSuffix = 1;
  while (usedNumericSuffixes.has(nextNumericSuffix)) {
    nextNumericSuffix += 1;
  }

  return `${displayName} – ${nextNumericSuffix}`;
}

function buildBaseTabTitle(session: SessionConfig): string {
  if (session.protocol === "serial") {
    return `${session.serialConfig.portName} @ ${session.serialConfig.baudRate}`;
  }
  if (session.protocol === "telnet") {
    return `telnet://${session.telnetConfig.host}:${session.telnetConfig.port}`;
  }
  if (session.protocol === "ssh") {
    return `${session.sshConfig.user}@${session.sshConfig.host}`;
  }
  return "Local Shell";
}

function createSessionTab(tabId: string, session: SessionConfig, bookmarkName?: string, logPath?: string): Tab {
  return {
    id: tabId,
    title: generateUniqueTabTitle(bookmarkName, buildBaseTabTitle(session)),
    session,
    logPath,
  };
}

function formatSerialFrame(serialConfig: SerialConfig) {
  const parity = serialConfig.parity === "none" ? "N" : serialConfig.parity === "even" ? "E" : "O";
  return `${serialConfig.dataBits}${parity}${serialConfig.stopBits}`;
}

const tabs = ref<Tab[]>([]);
const osType = ref("windows");
const appWindow = getCurrentWindow();
const isMainWindow = new URLSearchParams(window.location.search).get('role') !== 'child';
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
const renamingTabId = ref<string | null>(null);
const renamingTabTitle = ref("");
const tabContextMenu = ref<TabContextMenuState | null>(null);
const suppressTabClick = ref(false);
const settingsRef = ref<AppSettings>(DEFAULT_SETTINGS);
const uiTheme = computed(() => deriveUiTheme(settings.value.theme, settings.value.uiThemeMode));
const menuBarRef = ref<HTMLDivElement | null>(null);
const tabContextMenuRef = ref<HTMLDivElement | null>(null);
const terminalContainerRef = ref<HTMLDivElement | null>(null);
const isFullscreen = ref(false);
const {
  paneLayout,
  focusedPaneId,
  activeTabId,
  draggedTabId,
  hoveredEmptyPaneId,
  dropTargetPaneId,
  dropTargetPosition,
  paneLeaves,
  paneSplitHandles,
  paneByTabId,
  restoreWorkspaceState,
  applyRestoredWorkspaceState,
  applyPaneLayoutFromTabs,
  createPersistedPaneLayoutState,
  createPersistedWorkspaceState,
  findPaneById,
  findPaneByTabId,
  focusPane,
  assignTabToFocusedPane,
  selectTab,
  splitTabToPane,
  handleSplitPane,
  handleClosePane,
  handleTabRemoved,
  getPaneShellStyle,
  getTerminalViewportStyle,
  getSplitHandleStyle,
  isPaneFocused,
  getTabTitle,
  getTabProtocolLabel,
  handlePaneResizePointerDown,
  handleTabPointerDown,
  handleTabPointerMove,
  handleTabPointerUp,
  handleTabPointerCancel,
  handlePaneHeaderPointerDown,
  handlePaneHeaderPointerMove,
  handlePaneHeaderPointerUp,
  handlePaneHeaderPointerCancel,
} = usePaneLayout({ tabs, isWindowFocused, terminalContainerRef });
const termRefs = new Map<string, TerminalHandle>();
const paneViewportRefs = new Map<string, HTMLElement>();
const cleanupFns: Array<() => void> = [];
let paneResizeObserver: ResizeObserver | null = null;
let pendingFitFrame: number | null = null;
const pendingFitTabIds = new Set<string>();
let persistWorkspaceStateTimer: number | null = null;
const hasLoadedSettings = ref(false);

function createDefaultLocalShellTab(tabId = "tab-0"): Tab {
  return {
    id: tabId,
    title: "Local Shell",
    session: { protocol: "local" },
  };
}

function syncTabIdCounter(sourceTabs: Array<{ id: string }>) {
  let maxTabIndex = -1;

  for (const item of sourceTabs) {
    const numericValue = item.id.startsWith("tab-") ? Number.parseInt(item.id.slice(4), 10) : Number.NaN;
    maxTabIndex = Math.max(maxTabIndex, Number.isFinite(numericValue) ? numericValue : -1);
  }

  nextTabId = Math.max(nextTabId, maxTabIndex + 1);
}

function prepareSettingsForSave(baseSettings: AppSettings, restoreEnabled = baseSettings.restoreTabsOnStartup) {
  return normalizeAppSettings({
    ...baseSettings,
    paneLayout: createPersistedPaneLayoutState(),
    workspaceState: createPersistedWorkspaceState(restoreEnabled),
  });
}

function scheduleWorkspaceStatePersistence() {
  if (!hasLoadedSettings.value) {
    return;
  }

  if (persistWorkspaceStateTimer !== null) {
    window.clearTimeout(persistWorkspaceStateTimer);
  }
  persistWorkspaceStateTimer = window.setTimeout(() => {
    persistWorkspaceStateTimer = null;
    const nextSettings = prepareSettingsForSave(settingsRef.value);
    if (
      JSON.stringify(settingsRef.value.paneLayout) === JSON.stringify(nextSettings.paneLayout)
      && JSON.stringify(settingsRef.value.workspaceState) === JSON.stringify(nextSettings.workspaceState)
    ) {
      return;
    }

    persistSettingsSilently(nextSettings);
  }, 240);
}

function updateTabSession(tabId: string, session: SessionConfig) {
  tabs.value = tabs.value.map((tab) => (
    tab.id === tabId
      ? { ...tab, session }
      : tab
  ));
}

function closeOpenMenus() {
  openMenuId.value = null;
  openFileSubmenuId.value = null;
  tabContextMenu.value = null;
}

watch(settings, (value) => {
  settingsRef.value = value;
}, { deep: true, immediate: true });

watch(uiTheme, (value) => {
  const root = document.documentElement;
  Object.entries(value.variables).forEach(([key, cssValue]) => {
    root.style.setProperty(key, String(cssValue));
  });
  root.style.colorScheme = value.appearance;
  document.body.style.backgroundColor = value.variables["--app-bg"];
  document.body.style.color = value.variables["--app-text"];
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

watch(tabContextMenu, (value, _previous, onCleanup) => {
  if (!value) {
    return;
  }

  const handlePointerDown = (event: PointerEvent) => {
    const target = event.target;
    if (!(target instanceof Node)) {
      return;
    }
    if (tabContextMenuRef.value?.contains(target)) {
      return;
    }
    tabContextMenu.value = null;
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      tabContextMenu.value = null;
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
  isFullscreen.value = await appWindow.isFullscreen().catch((error) => {
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
  if (typeof ResizeObserver !== "undefined") {
    paneResizeObserver = new ResizeObserver((entries) => {
      for (const entry of entries) {
        if (entry.contentRect.width < 2 || entry.contentRect.height < 2) {
          continue;
        }

        const tabId = (entry.target as HTMLElement).dataset.tabId;
        if (tabId) {
          queueTerminalFit(tabId);
        }
      }
    });

    for (const element of paneViewportRefs.values()) {
      paneResizeObserver.observe(element);
    }
  }

  try {
    cleanupFns.push(await listen("tauri://focus", () => {
      isWindowFocused.value = true;
      // Focus the active terminal when the window regains focus
      const handle = termRefs.get(activeTabId.value);
      if (handle) {
        handle.focus();
      }
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
    const normalizedSettings = normalizeAppSettings(loaded);
    settings.value = normalizedSettings;
    settingsRef.value = normalizedSettings;

    const restoredWorkspaceState = normalizedSettings.restoreTabsOnStartup
      ? restoreWorkspaceState(normalizedSettings.workspaceState)
      : null;

    if (restoredWorkspaceState) {
      applyRestoredWorkspaceState(restoredWorkspaceState);
      syncTabIdCounter(restoredWorkspaceState.tabs);
    } else {
      const defaultTabs = [createDefaultLocalShellTab()];
      applyPaneLayoutFromTabs(defaultTabs, normalizedSettings.paneLayout);
      syncTabIdCounter(defaultTabs);
    }
  } catch {
    const fallbackSettings = normalizeAppSettings();
    settings.value = fallbackSettings;
    settingsRef.value = fallbackSettings;
    const defaultTabs = [createDefaultLocalShellTab()];
    applyPaneLayoutFromTabs(defaultTabs, fallbackSettings.paneLayout);
    syncTabIdCounter(defaultTabs);
  }

  hasLoadedSettings.value = true;

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
    cleanupFns.push(await listen("menu-split-right", () => {
      handleSplitPaneFromView("vertical");
    }));
    cleanupFns.push(await listen("menu-split-down", () => {
      handleSplitPaneFromView("horizontal");
    }));
    cleanupFns.push(await listen("menu-close-pane", () => {
      handleClosePaneFromView();
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

  void nextTick(() => {
    fitVisibleTerminals();
  });
});

onBeforeUnmount(() => {
  if (hasLoadedSettings.value) {
    const finalSettings = prepareSettingsForSave(settingsRef.value);
    void invoke("save_settings", { settings: finalSettings }).catch((error) => {
      console.error("save_settings on unmount failed", error);
    });
  }

  if (persistWorkspaceStateTimer !== null) {
    window.clearTimeout(persistWorkspaceStateTimer);
    persistWorkspaceStateTimer = null;
  }
  if (pendingFitFrame !== null) {
    window.cancelAnimationFrame(pendingFitFrame);
    pendingFitFrame = null;
  }
  paneResizeObserver?.disconnect();
  paneResizeObserver = null;
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
  `theme-${uiTheme.value.appearance}`,
  isWindowFocused.value ? "focused" : "blurred",
  draggedTabId.value ? "tab-dragging" : "",
]);

function flushPendingTerminalFits() {
  pendingFitFrame = null;
  const tabIds = [...pendingFitTabIds];
  pendingFitTabIds.clear();

  for (const tabId of tabIds) {
    if (!paneByTabId.value[tabId]) {
      continue;
    }
    termRefs.get(tabId)?.fit();
  }
}

function queueTerminalFit(tabId: string | null | undefined) {
  if (!tabId) {
    return;
  }

  pendingFitTabIds.add(tabId);
  if (pendingFitFrame !== null) {
    return;
  }

  pendingFitFrame = window.requestAnimationFrame(() => {
    flushPendingTerminalFits();
  });
}

function fitVisibleTerminals() {
  for (const pane of paneLeaves.value) {
    queueTerminalFit(pane.tabId);
  }
}

function focusActiveTerminal() {
  if (!activeTabId.value) {
    return;
  }
  termRefs.get(activeTabId.value)?.focus();
}

function setPaneViewportRef(tabId: string, instance: Element | null) {
  const element = instance instanceof HTMLElement ? instance : null;
  const previous = paneViewportRefs.get(tabId);

  if (previous && previous !== element) {
    paneResizeObserver?.unobserve(previous);
    paneViewportRefs.delete(tabId);
  }

  if (!element) {
    return;
  }

  element.dataset.tabId = tabId;
  paneViewportRefs.set(tabId, element);
  paneResizeObserver?.observe(element);
  if (paneByTabId.value[tabId]) {
    queueTerminalFit(tabId);
  }
}

async function handleSaveSettings(newSettings: AppSettings) {
  const normalizedSettings = prepareSettingsForSave(newSettings, newSettings.restoreTabsOnStartup);
  await invoke("save_settings", { settings: normalizedSettings }).catch(console.error);
  settingsRef.value = normalizedSettings;
  settings.value = normalizedSettings;
  showSettings.value = false;
}

function persistSettingsSilently(newSettings: AppSettings) {
  const normalizedSettings = prepareSettingsForSave(newSettings, newSettings.restoreTabsOnStartup);
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

function addToInputHistory(text: string) {
  const trimmed = text.trim();
  if (!trimmed) return;

  const current = settingsRef.value.inputHistory ?? [];
  // Remove duplicate and add to front
  const filtered = current.filter(h => h !== trimmed);
  const newHistory = [trimmed, ...filtered].slice(0, MAX_INPUT_HISTORY);

  persistSettingsSilently({
    ...settingsRef.value,
    inputHistory: newHistory,
  });
}

function handleInputSend(text: string) {
  addToInputHistory(text.replace(/\n$/, '')); // Remove trailing newline for history
  sendToActiveTerminal(text);
}

async function handleButtonsChange(buttons: QuickButton[]) {
  const newSettings = prepareSettingsForSave({ ...settings.value, quickButtons: buttons });
  await invoke("save_settings", { settings: newSettings }).catch(console.error);
  settingsRef.value = newSettings;
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
    if (paneByTabId.value[tabId]) {
      queueTerminalFit(tabId);
    }
    return;
  }
  termRefs.delete(tabId);
}

function fitActiveTerminal() {
  fitVisibleTerminals();
}

watch(() => settings.value.showInputBar, () => {
  void nextTick(() => {
    fitActiveTerminal();
  });
});

watch([paneLayout, sidebarOpen, showRemoteFileManager], () => {
  void nextTick(() => {
    fitVisibleTerminals();
  });
}, { deep: true });

watch([tabs, paneLayout, focusedPaneId, activeTabId], () => {
  scheduleWorkspaceStatePersistence();
}, { deep: true });

watch(focusedPaneId, () => {
  void nextTick(() => {
    focusActiveTerminal();
  });
});

watch(activeSshConfig, (value) => {
  if (!value) {
    showRemoteFileManager.value = false;
  }
});

function handleTabClick(tabId: string) {
  if (suppressTabClick.value) {
    suppressTabClick.value = false;
    return;
  }
  selectTab(tabId);
}

function startTabRename(tabId: string) {
  const tab = tabs.value.find((item) => item.id === tabId);
  if (!tab) {
    return;
  }

  tabContextMenu.value = null;
  renamingTabId.value = tabId;
  renamingTabTitle.value = tab.title;
  activeTabId.value = tabId;

  void nextTick(() => {
    const input = document.querySelector<HTMLInputElement>(`.tab-item[data-tab-id="${tabId}"] .tab-title-input`);
    input?.focus();
    input?.select();
  });
}

function cancelTabRename() {
  renamingTabId.value = null;
  renamingTabTitle.value = "";
}

function sanitizeSessionName(title: string): string {
  return (
    title
      .trim()
      .replace(/[^a-zA-Z0-9_-]/g, "-")
      .replace(/-+/g, "-")
      .replace(/^-+|-+$/g, "")
    || "session"
  );
}

function commitTabRename() {
  if (!renamingTabId.value) {
    return;
  }

  const tabId = renamingTabId.value;
  const nextTitle = renamingTabTitle.value.trim();
  if (nextTitle) {
    const tab = tabs.value.find((t) => t.id === tabId);
    tabs.value = tabs.value.map((t) => (
      t.id === tabId ? { ...t, title: nextTitle } : t
    ));

    if (tab?.session.protocol === "ssh") {
      const reconnectType = tab.session.sshConfig.reconnectType;
      if (reconnectType === "screen" || reconnectType === "tmux") {
        const newSessionName = `at-${sanitizeSessionName(nextTitle)}`;
        void invoke("rename_ssh_session", { id: tabId, newName: newSessionName }).catch((error) => {
          console.warn("Failed to rename remote session:", error);
        });
      }
    }
  }

  cancelTabRename();
}

function handleTabRenameKeyDown(event: KeyboardEvent) {
  if (event.key === "Enter") {
    event.preventDefault();
    commitTabRename();
    return;
  }

  if (event.key === "Escape") {
    event.preventDefault();
    cancelTabRename();
  }
}

function handleTabContextMenu(event: MouseEvent, tabId: string) {
  const target = event.target;
  if (target instanceof Element && target.closest(".tab-close-btn")) {
    return;
  }

  event.preventDefault();
  const visiblePane = findPaneByTabId(paneLayout.value, tabId);
  if (visiblePane) {
    focusPane(visiblePane.paneId);
  } else {
    activeTabId.value = tabId;
  }
  tabContextMenu.value = {
    x: event.clientX,
    y: event.clientY,
    tabId,
  };
}

function handleRenameTabFromContextMenu() {
  const tabId = tabContextMenu.value?.tabId;
  if (!tabId) {
    return;
  }

  startTabRename(tabId);
}

function handleSplitTabFromContextMenu(axis: PaneAxis) {
  const tabId = tabContextMenu.value?.tabId;
  if (!tabId) {
    return;
  }

  tabContextMenu.value = null;
  splitTabToPane(tabId, axis);
}

function handleMoveTabToFocusedPaneFromContextMenu() {
  const tabId = tabContextMenu.value?.tabId;
  if (!tabId) {
    return;
  }

  tabContextMenu.value = null;
  assignTabToFocusedPane(tabId);
}

function handleClosePaneFromContextMenu() {
  const tabId = tabContextMenu.value?.tabId;
  if (!tabId) {
    return;
  }

  const pane = findPaneByTabId(paneLayout.value, tabId) ?? findPaneById(paneLayout.value, focusedPaneId.value);
  tabContextMenu.value = null;
  if (!pane) {
    return;
  }

  focusPane(pane.paneId);
  handleClosePane(pane.paneId);
}

function handleTitlebarMouseDown(event: MouseEvent) {
  if (event.button !== 0) {
    return;
  }
  if ((event.target as HTMLElement).closest("[data-no-drag='true']")) {
    return;
  }
  void appWindow.startDragging().catch((error) => {
    console.error("startDragging failed", error);
  });
}

async function handleMinimize() {
  await appWindow.minimize().catch((error) => {
    console.error("minimize failed", error);
  });
}

async function handleToggleMaximize() {
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
  await appWindow.close().catch((error) => {
    console.error("close failed", error);
  });
}

async function handleExitApp() {
  closeOpenMenus();
  await appWindow.close().catch((error) => {
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

function handleSplitPaneFromView(axis: PaneAxis) {
  closeOpenMenus();
  handleSplitPane(axis);
}

function handleClosePaneFromView() {
  closeOpenMenus();
  handleClosePane();
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
  assignTabToFocusedPane(newId);
}

function openConnect(protocol: ConnectionProtocol) {
  connectDialogProtocol.value = protocol;
  showConnectDialog.value = true;
}

function handleCloseTab(id: string) {
  const previousTabs = tabs.value;
  const nextTabs = previousTabs.filter((tab) => tab.id !== id);

  if (tabContextMenu.value?.tabId === id) {
    tabContextMenu.value = null;
  }

  if (renamingTabId.value === id) {
    cancelTabRename();
  }

  handleTabRemoved(id, nextTabs);

  if (id in serialConnectionStates.value) {
    const nextStates = { ...serialConnectionStates.value };
    delete nextStates[id];
    serialConnectionStates.value = nextStates;
  }
}

async function handleConnectResult(result: ConnectResult) {
  const newId = `tab-${nextTabId++}`;
  const { protocol, sshConfig, telnetConfig, serialConfig, saveAs, saveGroup } = result;
  let tab: Tab | null = null;
  const savedConnectionId = saveAs ? crypto.randomUUID() : undefined;

  if (protocol === "ssh" && sshConfig) {
    tab = createSessionTab(newId, {
      protocol: "ssh",
      sshConfig: {
        ...sshConfig,
        savedConnectionId,
      },
    }, saveAs, result.logPath);
  } else if (protocol === "telnet" && telnetConfig) {
    tab = createSessionTab(newId, { protocol: "telnet", telnetConfig }, saveAs, result.logPath);
  } else if (protocol === "serial" && serialConfig) {
    tab = createSessionTab(newId, { protocol: "serial", serialConfig }, saveAs, result.logPath);
  }

  if (!tab) {
    showConnectDialog.value = false;
    return;
  }

  tabs.value = [...tabs.value, tab];

  if (protocol === "serial" && serialConfig) {
    rememberSerialConfig(serialConfig);
    updateSerialConnectionState(newId, "connecting");
  }

  assignTabToFocusedPane(newId);
  showConnectDialog.value = false;

  if (!saveAs) {
    return;
  }

  const reconnectType = protocol === "ssh" ? normalizeReconnectType(sshConfig) : undefined;

  const connection: SavedConnection = {
    id: savedConnectionId ?? crypto.randomUUID(),
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
    tab = createSessionTab(newId, { protocol: "serial", serialConfig }, connection.name, connection.logPath);
  } else if (protocol === "telnet") {
    tab = createSessionTab(newId, {
      protocol: "telnet",
      telnetConfig: {
        host: connection.host,
        port: connection.port,
      },
    }, connection.name, connection.logPath);
  } else {
    const reconnectType = normalizeReconnectType(connection);
    tab = createSessionTab(newId, {
      protocol: "ssh",
      sshConfig: {
        host: connection.host,
        port: connection.port,
        user: connection.user,
        password: connection.password,
        privateKey: connection.authType === "key" ? connection.privateKey : undefined,
        savedConnectionId: connection.id,
        autoReconnect: isReconnectEnabled(reconnectType),
        reconnectType,
      },
    }, connection.name, connection.logPath);
  }

  if (tab.session.protocol === "serial") {
    updateSerialConnectionState(newId, "connecting");
  }

  tabs.value = [...tabs.value, tab];
  assignTabToFocusedPane(newId);
}
</script>

<template>
  <div :class="appClassName">
    <div v-if="isMainWindow" class="titlebar" @mousedown="handleTitlebarMouseDown" @dblclick="handleToggleMaximize">
      <div v-if="isMainWindow && osType !== 'windows'" class="titlebar-controls" aria-label="Window controls" data-no-drag="true">
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
              <button class="titlebar-menu-item" type="button" @click="handleSplitPaneFromView('vertical')">
                <span>Split Right</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleSplitPaneFromView('horizontal')">
                <span>Split Down</span>
              </button>
              <button class="titlebar-menu-item" type="button" :disabled="paneLeaves.length <= 1" @click="handleClosePaneFromView">
                <span>Close Pane</span>
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

      <div v-if="isMainWindow && osType === 'windows'" class="titlebar-controls-win" aria-label="Window controls" data-no-drag="true">
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

    <div v-if="isMainWindow" class="tab-bar">
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
          :title="renamingTabId === tab.id ? undefined : `${tab.title}\nRight-click to rename`"
          :class="{ active: activeTabId === tab.id, dragging: draggedTabId === tab.id }"
          @pointerdown="handleTabPointerDown($event, tab.id)"
          @pointermove="handleTabPointerMove($event, tab.id)"
          @pointerup="handleTabPointerUp($event, tab.id)"
          @pointercancel="handleTabPointerCancel(tab.id)"
          @click="handleTabClick(tab.id)"
          @contextmenu.prevent.stop="handleTabContextMenu($event, tab.id)"
        >
          <input
            v-if="renamingTabId === tab.id"
            v-model="renamingTabTitle"
            class="tab-title-input"
            type="text"
            maxlength="120"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            @blur="commitTabRename"
            @click.stop
            @keydown="handleTabRenameKeyDown"
            @pointerdown.stop
          >
          <span v-else class="tab-title">{{ tab.title }}</span>
          <button class="tab-close-btn" title="Close Tab" @click.stop="handleCloseTab(tab.id)">×</button>
        </div>

        <button key="__newtab__" class="tab-new-btn" type="button" title="New Tab" @mousedown.stop @click="handleOpenNewTabMenu">+</button>
      </TransitionGroup>

      <div class="tab-bar-actions">
        <button
          class="tab-new-btn"
          type="button"
          title="Split Right"
          @mousedown.stop
          @click.stop="handleSplitPane('vertical')"
        >
          ║
        </button>
        <button
          class="tab-new-btn"
          type="button"
          title="Split Down"
          @mousedown.stop
          @click.stop="handleSplitPane('horizontal')"
        >
          ＝
        </button>
        <button
          class="tab-new-btn"
          type="button"
          title="Close Pane"
          :disabled="paneLeaves.length <= 1"
          @mousedown.stop
          @click.stop="handleClosePane()"
        >
          ◫
        </button>
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

    <div
      v-if="tabContextMenu"
      ref="tabContextMenuRef"
      class="tab-context-menu"
      :style="{ top: `${tabContextMenu.y}px`, left: `${tabContextMenu.x}px` }"
    >
      <button class="tab-context-item" type="button" @click="handleSplitTabFromContextMenu('vertical')">Split Right</button>
      <button class="tab-context-item" type="button" @click="handleSplitTabFromContextMenu('horizontal')">Split Down</button>
      <button class="tab-context-item" type="button" @click="handleMoveTabToFocusedPaneFromContextMenu">Move To Focused Pane</button>
      <button class="tab-context-item" type="button" :disabled="paneLeaves.length <= 1" @click="handleClosePaneFromContextMenu">Close Pane</button>
      <div class="titlebar-menu-separator" />
      <button class="tab-context-item" type="button" @click="handleRenameTabFromContextMenu">Rename Tab</button>
    </div>

    <div class="workspace">
      <BookmarkSidebar v-if="sidebarOpen" :refresh-token="sidebarRefreshToken" :settings="settings" @connect="handleBookmarkConnect" />

      <div class="terminal-wrapper">
        <div ref="terminalContainerRef" class="terminal-container split-terminal-container">
          <div v-if="tabs.length === 0" class="terminal-empty-state">
            No open tabs. Click + to open a new tab.
          </div>

          <div v-else class="pane-frame-layer">
            <div
              v-for="pane in paneLeaves"
              :key="pane.paneId"
              class="terminal-pane-frame"
              :class="{ focused: isPaneFocused(pane.paneId), empty: !pane.tabId }"
              :style="getPaneShellStyle(pane.rect)"
              :data-pane-id="pane.paneId"
            >
              <div
                v-if="isMainWindow && paneLeaves.length > 1"
                class="terminal-pane-header"
                @pointerdown="handlePaneHeaderPointerDown($event, pane.paneId)"
                @pointermove="handlePaneHeaderPointerMove($event, pane.paneId)"
                @pointerup="handlePaneHeaderPointerUp($event, pane.paneId)"
                @pointercancel="handlePaneHeaderPointerCancel(pane.paneId)"
              >
                <div class="terminal-pane-header-meta" @mousedown="focusPane(pane.paneId)">
                  <span class="terminal-pane-title">{{ getTabTitle(pane.tabId) }}</span>
                  <span class="terminal-pane-protocol">{{ getTabProtocolLabel(pane.tabId) }}</span>
                </div>
                <div class="terminal-pane-actions">
                  <button type="button" title="Split Right" @click.stop="focusPane(pane.paneId); handleSplitPane('vertical', pane.paneId)">║</button>
                  <button type="button" title="Split Down" @click.stop="focusPane(pane.paneId); handleSplitPane('horizontal', pane.paneId)">＝</button>
                  <button type="button" title="Close Pane" :disabled="paneLeaves.length <= 1" @click.stop="focusPane(pane.paneId); handleClosePane(pane.paneId)">×</button>
                </div>
              </div>

              <div
                v-if="!pane.tabId"
                class="terminal-pane-empty"
                :class="{ 'drag-target': hoveredEmptyPaneId === pane.paneId }"
                :data-pane-id="pane.paneId"
                @mousedown="focusPane(pane.paneId)"
              >
                <div class="terminal-pane-empty-title">Empty Pane</div>
                <div class="terminal-pane-empty-desc">
                  {{ draggedTabId ? 'Release to move this tab into the empty pane.' : 'Drag a tab here, or select a tab above to show it here.' }}
                </div>
              </div>
              <div
                v-if="dropTargetPaneId === pane.paneId && dropTargetPosition"
                class="terminal-pane-drop-preview"
                :class="dropTargetPosition"
              />
            </div>
          </div>

          <div v-if="tabs.length > 0" class="terminal-instance-layer">
            <div
              v-for="tab in tabs"
              :key="tab.id"
              :ref="(instance) => setPaneViewportRef(tab.id, instance as Element | null)"
              class="terminal-instance-shell"
              :class="{ visible: Boolean(paneByTabId[tab.id]), focused: activeTabId === tab.id }"
              :style="getTerminalViewportStyle(paneByTabId[tab.id]?.rect ?? { left: 0, top: 0, width: 0, height: 0 }, Boolean(paneByTabId[tab.id]))"
              @mousedown="paneByTabId[tab.id] ? focusPane(paneByTabId[tab.id].paneId) : null"
            >
              <TerminalComponent
                :ref="(instance) => setTerminalRef(tab.id, instance)"
                :session-id="tab.id"
                :is-visible="Boolean(paneByTabId[tab.id])"
                :is-focused="activeTabId === tab.id"
                :session="tab.session"
                :log-path="tab.logPath"
                :settings="settings"
                @session-update="(session) => updateTabSession(tab.id, session)"
                @ssh-password-updated="sidebarRefreshToken += 1"
                @serial-connection-state-change="(state) => updateSerialConnectionState(tab.id, state)"
              />
            </div>
          </div>

          <div v-if="tabs.length > 0" class="pane-resize-layer">
            <div
              v-for="handle in paneSplitHandles"
              :key="handle.splitId"
              class="pane-split-handle"
              :class="handle.axis"
              :style="getSplitHandleStyle(handle)"
              @pointerdown="handlePaneResizePointerDown($event, handle)"
            >
              <div class="pane-split-handle-grip" />
            </div>
          </div>
        </div>

        <TerminalInputBar
          v-if="settings.showInputBar"
          :quick-buttons="settings.quickButtons"
          :input-history="settings.inputHistory"
          @send="handleInputSend"
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