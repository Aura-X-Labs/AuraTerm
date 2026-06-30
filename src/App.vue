<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { type as getOsType } from "@tauri-apps/plugin-os";
import TerminalComponent from "./TerminalComponent.vue";
import ConnectDialog from "./ConnectDialog.vue";
import BookmarkSidebar from "./BookmarkSidebar.vue";
import SettingsDialog from "./SettingsDialog.vue";
import AboutDialog from "./AboutDialog.vue";
import TerminalInputBar from "./TerminalInputBar.vue";
import RemoteFileManager from "./RemoteFileManager.vue";
import MasterPasswordDialog from "./MasterPasswordDialog.vue";
import TunnelManager from "./TunnelManager.vue";
import CommandPalette from "./CommandPalette.vue";
import CloudSyncDialog from "./CloudSyncDialog.vue";
import { open as openExternalUrl } from "@tauri-apps/plugin-shell";
import { useSshTunnels } from "./composables/useSshTunnels";
import { usePaneLayout, type PaneAxis, type PaneLayoutTab } from "./usePaneLayout";
import { useAppEventListeners } from "./composables/useAppEventListeners";
import { useWorkspacePersistence } from "./composables/useWorkspacePersistence";
import { useAppMenus } from "./composables/useAppMenus";
import { useTitlebarControls } from "./composables/useTitlebarControls";
import { useTerminalFontSize } from "./composables/useTerminalFontSize";
import { useTabManager } from "./composables/useTabManager";
import { setLanguage, t, type AppLanguage } from "./i18n";
import {
  DEFAULT_SETTINGS,
  deriveUiTheme,
  MAX_INPUT_HISTORY,
  normalizeAppSettings,
  type AppSettings,
  type QuickButton,
  type SerialHistoryItem,
} from "./settings";
import {
  buildSavedConnectionFromConnectResult,
  buildSessionFromConnectResult,
  buildSessionFromSavedConnection,
} from "./composables/sessionMapping";
import type {
  ConnectResult,
  ConnectionProtocol,
  PaletteCommand,
  SavedConnection,
  SerialConfig,
  SerialConnectionState,
  SessionConfig,
  SshConfig,
  TerminalHandle,
  TerminalSearchOptions,
  TerminalSearchResults,
  TunnelConfig,
} from "./types";
import logoUrl from "./logo.png";
// App styles, split by feature area (order preserved for the cascade).
import "./styles/base-and-titlebar.css";
import "./styles/tabs.css";
import "./styles/workspace.css";
import "./styles/input-bar.css";
import "./styles/bookmark-sidebar.css";
import "./styles/settings.css";
import "./styles/overlays.css";

type Tab = PaneLayoutTab;

type TerminalSearchToggleKey = "caseSensitive" | "wholeWord" | "regex";

const EMPTY_TERMINAL_SEARCH_RESULTS: TerminalSearchResults = {
  query: "",
  resultIndex: -1,
  resultCount: 0,
  limitExceeded: false,
};

const tabs = ref<Tab[]>([]);
const osType = ref("windows");
const isMainWindow = new URLSearchParams(window.location.search).get('role') !== 'child';

// Apply the UI language and keep the native macOS menubar in sync with it.
function syncLanguage(language: AppLanguage) {
  const locale = setLanguage(language);
  if (osType.value === "macos") {
    void invoke("set_menu_language", { locale }).catch(() => {});
  }
}
const showConnectDialog = ref(false);
const connectDialogProtocol = ref<ConnectionProtocol>("ssh");
// settings/settingsRef are replaced wholesale (normalizeAppSettings returns new objects),
// so shallowRef avoids building a deep Proxy over the large AppSettings tree.
const settings = shallowRef<AppSettings>(DEFAULT_SETTINGS);
const showSettings = ref(false);
const showCloudSync = ref(false);
const showAbout = ref(false);
const sidebarOpen = ref(false);
const sidebarRefreshToken = ref(0);
const sidebarExpandGroup = ref<string | undefined>(undefined);
const showRemoteFileManager = ref(false);
const showTunnelManager = ref(false);
const showCommandPalette = ref(false);
// Shared SSH port-forwarding runtime state (status mirror + start/stop/list).
const tunnels = useSshTunnels();
// Per-tab configured tunnels, kept out of `tab.session` so editing them never
// triggers the session-revision reconnect. Persistence is via bookmarks.
const tabTunnels = ref<Record<string, TunnelConfig[]>>({});
// Bookmarks snapshot loaded when the command palette opens (for quick-connect).
const paletteBookmarks = ref<SavedConnection[]>([]);
// MultiExec: when on, keystrokes typed into the focused pane are fanned out to
// every other currently-visible pane. Targets are the visible split panes only.
const broadcastInput = ref(false);
const isWindowFocused = ref(true);
// Immutable Record updates ({ ...obj, [k]: v }) pair best with shallowRef:
// the whole map is swapped, so shallow reactivity is enough and skips per-key proxies.
const serialConnectionStates = shallowRef<Record<string, SerialConnectionState>>({});
const suppressTabClick = ref(false);
const settingsRef = shallowRef<AppSettings>(DEFAULT_SETTINGS);
const uiTheme = computed(() => deriveUiTheme(settings.value.theme, settings.value.uiThemeMode));
const terminalContainerRef = ref<HTMLDivElement | null>(null);
const terminalSearchInputRef = ref<HTMLInputElement | null>(null);
// DOM refs the component binds via `ref="…"`; passed into useAppMenus so its
// outside-click watchers can hit-test against them.
const menuBarRef = ref<HTMLDivElement | null>(null);
const layoutMenuRef = ref<HTMLDivElement | null>(null);
const tabContextMenuRef = ref<HTMLDivElement | null>(null);
const terminalSearchVisible = ref(false);
const terminalSearchQuery = ref("");
const showMasterPasswordDialog = ref(false);
const masterPasswordDialogMode = ref<'setup' | 'unlock'>('setup');
const masterPasswordVerified = ref(false);
const masterPasswordRememberAvailable = ref(false);
const terminalSearchOptions = ref<Required<Pick<TerminalSearchOptions, TerminalSearchToggleKey>>>({
  caseSensitive: false,
  wholeWord: false,
  regex: false,
});
// Same immutable-swap pattern as serialConnectionStates.
const terminalSearchResults = shallowRef<Record<string, TerminalSearchResults>>({});
let searchDebounceTimer: number | null = null;

// Menu / dropdown / context-menu open-close state and outside-click handling.
const {
  openMenuId,
  openFileSubmenuId,
  tabContextMenu,
  showLayoutMenu,
  showNewTabMenu,
  layoutMenuPos,
  closeOpenMenus,
  toggleLayoutMenu,
  toggleMenu,
  toggleFileSubmenu,
  handleOpenNewTabMenu,
} = useAppMenus({ menuBarRef, layoutMenuRef, tabContextMenuRef });
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
const hasLoadedSettings = ref(false);

const {
  prepareSettingsForSave,
  persistSettingsSilently,
  scheduleWorkspaceStatePersistence,
  clearScheduledWorkspacePersistence,
} = useWorkspacePersistence({
  settings,
  settingsRef,
  hasLoadedSettings,
  createPersistedPaneLayoutState,
  createPersistedWorkspaceState,
});

// Custom-titlebar window controls + fullscreen state.
const {
  isFullscreen,
  syncFullscreenState,
  handleTitlebarMouseDown,
  handleMinimize,
  handleToggleMaximize,
  handleClose,
  handleExitApp,
  handleToggleFullScreen,
  stopDragPropagation,
} = useTitlebarControls({ closeOpenMenus });

// Terminal font-size zoom (View menu + Ctrl/Cmd +/-/0).
const {
  handleIncreaseTerminalFontSize,
  handleDecreaseTerminalFontSize,
  handleResetTerminalFontSize,
} = useTerminalFontSize({ settingsRef, persistSettingsSilently, closeOpenMenus });

// Tab title generation/de-dup, inline rename flow, and the tab-id counter.
const {
  renamingTabId,
  renamingTabTitle,
  createSessionTab,
  formatSerialFrame,
  mintTabId,
  syncTabIdCounter,
  startTabRename,
  cancelTabRename,
  commitTabRename,
  handleTabRenameKeyDown,
} = useTabManager({
  tabs,
  activeTabId,
  closeTabContextMenu: () => {
    tabContextMenu.value = null;
  },
});

const { registerAppEventListeners } = useAppEventListeners({
  setWindowFocused: (focused) => {
    isWindowFocused.value = focused;
  },
  focusActiveTerminal,
  syncFullscreenState,
  handleOpenAbout,
  handleOpenSettings,
  handleOpenCloudSync,
  handleNewLocalSessionFromMenu,
  handleOpenConnectionFromMenu,
  handleCloseActiveTab,
  handleToggleBookmarks,
  handleToggleRemoteFileManager,
  handleToggleTunnelManager,
  handleToggleCommandPalette,
  handleSplitPaneFromView,
  handleClosePaneFromView,
  handleIncreaseTerminalFontSize,
  handleDecreaseTerminalFontSize,
  handleResetTerminalFontSize,
});

function createDefaultLocalShellTab(tabId = "tab-0"): Tab {
  return {
    id: tabId,
    title: "Local Shell",
    session: { protocol: "local" },
  };
}

function updateTabSession(tabId: string, session: SessionConfig) {
  tabs.value = tabs.value.map((tab) => (
    tab.id === tabId
      ? { ...tab, session }
      : tab
  ));
}

// settings is replaced wholesale; shallow watch suffices (no deep traversal).
watch(settings, (value) => {
  settingsRef.value = value;
}, { immediate: true });

// Re-apply the UI language whenever the preference changes (e.g. via Settings).
watch(() => settings.value.language, (language) => {
  syncLanguage(language);
});

// uiTheme is a computed that returns a fresh object on change; deep traversal is unnecessary.
watch(uiTheme, (value) => {
  const root = document.documentElement;
  Object.entries(value.variables).forEach(([key, cssValue]) => {
    root.style.setProperty(key, String(cssValue));
  });
  root.style.colorScheme = value.appearance;
  document.body.style.backgroundColor = value.variables["--app-bg"];
  document.body.style.color = value.variables["--app-text"];
}, { immediate: true });

function handleGlobalKeyDown(event: KeyboardEvent) {
  const hasPrimaryModifier = event.ctrlKey || event.metaKey;
  const normalizedKey = event.key.toLowerCase();

  if (hasPrimaryModifier && !event.altKey) {
    if (event.shiftKey && normalizedKey === "p") {
      event.preventDefault();
      handleToggleCommandPalette();
      return;
    }

    if (!event.shiftKey && normalizedKey === "f") {
      event.preventDefault();
      handleOpenTerminalSearch();
      return;
    }

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

  if (terminalSearchVisible.value && event.key === "F3") {
    event.preventDefault();
    if (event.shiftKey) {
      handleFindPreviousInTerminal();
    } else {
      handleFindNextInTerminal();
    }
    return;
  }

  if (terminalSearchVisible.value && event.key === "Escape" && !isEditableTarget(event.target)) {
    event.preventDefault();
    handleCloseTerminalSearch();
    return;
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
    syncLanguage(normalizedSettings.language);

    const restoredWorkspaceState = normalizedSettings.restoreTabsOnStartup
      ? restoreWorkspaceState(normalizedSettings.workspaceState)
      : null;

    if (restoredWorkspaceState) {
      try {
        const savedConnections = await invoke<SavedConnection[]>("get_connections");
        const savedById = new Map(savedConnections.map((connection) => [connection.id, connection]));
        for (const tab of restoredWorkspaceState.tabs) {
          if (tab.session.protocol !== "ssh") {
            continue;
          }
          const savedId = tab.session.sshConfig.savedConnectionId;
          const saved = savedId ? savedById.get(savedId) : undefined;
          if (saved) {
            tab.session = buildSessionFromSavedConnection(saved);
          }
        }
      } catch (error) {
        console.error("Failed to hydrate restored SSH credentials", error);
      }
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
    syncLanguage(fallbackSettings.language);
    const defaultTabs = [createDefaultLocalShellTab()];
    applyPaneLayoutFromTabs(defaultTabs, fallbackSettings.paneLayout);
    syncTabIdCounter(defaultTabs);
  }

  hasLoadedSettings.value = true;

  // 凭据保护状态:
  //  - 无主密码(本地密钥模式,默认):不弹窗;
  //  - 有主密码:先尝试钥匙串静默自动解锁,失败再弹解锁框。
  // 全新安装默认无主密码,因此不再强制首次设置(可在 设置→Security 里启用)。
  try {
    const sec = await invoke<{
      passwordEnabled: boolean;
      unlocked: boolean;
      rememberEnabled: boolean;
      rememberAvailable: boolean;
    }>("get_credential_security_state");
    masterPasswordRememberAvailable.value = sec.rememberAvailable;

    if (!sec.passwordEnabled || sec.unlocked) {
      masterPasswordVerified.value = true;
    } else if (await invoke<boolean>("try_auto_unlock")) {
      masterPasswordVerified.value = true;
    } else {
      masterPasswordDialogMode.value = 'unlock';
      showMasterPasswordDialog.value = true;
    }
  } catch (error) {
    console.error("Failed to resolve credential security state", error);
  }

  cleanupFns.push(await registerAppEventListeners());

  await tunnels.registerTunnelListener();
  cleanupFns.push(() => {
    tunnels.disposeTunnelListener();
  });

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

  clearScheduledWorkspacePersistence();
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
const activeTerminalSearchResults = computed(() => {
  if (!activeTabId.value) {
    return EMPTY_TERMINAL_SEARCH_RESULTS;
  }
  return terminalSearchResults.value[activeTabId.value] ?? EMPTY_TERMINAL_SEARCH_RESULTS;
});
const terminalSearchSummary = computed(() => {
  if (!terminalSearchQuery.value) {
    return "Search the active terminal";
  }

  const results = activeTerminalSearchResults.value;
  if (results.query !== terminalSearchQuery.value) {
    return "Searching...";
  }
  if (results.resultIndex < 0) {
    return "No matches";
  }
  return "Found";
});
const appClassName = computed(() => [
  "app-container",
  osType.value,
  `theme-${uiTheme.value.appearance}`,
  isWindowFocused.value ? "focused" : "blurred",
  draggedTabId.value ? "tab-dragging" : "",
]);

// Step 4: 计算在非焦点 pane 中可见的 tab id 集合（用于在 tab 栏显示多 pane 指示点）
const visibleNonFocusedTabIds = computed(() => {
  if (paneLeaves.value.length <= 1) return new Set<string>();
  const result = new Set<string>();
  for (const pane of paneLeaves.value) {
    if (pane.tabId && pane.paneId !== focusedPaneId.value) {
      result.add(pane.tabId);
    }
  }
  return result;
});

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

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) {
    return false;
  }

  const tagName = target.tagName;
  return target.isContentEditable || tagName === "INPUT" || tagName === "TEXTAREA" || tagName === "SELECT";
}

function focusTerminalSearchInput(selectText = false) {
  const input = terminalSearchInputRef.value;
  if (!input) {
    return;
  }

  input.focus();
  if (selectText) {
    input.select();
  }
}

function updateTerminalSearchResults(tabId: string, results: TerminalSearchResults) {
  terminalSearchResults.value = {
    ...terminalSearchResults.value,
    [tabId]: results,
  };
}

function removeTerminalSearchResults(tabId: string) {
  if (!(tabId in terminalSearchResults.value)) {
    return;
  }

  const nextResults = { ...terminalSearchResults.value };
  delete nextResults[tabId];
  terminalSearchResults.value = nextResults;
}

function getActiveTerminalHandle() {
  return termRefs.get(activeTabId.value);
}

function syncSearchToActiveTerminal(direction: "next" | "previous" = "next", incremental = true) {
  const tabId = activeTabId.value;
  const handle = getActiveTerminalHandle();
  if (!tabId || !handle) {
    return false;
  }

  if (!terminalSearchQuery.value) {
    handle.clearSearch();
    updateTerminalSearchResults(tabId, EMPTY_TERMINAL_SEARCH_RESULTS);
    return false;
  }

  const searchOptions: TerminalSearchOptions = {
    ...terminalSearchOptions.value,
    incremental,
  };
  const matched = direction === "previous"
    ? handle.findPrevious(terminalSearchQuery.value, searchOptions)
    : handle.findNext(terminalSearchQuery.value, searchOptions);

  if (!matched) {
    updateTerminalSearchResults(tabId, {
      query: terminalSearchQuery.value,
      resultIndex: -1,
      resultCount: 0,
      limitExceeded: false,
    });
  }

  return matched;
}

function handleOpenTerminalSearch() {
  closeOpenMenus();
  if (!activeTab.value) {
    return;
  }

  terminalSearchVisible.value = true;
  void nextTick(() => {
    focusTerminalSearchInput(true);
    syncSearchToActiveTerminal("next", true);
  });
}

function handleCloseTerminalSearch() {
  if (!terminalSearchVisible.value && !terminalSearchQuery.value) {
    return;
  }

  terminalSearchVisible.value = false;
  terminalSearchQuery.value = "";
  terminalSearchResults.value = {};
  for (const handle of termRefs.values()) {
    handle.clearSearch();
  }
  void nextTick(() => {
    focusActiveTerminal();
  });
}

function handleFindNextInTerminal() {
  if (!terminalSearchVisible.value) {
    handleOpenTerminalSearch();
    return;
  }
  syncSearchToActiveTerminal("next", false);
}

function handleFindPreviousInTerminal() {
  if (!terminalSearchVisible.value) {
    handleOpenTerminalSearch();
    return;
  }
  syncSearchToActiveTerminal("previous", false);
}

function toggleTerminalSearchOption(key: TerminalSearchToggleKey) {
  terminalSearchOptions.value = {
    ...terminalSearchOptions.value,
    [key]: !terminalSearchOptions.value[key],
  };
}

function handleTerminalSearchKeyDown(event: KeyboardEvent) {
  if (event.key === "Enter") {
    event.preventDefault();
    if (event.shiftKey) {
      handleFindPreviousInTerminal();
    } else {
      handleFindNextInTerminal();
    }
    return;
  }

  if (event.key === "Escape") {
    event.preventDefault();
    handleCloseTerminalSearch();
  }
}

function handleTerminalSearchBlur() {
  getActiveTerminalHandle()?.clearSearchActiveDecoration();
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

function handleMasterPasswordSuccess() {
  // 主密码设置成功
  showMasterPasswordDialog.value = false;
  masterPasswordVerified.value = true;
}

function handleMasterPasswordUnlocked() {
  // 主密码验证成功
  showMasterPasswordDialog.value = false;
  masterPasswordVerified.value = true;
}

function handleMasterPasswordCancel() {
  // 用户取消，如果是首次设置则退出应用
  if (masterPasswordDialogMode.value === 'setup') {
    // 首次设置被取消，退出应用
    void invoke('tauri', { __tauriModule: 'Core', message: { cmd: 'exit', exitCode: 0 } }).catch(() => {
      window.close();
    });
  } else {
    // 解锁被取消，退出应用
    void invoke('tauri', { __tauriModule: 'Core', message: { cmd: 'exit', exitCode: 0 } }).catch(() => {
      window.close();
    });
  }
}

async function handleSaveSettings(newSettings: AppSettings) {
  const normalizedSettings = prepareSettingsForSave(newSettings, newSettings.restoreTabsOnStartup);
  await invoke("save_settings", { settings: normalizedSettings }).catch(console.error);
  settingsRef.value = normalizedSettings;
  settings.value = normalizedSettings;
  showSettings.value = false;
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

function sendToActiveTerminal(text: string, raw = false) {
  const handle = termRefs.get(activeTabId.value);
  if (handle) {
    if (raw) {
      handle.writeInput(text);
    } else {
      handle.sendData(text);
    }
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

function handleInputSend(text: string, raw = false) {
  if (!raw) {
    addToInputHistory(text.replace(/\n$/, '')); // Remove trailing newline for history
  }
  sendToActiveTerminal(text, raw);
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
    if (terminalSearchVisible.value && activeTabId.value === tabId) {
      void nextTick(() => {
        syncSearchToActiveTerminal("next", true);
      });
    }
    return;
  }
  termRefs.delete(tabId);
}

function fitActiveTerminal() {
  fitVisibleTerminals();
}

// Number of panes currently shown in the split layout — the candidate broadcast
// targets. Broadcast is only meaningful with at least two.
const broadcastTargetCount = computed(() => Object.keys(paneByTabId.value).length);

function toggleBroadcastInput() {
  if (broadcastTargetCount.value < 2) {
    return;
  }
  broadcastInput.value = !broadcastInput.value;
}

// Fan a focused pane's keystroke out to the other visible panes. writeInput on a
// target does not re-fire its onData, so this cannot loop back.
function handleBroadcastInput(sourceTabId: string, data: string) {
  if (!broadcastInput.value) {
    return;
  }
  for (const tabId of Object.keys(paneByTabId.value)) {
    if (tabId === sourceTabId) {
      continue;
    }
    termRefs.get(tabId)?.writeInput(data);
  }
}

// Closing/merging panes can drop the visible count below 2; silently disable
// broadcast so it can't keep firing into a single pane.
watch(broadcastTargetCount, (count) => {
  if (count < 2 && broadcastInput.value) {
    broadcastInput.value = false;
  }
});

watch(() => settings.value.showInputBar, () => {
  void nextTick(() => {
    fitActiveTerminal();
  });
});

watch(terminalSearchVisible, () => {
  void nextTick(() => {
    fitVisibleTerminals();
    if (terminalSearchVisible.value) {
      focusTerminalSearchInput(true);
    }
  });
});

watch(
  () => [
    terminalSearchQuery.value,
    terminalSearchOptions.value.caseSensitive,
    terminalSearchOptions.value.wholeWord,
    terminalSearchOptions.value.regex,
  ],
  () => {
    if (!terminalSearchVisible.value) {
      return;
    }
    if (searchDebounceTimer !== null) {
      window.clearTimeout(searchDebounceTimer);
    }
    searchDebounceTimer = window.setTimeout(() => {
      searchDebounceTimer = null;
      syncSearchToActiveTerminal("next", true);
    }, 300);
  },
);

// paneLayout is always replaced via `paneLayout.value = newTree` (see usePaneLayout.ts),
// including split-ratio drags, so shallow watch catches every structural change while
// skipping the deep traversal over the pane tree on every tab/split interaction.
watch([paneLayout, sidebarOpen, showRemoteFileManager], () => {
  void nextTick(() => {
    fitVisibleTerminals();
  });
});

// tabs is also replaced wholesale (`tabs.value = [...tabs.value, tab]` etc.).
// Dropping deep here is the main win: pane drag/reorder no longer walks the whole tree
// on every frame just to schedule a debounced save.
watch([tabs, paneLayout, focusedPaneId, activeTabId], () => {
  scheduleWorkspaceStatePersistence();
});

watch(focusedPaneId, () => {
  void nextTick(() => {
    focusActiveTerminal();
  });
});

watch(activeTabId, (newTabId, previousTabId) => {
  if (previousTabId && previousTabId !== newTabId) {
    termRefs.get(previousTabId)?.clearSearch();
  }

  if (!terminalSearchVisible.value || !newTabId) {
    return;
  }

  void nextTick(() => {
    syncSearchToActiveTerminal("next", true);
    focusTerminalSearchInput(false);
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

function handleOpenAbout() {
  closeOpenMenus();
  showAbout.value = true;
}

/** Open the online AuraTerm user manual (hosted on the AuraXLab site) in the
 *  system browser. */
const USER_MANUAL_URL = "https://auraxlab.com/products/auraterm/manual";
function handleOpenUserManual() {
  closeOpenMenus();
  void openExternalUrl(USER_MANUAL_URL);
}

function handleOpenSettings() {
  closeOpenMenus();
  showSettings.value = true;
}

function handleOpenCloudSync() {
  closeOpenMenus();
  showCloudSync.value = true;
}

function toggleRemoteFileManager() {
  if (!activeSshConfig.value) {
    return;
  }
  showRemoteFileManager.value = !showRemoteFileManager.value;
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

function handleNewLocalSession() {
  const newId = mintTabId();
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

  removeTerminalSearchResults(id);
  if (nextTabs.length === 0) {
    handleCloseTerminalSearch();
  }

  handleTabRemoved(id, nextTabs);

  if (id in serialConnectionStates.value) {
    const nextStates = { ...serialConnectionStates.value };
    delete nextStates[id];
    serialConnectionStates.value = nextStates;
  }

  if (id in tabTunnels.value) {
    const nextTunnels = { ...tabTunnels.value };
    delete nextTunnels[id];
    tabTunnels.value = nextTunnels;
  }
  if (showTunnelManager.value && activeTabId.value === id) {
    showTunnelManager.value = false;
  }
}

async function handleConnectResult(result: ConnectResult) {
  const newId = mintTabId();
  const savedConnectionId = result.saveAs ? crypto.randomUUID() : undefined;
  const session = buildSessionFromConnectResult(result, savedConnectionId);
  const tab = session
    ? createSessionTab(newId, session, result.saveAs, result.logPath)
    : null;

  if (!tab) {
    showConnectDialog.value = false;
    return;
  }

  tabs.value = [...tabs.value, tab];

  if (session?.protocol === "serial") {
    rememberSerialConfig(session.serialConfig);
    updateSerialConnectionState(newId, "connecting");
  }

  assignTabToFocusedPane(newId);
  showConnectDialog.value = false;

  if (!result.saveAs) {
    return;
  }

  // 查找已存在的同主机+端口+用户的连接，复用其 id 和 lastUsed 以更新而非重复追加
  let existingConn: SavedConnection | undefined;
  try {
    const existingConns = await invoke<SavedConnection[]>("get_connections");
    if (result.protocol === "ssh" && result.sshConfig) {
      const { host, port, user } = result.sshConfig;
      existingConn = existingConns.find(
        c => c.protocol === "ssh" && c.host === host && c.port === (port ?? 22) && c.user === user,
      );
    } else if (result.protocol === "telnet" && result.telnetConfig) {
      const { host, port } = result.telnetConfig;
      existingConn = existingConns.find(
        c => c.protocol === "telnet" && c.host === host && c.port === (port ?? 23),
      );
    } else if (result.protocol === "serial" && result.serialConfig) {
      existingConn = existingConns.find(
        c => c.protocol === "serial" && c.portName === result.serialConfig!.portName,
      );
    }
  } catch {
    // 查询失败时降级为新建
  }

  const connection = buildSavedConnectionFromConnectResult(
    result,
    existingConn?.id ?? savedConnectionId ?? crypto.randomUUID(),
    existingConn?.createdAt ?? Date.now(),
  );
  // 保留已有的 lastUsed，避免更新时丢失（否则连接会从 Recently Used 消失）
  if (existingConn?.lastUsed) {
    connection.lastUsed = existingConn.lastUsed;
  }
  // 保留已配置的端口转发隧道，避免通过连接对话框重连时被覆盖丢失
  if (existingConn?.tunnels?.length) {
    connection.tunnels = existingConn.tunnels;
  }

  try {
    await invoke("save_connection", { connection });
    sidebarExpandGroup.value = connection.group?.trim() || "Ungrouped";
    sidebarRefreshToken.value += 1;
    sidebarOpen.value = true;
  } catch (error) {
    console.error("Failed to save connection", error);
  }
}

function handleBookmarkConnect(connection: SavedConnection) {
  const newId = mintTabId();
  const session = buildSessionFromSavedConnection(connection);
  const tab = createSessionTab(newId, session, connection.name, connection.logPath);

  if (session.protocol === "serial") {
    rememberSerialConfig(session.serialConfig);
    updateSerialConnectionState(newId, "connecting");
  }

  if (session.protocol === "ssh" && connection.tunnels?.length) {
    tabTunnels.value = { ...tabTunnels.value, [newId]: connection.tunnels.map((tunnel) => ({ ...tunnel })) };
  }

  tabs.value = [...tabs.value, tab];
  assignTabToFocusedPane(newId);
}

const activeTabTunnels = computed<TunnelConfig[]>(() => (
  activeTabId.value ? tabTunnels.value[activeTabId.value] ?? [] : []
));

async function handleToggleTunnelManager() {
  closeOpenMenus();
  if (!activeSshConfig.value || !activeTabId.value) {
    return;
  }
  if (showTunnelManager.value) {
    showTunnelManager.value = false;
    return;
  }

  const tabId = activeTabId.value;
  // Lazily hydrate tunnels from the bookmark for sessions restored on startup
  // (where tabTunnels was never seeded from a live connect).
  if (!tabTunnels.value[tabId] && activeSshConfig.value.savedConnectionId) {
    try {
      const connections = await invoke<SavedConnection[]>("get_connections");
      const match = connections.find((connection) => connection.id === activeSshConfig.value?.savedConnectionId);
      if (match?.tunnels?.length) {
        tabTunnels.value = { ...tabTunnels.value, [tabId]: match.tunnels.map((tunnel) => ({ ...tunnel })) };
      }
    } catch (error) {
      console.error("Failed to load tunnels for session", error);
    }
  }
  showTunnelManager.value = true;
}

async function handleUpdateTunnels(nextTunnels: TunnelConfig[]) {
  const tabId = activeTabId.value;
  if (!tabId) {
    return;
  }
  tabTunnels.value = { ...tabTunnels.value, [tabId]: nextTunnels };

  // Persist to the underlying bookmark, if this session came from one.
  const savedConnectionId = activeSshConfig.value?.savedConnectionId;
  if (!savedConnectionId) {
    return;
  }
  try {
    const connections = await invoke<SavedConnection[]>("get_connections");
    const target = connections.find((connection) => connection.id === savedConnectionId);
    if (target) {
      await invoke("save_connection", { connection: { ...target, tunnels: nextTunnels } });
      sidebarRefreshToken.value += 1;
    }
  } catch (error) {
    console.error("Failed to persist tunnels to bookmark", error);
  }
}

function handleSshConnectedForTab(tabId: string) {
  const configured = tabTunnels.value[tabId];
  if (configured?.length) {
    void tunnels.autoStartTunnels(tabId, configured);
  }
  if (settingsRef.value.autoOpenSftp && activeTabId.value === tabId) {
    showRemoteFileManager.value = true;
  }
}

async function handleToggleCommandPalette() {
  closeOpenMenus();
  if (showCommandPalette.value) {
    showCommandPalette.value = false;
    return;
  }
  try {
    paletteBookmarks.value = await invoke<SavedConnection[]>("get_connections");
  } catch {
    paletteBookmarks.value = [];
  }
  showCommandPalette.value = true;
}

const paletteCommands = computed<PaletteCommand[]>(() => {
  const hasTab = Boolean(activeTab.value);
  const hasSsh = Boolean(activeSshConfig.value);
  const commands: PaletteCommand[] = [
    { id: "new-local", title: t("palette.cmd.newLocal"), group: t("palette.groups.session"), keywords: "terminal shell", run: () => handleNewLocalSession() },
    { id: "new-ssh", title: t("palette.cmd.newSsh"), group: t("palette.groups.session"), keywords: "remote", run: () => openConnect("ssh") },
    { id: "new-telnet", title: t("palette.cmd.newTelnet"), group: t("palette.groups.session"), run: () => openConnect("telnet") },
    { id: "new-serial", title: t("palette.cmd.newSerial"), group: t("palette.groups.session"), keywords: "port com", run: () => openConnect("serial") },
    { id: "close-tab", title: t("menu.closeTab"), group: t("palette.groups.session"), enabled: hasTab, run: () => handleCloseActiveTab() },
    { id: "toggle-bookmarks", title: sidebarOpen.value ? t("menu.hideBookmarks") : t("menu.showBookmarks"), group: t("palette.groups.view"), keywords: "sidebar connections", run: () => handleToggleBookmarks() },
    { id: "tunnels", title: t("palette.cmd.tunnels"), group: t("palette.groups.tools"), keywords: "tunnel forward socks proxy -L -R -D", enabled: hasSsh, run: () => { void handleToggleTunnelManager(); } },
    { id: "remote-files", title: showRemoteFileManager.value ? t("menu.hideRemoteFiles") : t("menu.showRemoteFiles"), group: t("palette.groups.tools"), keywords: "sftp scp files", enabled: hasSsh, run: () => handleToggleRemoteFileManager() },
    { id: "find", title: t("palette.cmd.find"), group: t("palette.groups.view"), keywords: "search", enabled: hasTab, run: () => handleOpenTerminalSearch() },
    { id: "command-previous", title: t("palette.cmd.prevCommand"), subtitle: "Ctrl/Cmd+Shift+Up", group: t("palette.groups.terminal"), keywords: "shell navigation osc 133", enabled: hasTab, run: () => { termRefs.get(activeTabId.value)?.previousCommand(); } },
    { id: "command-next", title: t("palette.cmd.nextCommand"), subtitle: "Ctrl/Cmd+Shift+Down", group: t("palette.groups.terminal"), keywords: "shell navigation osc 133", enabled: hasTab, run: () => { termRefs.get(activeTabId.value)?.nextCommand(); } },
    { id: "command-rerun", title: t("palette.cmd.rerunCommand"), group: t("palette.groups.terminal"), keywords: "shell repeat osc 133", enabled: hasTab, run: () => { termRefs.get(activeTabId.value)?.rerunLastCommand(); } },
    { id: "command-copy", title: t("palette.cmd.copyCommand"), group: t("palette.groups.terminal"), keywords: "shell clipboard osc 133", enabled: hasTab, run: () => { void termRefs.get(activeTabId.value)?.copyLastCommand(); } },
    { id: "split-right", title: t("menu.splitRight"), group: t("palette.groups.layout"), keywords: "pane vertical", run: () => handleSplitPane("vertical") },
    { id: "split-down", title: t("menu.splitDown"), group: t("palette.groups.layout"), keywords: "pane horizontal", run: () => handleSplitPane("horizontal") },
    { id: "close-pane", title: t("menu.closePane"), group: t("palette.groups.layout"), enabled: paneLeaves.value.length > 1, run: () => handleClosePane() },
    { id: "font-increase", title: t("menu.increaseFontSize"), group: t("palette.groups.view"), keywords: "zoom", run: () => handleIncreaseTerminalFontSize() },
    { id: "font-decrease", title: t("menu.decreaseFontSize"), group: t("palette.groups.view"), keywords: "zoom", run: () => handleDecreaseTerminalFontSize() },
    { id: "font-reset", title: t("menu.resetFontSize"), group: t("palette.groups.view"), keywords: "zoom", run: () => handleResetTerminalFontSize() },
    { id: "fullscreen", title: t("palette.cmd.fullscreen"), group: t("palette.groups.view"), run: () => { void handleToggleFullScreen(); } },
    { id: "settings", title: t("palette.cmd.settings"), group: t("palette.groups.app"), keywords: "preferences config", run: () => handleOpenSettings() },
    { id: "cloud-sync", title: t("palette.cmd.cloudSync"), group: t("palette.groups.app"), keywords: "sync backup gist gitee webdav e2e encrypt bookmarks", run: () => { showCloudSync.value = true; } },
    { id: "user-manual", title: t("menu.userManual"), group: t("palette.groups.app"), keywords: "help docs documentation guide manual", run: () => handleOpenUserManual() },
    { id: "about", title: t("menu.about"), group: t("palette.groups.app"), run: () => handleOpenAbout() },
  ];

  for (const bookmark of paletteBookmarks.value) {
    const protocol = bookmark.protocol ?? "ssh";
    const detail = protocol === "serial"
      ? bookmark.portName ?? ""
      : `${protocol === "ssh" && bookmark.user ? `${bookmark.user}@` : ""}${bookmark.host}${bookmark.port ? `:${bookmark.port}` : ""}`;
    commands.push({
      id: `bookmark-${bookmark.id}`,
      title: t("palette.cmd.connectBookmark", { name: bookmark.name }),
      subtitle: detail,
      group: t("palette.groups.bookmark"),
      keywords: `${bookmark.group ?? ""} ${bookmark.host} ${protocol}`,
      run: () => handleBookmarkConnect(bookmark),
    });
  }

  return commands;
});
</script>

<template>
  <div :class="appClassName">
    <div v-if="isMainWindow" class="titlebar" @mousedown="handleTitlebarMouseDown" @dblclick="handleToggleMaximize">
      <div v-if="isMainWindow && osType !== 'windows'" class="titlebar-controls" :aria-label="$t('titlebar.windowControls')" data-no-drag="true">
        <button
          class="titlebar-control-btn titlebar-control-close"
          type="button"
          :aria-label="$t('titlebar.close')"
          @mousedown="stopDragPropagation"
          @click="handleClose"
        />
        <button
          class="titlebar-control-btn titlebar-control-minimize"
          type="button"
          :aria-label="$t('titlebar.minimize')"
          @mousedown="stopDragPropagation"
          @click="handleMinimize"
        />
        <button
          class="titlebar-control-btn titlebar-control-maximize"
          type="button"
          :aria-label="$t('titlebar.maximize')"
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
              {{ $t('menu.file') }}
            </button>
            <div v-if="openMenuId === 'file'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <div
                class="titlebar-menu-submenu-group"
                @mouseenter="openFileSubmenuId = 'new-session'"
              >
                <button class="titlebar-menu-item titlebar-menu-item--submenu" type="button" @click="toggleFileSubmenu('new-session')">
                  <span>{{ $t('menu.newSession') }}</span>
                  <span class="titlebar-menu-item-arrow">›</span>
                </button>
                <div v-if="openFileSubmenuId === 'new-session'" class="titlebar-menu-submenu" @mousedown="stopDragPropagation">
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleNewLocalSessionFromMenu">
                    <span>{{ $t('menu.localShell') }}</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('ssh')">
                    <span>{{ $t('menu.ssh') }}</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('telnet')">
                    <span>{{ $t('menu.telnet') }}</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenConnectionFromMenu('serial')">
                    <span>{{ $t('menu.serial') }}</span>
                  </button>
                </div>
              </div>
              <button class="titlebar-menu-item" type="button" :disabled="!activeTab" @click="handleCloseActiveTab">
                <span>{{ $t('menu.closeTab') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+W</span>
              </button>
              <div class="titlebar-menu-separator" />
              <div
                class="titlebar-menu-submenu-group"
                @mouseenter="openFileSubmenuId = 'preferences'"
              >
                <button class="titlebar-menu-item titlebar-menu-item--submenu" type="button" @click="toggleFileSubmenu('preferences')">
                  <span>{{ $t('menu.preferences') }}</span>
                  <span class="titlebar-menu-item-arrow">›</span>
                </button>
                <div v-if="openFileSubmenuId === 'preferences'" class="titlebar-menu-submenu" @mousedown="stopDragPropagation">
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenSettings">
                    <span>{{ $t('menu.settings') }}</span>
                  </button>
                  <button class="titlebar-menu-item" type="button" @mousedown.stop="stopDragPropagation" @click="handleOpenCloudSync">
                    <span>{{ $t('menu.cloudSync') }}</span>
                  </button>
                </div>
              </div>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleExitApp">
                <span>{{ $t('menu.exit') }}</span>
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
              {{ $t('menu.view') }}
            </button>
            <div v-if="openMenuId === 'view'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" :disabled="!activeTab" @click="handleOpenTerminalSearch">
                <span>{{ $t('menu.findInTerminal') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+F</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleToggleBookmarks">
                <span>{{ sidebarOpen ? $t('menu.hideBookmarks') : $t('menu.showBookmarks') }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleToggleCommandPalette">
                <span>{{ $t('menu.commandPalette') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+Shift+P</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleSplitPaneFromView('vertical')">
                <span>{{ $t('menu.splitRight') }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleSplitPaneFromView('horizontal')">
                <span>{{ $t('menu.splitDown') }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" :disabled="paneLeaves.length <= 1" @click="handleClosePaneFromView">
                <span>{{ $t('menu.closePane') }}</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleIncreaseTerminalFontSize">
                <span>{{ $t('menu.increaseFontSize') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}++</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleDecreaseTerminalFontSize">
                <span>{{ $t('menu.decreaseFontSize') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+-</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleResetTerminalFontSize">
                <span>{{ $t('menu.resetFontSize') }}</span>
                <span class="titlebar-menu-item-hint">{{ primaryShortcutLabel }}+0</span>
              </button>
              <div class="titlebar-menu-separator" />
              <button class="titlebar-menu-item" type="button" @click="handleToggleFullScreen">
                <span>{{ isFullscreen ? $t('menu.exitFullScreen') : $t('menu.fullScreen') }}</span>
                <span class="titlebar-menu-item-hint">F11</span>
              </button>
            </div>
          </div>

          <div class="titlebar-menu-group" @mouseenter="openMenuId ? (openMenuId = 'tools') : null">
            <button
              class="titlebar-menu-btn"
              :class="{ open: openMenuId === 'tools' }"
              type="button"
              @mousedown="stopDragPropagation"
              @click="toggleMenu('tools')"
            >
              {{ $t('menu.tools') }}
            </button>
            <div v-if="openMenuId === 'tools'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" :disabled="!activeSshConfig" @click="handleToggleRemoteFileManager">
                <span>{{ showRemoteFileManager ? $t('menu.hideRemoteFiles') : $t('menu.showRemoteFiles') }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" :disabled="!activeSshConfig" @click="handleToggleTunnelManager">
                <span>{{ $t('menu.portForwarding') }}</span>
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
              {{ $t('menu.help') }}
            </button>
            <div v-if="openMenuId === 'help'" class="titlebar-menu-dropdown" @mousedown="stopDragPropagation">
              <button class="titlebar-menu-item" type="button" @click="handleOpenUserManual">
                <span>{{ $t('menu.userManual') }}</span>
              </button>
              <button class="titlebar-menu-item" type="button" @click="handleOpenAbout">
                <span>{{ $t('menu.about') }}</span>
              </button>
            </div>
          </div>
        </div>
        <div class="titlebar-title center">AuraTerm</div>
      </div>

      <div v-else class="titlebar-title">AuraTerm</div>

      <div v-if="isMainWindow && osType === 'windows'" class="titlebar-controls-win" :aria-label="$t('titlebar.windowControls')" data-no-drag="true">
        <button
          class="titlebar-control-win-btn"
          type="button"
          :aria-label="$t('titlebar.minimize')"
          @mousedown="stopDragPropagation"
          @click="handleMinimize"
        >
          &#xE921;
        </button>
        <button
          class="titlebar-control-win-btn"
          type="button"
          :aria-label="$t('titlebar.maximize')"
          @mousedown="stopDragPropagation"
          @click="handleToggleMaximize"
        >
          &#xE922;
        </button>
        <button
          class="titlebar-control-win-btn close"
          type="button"
          :aria-label="$t('titlebar.close')"
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
          :class="{
            active: activeTabId === tab.id,
            dragging: draggedTabId === tab.id,
            'pane-visible': visibleNonFocusedTabIds.has(tab.id)
          }"
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
          <!-- Step 4: 多 pane 可见性指示点 -->
          <span v-if="visibleNonFocusedTabIds.has(tab.id)" class="tab-pane-dot" title="Visible in another pane" />
          <button class="tab-close-btn" title="Close Tab" @click.stop="handleCloseTab(tab.id)">
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
              <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
              <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
            </svg>
          </button>
        </div>

        <button key="__newtab__" class="tab-new-btn" type="button" title="New Tab" @mousedown.stop @click="handleOpenNewTabMenu">+</button>
      </TransitionGroup>

      <div class="tab-bar-actions">
        <!-- 搜索组 -->
        <div class="tab-bar-action-group">
          <button
            class="tab-new-btn tab-search-btn"
            :class="{ active: terminalSearchVisible }"
            type="button"
            title="Find in Terminal (Ctrl+F)"
            :disabled="!activeTab"
            @mousedown.stop
            @click.stop="handleOpenTerminalSearch"
          >
            <!-- Step 2: SVG 搜索图标 -->
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <circle cx="6.5" cy="6.5" r="4.5" stroke="currentColor" stroke-width="1.4"/>
              <line x1="10" y1="10" x2="14" y2="14" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/>
            </svg>
          </button>
        </div>
        <!-- Step 1: 分组分隔线 -->
        <div class="tab-bar-action-divider" />
        <!-- 分栏操作组 (Step 3: Layout 下拉菜单) -->
        <!-- layoutMenuRef 绑在外层容器，用于检测点击外部关闭菜单 -->
        <div ref="layoutMenuRef" class="tab-bar-action-group tab-layout-group">
          <button
            class="tab-new-btn tab-layout-btn"
            :class="{ active: showLayoutMenu }"
            type="button"
            title="Layout"
            @mousedown.stop
            @click.stop="toggleLayoutMenu"
          >
            <!-- Step 2: SVG 分栏图标 -->
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <rect x="1" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
              <rect x="9" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
            </svg>
            <svg width="8" height="8" viewBox="0 0 8 8" fill="currentColor" style="margin-left: 2px; opacity: 0.6;">
              <path d="M1 2.5L4 5.5L7 2.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
            </svg>
          </button>
        </div>
        <!-- Step 1: 分组分隔线 -->
        <div class="tab-bar-action-divider" />
        <!-- 工具组 -->
        <div class="tab-bar-action-group">
          <button
            class="tab-new-btn tab-broadcast-btn"
            :class="{ active: broadcastInput }"
            type="button"
            :disabled="broadcastTargetCount < 2"
            :title="broadcastTargetCount < 2
              ? 'Broadcast input (split into 2+ panes to enable)'
              : broadcastInput
                ? `Broadcast ON — input goes to all ${broadcastTargetCount} visible panes`
                : 'Broadcast input to all visible panes'"
            @mousedown.stop
            @click.stop="toggleBroadcastInput"
          >
            <!-- 广播波纹图标 -->
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <circle cx="8" cy="8" r="1.6" fill="currentColor" />
              <path d="M5 5a4.3 4.3 0 000 6M11 5a4.3 4.3 0 010 6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
              <path d="M3 3a7.1 7.1 0 000 10M13 3a7.1 7.1 0 010 10" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" />
            </svg>
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
            <!-- Step 2: SVG 文件图标 -->
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <path d="M3 2h6l4 4v9a1 1 0 01-1 1H3a1 1 0 01-1-1V3a1 1 0 011-1z" stroke="currentColor" stroke-width="1.3"/>
              <path d="M9 2v4h4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
            </svg>
          </button>
          <button class="tab-new-btn tab-settings-btn" type="button" title="Settings" @mousedown.stop @click.stop="handleOpenSettings">
            <!-- Step 2: SVG 设置图标 -->
            <svg width="15" height="15" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
              <circle cx="8" cy="8" r="2.5" stroke="currentColor" stroke-width="1.3"/>
              <path d="M8 1v2M8 13v2M1 8h2M13 8h2M2.93 2.93l1.41 1.41M11.66 11.66l1.41 1.41M2.93 13.07l1.41-1.41M11.66 4.34l1.41-1.41" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
            </svg>
          </button>
        </div>
      </div>
    </div>

    <div
      v-if="tabContextMenu"
      ref="tabContextMenuRef"
      class="tab-context-menu"
      :style="{ top: `${tabContextMenu.y}px`, left: `${tabContextMenu.x}px` }"
    >
      <button class="tab-context-item" type="button" @click="handleSplitTabFromContextMenu('vertical')">{{ $t('menu.splitRight') }}</button>
      <button class="tab-context-item" type="button" @click="handleSplitTabFromContextMenu('horizontal')">{{ $t('menu.splitDown') }}</button>
      <button class="tab-context-item" type="button" @click="handleMoveTabToFocusedPaneFromContextMenu">{{ $t('menu.moveToFocusedPane') }}</button>
      <button class="tab-context-item" type="button" :disabled="paneLeaves.length <= 1" @click="handleClosePaneFromContextMenu">{{ $t('menu.closePane') }}</button>
      <div class="titlebar-menu-separator" />
      <button class="tab-context-item" type="button" @click="handleRenameTabFromContextMenu">{{ $t('menu.renameTab') }}</button>
    </div>

    <!-- Layout 下拉菜单：position:fixed 渲染在根级，避免被终端层遮挡 -->
    <!-- @pointerdown.stop 阻止 pointerdown 冒泡到 document，防止 watcher 提前关闭菜单 -->
    <div
      v-if="showLayoutMenu"
      class="layout-dropdown layout-dropdown--fixed"
      :style="{ top: `${layoutMenuPos.top}px`, right: `${layoutMenuPos.right}px` }"
      @pointerdown.stop
    >
      <button class="layout-dropdown-item" type="button" @click="showLayoutMenu = false; handleSplitPane('vertical')">
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect x="1" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
          <rect x="9" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
        </svg>
        <span>{{ $t('menu.splitRight') }}</span>
      </button>
      <button class="layout-dropdown-item" type="button" @click="showLayoutMenu = false; handleSplitPane('horizontal')">
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect x="1" y="1" width="14" height="6" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
          <rect x="1" y="9" width="14" height="6" rx="1.5" stroke="currentColor" stroke-width="1.3"/>
        </svg>
        <span>{{ $t('menu.splitDown') }}</span>
      </button>
      <div class="layout-dropdown-separator" />
      <button class="layout-dropdown-item" type="button" :disabled="paneLeaves.length <= 1" @click="showLayoutMenu = false; handleClosePane()">
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
          <rect x="1" y="1" width="14" height="14" rx="2" stroke="currentColor" stroke-width="1.3"/>
          <line x1="5" y1="5" x2="11" y2="11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
          <line x1="11" y1="5" x2="5" y2="11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
        </svg>
        <span>{{ $t('menu.closePane') }}</span>
      </button>
    </div>

    <div class="workspace">
      <BookmarkSidebar v-if="sidebarOpen" :refresh-token="sidebarRefreshToken" :expand-group="sidebarExpandGroup" :settings="settings" @connect="handleBookmarkConnect" />

      <div class="terminal-wrapper">
        <div v-if="terminalSearchVisible" class="terminal-searchbar">
          <div class="terminal-search-input-wrap">
            <span class="terminal-search-label">Find</span>
            <input
              ref="terminalSearchInputRef"
              v-model="terminalSearchQuery"
              class="terminal-search-input"
              type="text"
              placeholder="Search active terminal"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              @blur="handleTerminalSearchBlur"
              @keydown="handleTerminalSearchKeyDown"
            >
          </div>
          <div class="terminal-search-summary">{{ terminalSearchSummary }}</div>
          <div class="terminal-search-toggles">
            <button
              class="terminal-search-toggle"
              :class="{ active: terminalSearchOptions.caseSensitive }"
              type="button"
              title="Match Case"
              @click="toggleTerminalSearchOption('caseSensitive')"
            >Aa</button>
            <button
              class="terminal-search-toggle"
              :class="{ active: terminalSearchOptions.wholeWord }"
              type="button"
              title="Match Whole Word"
              @click="toggleTerminalSearchOption('wholeWord')"
            >W</button>
            <button
              class="terminal-search-toggle"
              :class="{ active: terminalSearchOptions.regex }"
              type="button"
              title="Use Regular Expression"
              @click="toggleTerminalSearchOption('regex')"
            >.*</button>
          </div>
          <div class="terminal-search-actions">
            <button class="terminal-search-action" type="button" :disabled="!terminalSearchQuery" title="Previous Match" @click="handleFindPreviousInTerminal">↑</button>
            <button class="terminal-search-action" type="button" :disabled="!terminalSearchQuery" title="Next Match" @click="handleFindNextInTerminal">↓</button>
            <button class="terminal-search-action close" type="button" title="Close Search" @click="handleCloseTerminalSearch">×</button>
          </div>
        </div>

        <div ref="terminalContainerRef" class="terminal-container split-terminal-container">
          <div v-if="tabs.length === 0" class="terminal-empty-state">
            No open tabs. Click + to open a new tab.
          </div>

          <!-- pane-frame-layer: 只渲染边框/背景，pointer-events:none 不拦截交互 -->
          <div v-else class="pane-frame-layer">
            <div
              v-for="pane in paneLeaves"
              :key="pane.paneId"
              class="terminal-pane-frame"
              :class="{ focused: isPaneFocused(pane.paneId), empty: !pane.tabId }"
              :style="getPaneShellStyle(pane.rect)"
              :data-pane-id="pane.paneId"
            >
              <!-- Step 6: Drop preview 增加文字提示 -->
              <div
                v-if="dropTargetPaneId === pane.paneId && dropTargetPosition"
                class="terminal-pane-drop-preview"
                :class="dropTargetPosition"
              >
                <span class="terminal-pane-drop-label">
                  {{ dropTargetPosition === 'center' ? 'Move here' : dropTargetPosition === 'left' ? 'Split left' : dropTargetPosition === 'right' ? 'Split right' : dropTargetPosition === 'top' ? 'Split above' : 'Split below' }}
                </span>
              </div>
            </div>
          </div>

          <div v-if="tabs.length > 0" class="terminal-instance-layer">
            <div
              v-for="tab in tabs"
              :key="tab.id"
              :ref="(instance) => setPaneViewportRef(tab.id, instance as Element | null)"
              class="terminal-instance-shell"
              :class="{
                visible: Boolean(paneByTabId[tab.id]),
                focused: activeTabId === tab.id,
                'broadcast-target': broadcastInput && Boolean(paneByTabId[tab.id]),
              }"
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
                :broadcast="broadcastInput"
                @search-results-change="(results) => updateTerminalSearchResults(tab.id, results)"
                @session-update="(session) => updateTabSession(tab.id, session)"
                @ssh-password-updated="sidebarRefreshToken += 1"
                @serial-connection-state-change="(state) => updateSerialConnectionState(tab.id, state)"
                @broadcast-input="(data) => handleBroadcastInput(tab.id, data)"
                @ssh-connected="handleSshConnectedForTab(tab.id)"
              />
            </div>
          </div>

          <!-- pane-overlay-layer: z-index:3, 高于 terminal-instance-layer(2)，包含 header/empty 交互内容 -->
          <div v-if="tabs.length > 0" class="pane-overlay-layer">
            <div
              v-for="pane in paneLeaves"
              :key="pane.paneId"
              class="pane-overlay-shell"
              :style="getPaneShellStyle(pane.rect)"
            >
              <!-- Step 7: Pane header 使用 Transition 动画，单→多 pane 时 slide-down 过渡 -->
              <Transition name="pane-header-slide">
                <div
                  v-if="isMainWindow && paneLeaves.length > 1"
                  class="terminal-pane-header"
                  @pointerdown="handlePaneHeaderPointerDown($event, pane.paneId)"
                  @pointermove="handlePaneHeaderPointerMove($event, pane.paneId)"
                  @pointerup="handlePaneHeaderPointerUp($event, pane.paneId)"
                  @pointercancel="handlePaneHeaderPointerCancel(pane.paneId)"
                >
                  <div class="terminal-pane-drag-handle" aria-hidden="true">
                    <svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor" xmlns="http://www.w3.org/2000/svg" style="opacity:0.35">
                      <circle cx="2" cy="2" r="1.2"/><circle cx="5" cy="2" r="1.2"/><circle cx="8" cy="2" r="1.2"/>
                      <circle cx="2" cy="5" r="1.2"/><circle cx="5" cy="5" r="1.2"/><circle cx="8" cy="5" r="1.2"/>
                      <circle cx="2" cy="8" r="1.2"/><circle cx="5" cy="8" r="1.2"/><circle cx="8" cy="8" r="1.2"/>
                    </svg>
                  </div>
                  <div class="terminal-pane-header-meta" @mousedown="focusPane(pane.paneId)">
                    <span class="terminal-pane-title">{{ getTabTitle(pane.tabId) }}</span>
                    <span class="terminal-pane-protocol">{{ getTabProtocolLabel(pane.tabId) }}</span>
                  </div>
                  <div class="terminal-pane-actions">
                    <button type="button" title="Split Right" @click.stop="focusPane(pane.paneId); handleSplitPane('vertical', pane.paneId)">
                      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <rect x="1" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.5"/>
                        <rect x="9" y="1" width="6" height="14" rx="1.5" stroke="currentColor" stroke-width="1.5"/>
                      </svg>
                    </button>
                    <button type="button" title="Split Down" @click.stop="focusPane(pane.paneId); handleSplitPane('horizontal', pane.paneId)">
                      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <rect x="1" y="1" width="14" height="6" rx="1.5" stroke="currentColor" stroke-width="1.5"/>
                        <rect x="1" y="9" width="14" height="6" rx="1.5" stroke="currentColor" stroke-width="1.5"/>
                      </svg>
                    </button>
                    <button type="button" title="Close Pane" :disabled="paneLeaves.length <= 1" @click.stop="focusPane(pane.paneId); handleClosePane(pane.paneId)">
                      <svg width="12" height="12" viewBox="0 0 10 10" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                        <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                      </svg>
                    </button>
                  </div>
                </div>
              </Transition>

              <!-- Step 5: 空 Pane 快捷操作 -->
              <div
                v-if="!pane.tabId"
                class="terminal-pane-empty"
                :class="{ 'drag-target': hoveredEmptyPaneId === pane.paneId }"
                :data-pane-id="pane.paneId"
                @mousedown="focusPane(pane.paneId)"
              >
                <template v-if="draggedTabId">
                  <div class="terminal-pane-empty-title">Drop Here</div>
                  <div class="terminal-pane-empty-desc">Release to show this tab in the pane.</div>
                </template>
                <template v-else>
                  <div class="terminal-pane-empty-title">Empty Pane</div>
                  <div class="terminal-pane-empty-desc">Start a new session or drag a tab here.</div>
                  <div class="terminal-pane-empty-actions">
                    <button class="terminal-pane-empty-btn" type="button" @click.stop="focusPane(pane.paneId); handleNewLocalSession()">
                      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <rect x="1" y="2" width="14" height="11" rx="2" stroke="currentColor" stroke-width="1.3"/>
                        <path d="M4 6l3 3-3 3" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
                        <line x1="9" y1="11" x2="12" y2="11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
                      </svg>
                      New Shell
                    </button>
                    <button class="terminal-pane-empty-btn" type="button" @click.stop="focusPane(pane.paneId); openConnect('ssh')">
                      <svg width="13" height="13" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <circle cx="8" cy="8" r="6.5" stroke="currentColor" stroke-width="1.3"/>
                        <path d="M8 1.5C8 1.5 5.5 4 5.5 8s2.5 6.5 2.5 6.5M8 1.5C8 1.5 10.5 4 10.5 8S8 14.5 8 14.5M1.5 8h13" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>
                      </svg>
                      Connect
                    </button>
                    <button class="terminal-pane-empty-btn terminal-pane-empty-btn--danger" type="button" @click.stop="handleClosePane(pane.paneId)">
                      <svg width="13" height="13" viewBox="0 0 10 10" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
                        <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                        <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                      </svg>
                      Close Pane
                    </button>
                  </div>
                </template>
              </div>
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
          :active-host="activeSshConfig?.host"
          :session-group="activeSshConfig?.savedConnectionGroup"
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

    <TunnelManager
      v-if="showTunnelManager && activeTab && activeSshConfig"
      :key="`tunnels-${activeTab.id}`"
      :session-id="activeTab.id"
      :ssh-config="activeSshConfig"
      :tunnels="activeTabTunnels"
      :api="tunnels"
      @update-tunnels="handleUpdateTunnels"
      @close="showTunnelManager = false"
    />

    <CommandPalette
      v-if="showCommandPalette"
      :commands="paletteCommands"
      @close="showCommandPalette = false"
    />

    <MasterPasswordDialog
      :is-open="showMasterPasswordDialog"
      :mode="masterPasswordDialogMode"
      :remember-available="masterPasswordRememberAvailable"
      @success="handleMasterPasswordSuccess"
      @unlocked="handleMasterPasswordUnlocked"
      @cancel="handleMasterPasswordCancel"
    />

    <SettingsDialog v-if="showSettings" :initial="settings" @save="handleSaveSettings" @cancel="showSettings = false" />
    <CloudSyncDialog v-if="showCloudSync" @close="showCloudSync = false" />
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
