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
import {
  DEFAULT_SETTINGS,
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

interface Tab {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}

interface TabContextMenuState {
  x: number;
  y: number;
  tabId: string;
}

type PaneAxis = "horizontal" | "vertical";

interface PaneLeafNode {
  kind: "leaf";
  paneId: string;
  tabId: string | null;
}

interface PaneSplitNode {
  kind: "split";
  splitId: string;
  axis: PaneAxis;
  ratio: number;
  first: PaneNode;
  second: PaneNode;
}

type PaneNode = PaneLeafNode | PaneSplitNode;

interface PaneRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

interface PaneLeafLayout {
  paneId: string;
  tabId: string | null;
  rect: PaneRect;
}

interface PaneSplitHandleLayout {
  splitId: string;
  axis: PaneAxis;
  parentRect: PaneRect;
  position: number;
}

interface RemovePaneResult {
  node: PaneNode | null;
  removed: boolean;
  fallbackPaneId: string | null;
}

interface PersistedPaneLayoutState {
  root: PaneNode;
  focusedPaneId: string | null;
}

interface PersistedTabSnapshot {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}

interface PersistedWorkspaceState {
  version: 1;
  tabs: PersistedTabSnapshot[];
  paneLayout: PaneNode;
  focusedPaneId: string | null;
  activeTabId: string | null;
}

type AppMenuId = "file" | "view" | "help";
type FileSubmenuId = "new-session" | "preferences";

let nextTabId = 1;
let nextPaneId = 1;
let nextSplitId = 1;
const PANE_HEADER_HEIGHT = 30;
const PANE_INSET = 4;
const MIN_PANE_RATIO = 0.15;
const MAX_PANE_RATIO = 0.85;

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
const paneLayout = ref<PaneNode>({
  kind: "leaf",
  paneId: "pane-0",
  tabId: null,
});
const focusedPaneId = ref("pane-0");
const activeTabId = ref("");
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
const hoveredEmptyPaneId = ref<string | null>(null);
const renamingTabId = ref<string | null>(null);
const renamingTabTitle = ref("");
const tabContextMenu = ref<TabContextMenuState | null>(null);
const suppressTabClick = ref(false);
let pendingTabDrag: { tabId: string; startX: number; startY: number } | null = null;
let tabDragMoved = false;
const settingsRef = ref<AppSettings>(DEFAULT_SETTINGS);
const menuBarRef = ref<HTMLDivElement | null>(null);
const tabContextMenuRef = ref<HTMLDivElement | null>(null);
const terminalContainerRef = ref<HTMLDivElement | null>(null);
const isFullscreen = ref(false);
const termRefs = new Map<string, TerminalHandle>();
const paneViewportRefs = new Map<string, HTMLElement>();
const cleanupFns: Array<() => void> = [];
let paneResizeObserver: ResizeObserver | null = null;
let pendingFitFrame: number | null = null;
const pendingFitTabIds = new Set<string>();
let cleanupPaneResizeTracking: (() => void) | null = null;
let persistWorkspaceStateTimer: number | null = null;
const hasLoadedSettings = ref(false);

function createDefaultPaneLayoutState(sourceTabs: Tab[]): PersistedPaneLayoutState {
  return {
    root: {
      kind: "leaf",
      paneId: "pane-0",
      tabId: sourceTabs[0]?.id ?? null,
    },
    focusedPaneId: "pane-0",
  };
}

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
    maxTabIndex = Math.max(maxTabIndex, parseSequenceId(item.id, "tab-") ?? -1);
  }

  nextTabId = Math.max(nextTabId, maxTabIndex + 1);
}

function createPaneId() {
  return `pane-${nextPaneId++}`;
}

function createSplitId() {
  return `split-${nextSplitId++}`;
}

function parseSequenceId(value: string | null | undefined, prefix: string) {
  if (!value || !value.startsWith(prefix)) {
    return null;
  }

  const numericValue = Number.parseInt(value.slice(prefix.length), 10);
  return Number.isFinite(numericValue) ? numericValue : null;
}

function syncPaneIdCounters(node: PaneNode) {
  let maxPaneIndex = -1;
  let maxSplitIndex = -1;

  const walk = (current: PaneNode) => {
    if (current.kind === "leaf") {
      maxPaneIndex = Math.max(maxPaneIndex, parseSequenceId(current.paneId, "pane-") ?? -1);
      return;
    }

    maxSplitIndex = Math.max(maxSplitIndex, parseSequenceId(current.splitId, "split-") ?? -1);
    walk(current.first);
    walk(current.second);
  };

  walk(node);
  nextPaneId = Math.max(nextPaneId, maxPaneIndex + 1);
  nextSplitId = Math.max(nextSplitId, maxSplitIndex + 1);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function restoreSessionConfig(value: unknown): SessionConfig | null {
  if (!isRecord(value) || typeof value.protocol !== "string") {
    return null;
  }

  if (value.protocol === "local") {
    return {
      protocol: "local",
      cwd: typeof value.cwd === "string" ? value.cwd : undefined,
    };
  }

  if (value.protocol === "ssh" && isRecord(value.sshConfig)) {
    const sshConfig = value.sshConfig;
    if (typeof sshConfig.host !== "string" || typeof sshConfig.port !== "number" || typeof sshConfig.user !== "string") {
      return null;
    }

    return {
      protocol: "ssh",
      sshConfig: {
        host: sshConfig.host,
        port: sshConfig.port,
        user: sshConfig.user,
        password: typeof sshConfig.password === "string" ? sshConfig.password : undefined,
        privateKey: typeof sshConfig.privateKey === "string" ? sshConfig.privateKey : undefined,
        savedConnectionId: typeof sshConfig.savedConnectionId === "string" ? sshConfig.savedConnectionId : undefined,
        autoReconnect: typeof sshConfig.autoReconnect === "boolean" ? sshConfig.autoReconnect : undefined,
        reconnectType: sshConfig.reconnectType === "manual"
          || sshConfig.reconnectType === "simple"
          || sshConfig.reconnectType === "screen"
          || sshConfig.reconnectType === "tmux"
          ? sshConfig.reconnectType
          : undefined,
      },
    };
  }

  if (value.protocol === "telnet" && isRecord(value.telnetConfig)) {
    const telnetConfig = value.telnetConfig;
    if (typeof telnetConfig.host !== "string" || typeof telnetConfig.port !== "number") {
      return null;
    }

    return {
      protocol: "telnet",
      telnetConfig: {
        host: telnetConfig.host,
        port: telnetConfig.port,
      },
    };
  }

  if (value.protocol === "serial" && isRecord(value.serialConfig)) {
    const serialConfig = value.serialConfig;
    if (
      typeof serialConfig.portName !== "string"
      || typeof serialConfig.baudRate !== "number"
      || (serialConfig.dataBits !== 5 && serialConfig.dataBits !== 6 && serialConfig.dataBits !== 7 && serialConfig.dataBits !== 8)
      || (serialConfig.stopBits !== 1 && serialConfig.stopBits !== 2)
      || (serialConfig.parity !== "none" && serialConfig.parity !== "odd" && serialConfig.parity !== "even")
      || (serialConfig.flowControl !== "none" && serialConfig.flowControl !== "hardware" && serialConfig.flowControl !== "software")
    ) {
      return null;
    }

    return {
      protocol: "serial",
      serialConfig: {
        portName: serialConfig.portName,
        baudRate: serialConfig.baudRate,
        dataBits: serialConfig.dataBits,
        stopBits: serialConfig.stopBits,
        parity: serialConfig.parity,
        flowControl: serialConfig.flowControl,
      },
    };
  }

  return null;
}

function restorePersistedTabSnapshot(value: unknown): Tab | null {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.title !== "string") {
    return null;
  }

  const session = restoreSessionConfig(value.session);
  if (!session) {
    return null;
  }

  return {
    id: value.id,
    title: value.title,
    session,
    logPath: typeof value.logPath === "string" ? value.logPath : undefined,
  };
}

function restorePaneNode(value: unknown): PaneNode | null {
  if (!isRecord(value) || value.kind !== "leaf" && value.kind !== "split") {
    return null;
  }

  if (value.kind === "leaf") {
    if (typeof value.paneId !== "string") {
      return null;
    }

    return {
      kind: "leaf",
      paneId: value.paneId,
      tabId: typeof value.tabId === "string" ? value.tabId : null,
    };
  }

  if (typeof value.splitId !== "string") {
    return null;
  }
  if (value.axis !== "horizontal" && value.axis !== "vertical") {
    return null;
  }

  const first = restorePaneNode(value.first);
  const second = restorePaneNode(value.second);
  if (!first || !second) {
    return null;
  }

  return {
    kind: "split",
    splitId: value.splitId,
    axis: value.axis,
    ratio: clampPaneRatio(typeof value.ratio === "number" ? value.ratio : 0.5),
    first,
    second,
  };
}

function sanitizePaneTabs(node: PaneNode, sourceTabs: Tab[]): PaneNode {
  const availableTabIds = new Set(sourceTabs.map((tab) => tab.id));
  const assignedTabIds = new Set<string>();

  const sanitize = (current: PaneNode): PaneNode => {
    if (current.kind === "leaf") {
      const nextTabId = current.tabId && availableTabIds.has(current.tabId) && !assignedTabIds.has(current.tabId)
        ? current.tabId
        : null;

      if (nextTabId) {
        assignedTabIds.add(nextTabId);
      }

      return {
        ...current,
        tabId: nextTabId,
      };
    }

    return {
      ...current,
      ratio: clampPaneRatio(current.ratio),
      first: sanitize(current.first),
      second: sanitize(current.second),
    };
  };

  return sanitize(node);
}

function restorePaneLayoutState(value: unknown, sourceTabs: Tab[]): PersistedPaneLayoutState {
  if (!isRecord(value)) {
    const fallbackState = createDefaultPaneLayoutState(sourceTabs);
    syncPaneIdCounters(fallbackState.root);
    return fallbackState;
  }

  const restoredRoot = restorePaneNode(value.root);
  if (!restoredRoot) {
    const fallbackState = createDefaultPaneLayoutState(sourceTabs);
    syncPaneIdCounters(fallbackState.root);
    return fallbackState;
  }

  const sanitizedRoot = fillEmptyPanes(sanitizePaneTabs(restoredRoot, sourceTabs), sourceTabs);
  syncPaneIdCounters(sanitizedRoot);
  const preferredFocusedPaneId = typeof value.focusedPaneId === "string" ? value.focusedPaneId : null;
  const focusedPane = preferredFocusedPaneId ? findPaneById(sanitizedRoot, preferredFocusedPaneId) : null;

  return {
    root: sanitizedRoot,
    focusedPaneId: focusedPane?.paneId ?? getFirstLeafPane(sanitizedRoot).paneId,
  };
}

function restoreWorkspaceState(value: unknown): PersistedWorkspaceState | null {
  if (!isRecord(value) || !Array.isArray(value.tabs) || value.version !== 1) {
    return null;
  }

  const restoredTabs: Tab[] = [];
  const usedIds = new Set<string>();
  for (const item of value.tabs) {
    const restoredTab = restorePersistedTabSnapshot(item);
    if (!restoredTab || usedIds.has(restoredTab.id)) {
      continue;
    }
    usedIds.add(restoredTab.id);
    restoredTabs.push(restoredTab);
  }

  if (restoredTabs.length === 0) {
    return null;
  }

  const paneState = restorePaneLayoutState({
    root: value.paneLayout,
    focusedPaneId: value.focusedPaneId,
  }, restoredTabs);
  const activeTabId = typeof value.activeTabId === "string" && restoredTabs.some((tab) => tab.id === value.activeTabId)
    ? value.activeTabId
    : null;

  return {
    version: 1,
    tabs: restoredTabs,
    paneLayout: paneState.root,
    focusedPaneId: paneState.focusedPaneId,
    activeTabId,
  };
}

function createPersistedPaneLayoutState(): PersistedPaneLayoutState {
  return {
    root: paneLayout.value,
    focusedPaneId: focusedPaneId.value || null,
  };
}

function createPersistedWorkspaceState(restoreEnabled = settingsRef.value.restoreTabsOnStartup): PersistedWorkspaceState | null {
  if (!restoreEnabled || tabs.value.length === 0) {
    return null;
  }

  return {
    version: 1,
    tabs: tabs.value.map((tab) => ({
      id: tab.id,
      title: tab.title,
      session: tab.session,
      logPath: tab.logPath,
    })),
    paneLayout: paneLayout.value,
    focusedPaneId: focusedPaneId.value || null,
    activeTabId: activeTabId.value || null,
  };
}

function prepareSettingsForSave(baseSettings: AppSettings, restoreEnabled = baseSettings.restoreTabsOnStartup) {
  return normalizeAppSettings({
    ...baseSettings,
    paneLayout: createPersistedPaneLayoutState(),
    workspaceState: createPersistedWorkspaceState(restoreEnabled),
  });
}

function applyRestoredWorkspaceState(restoredWorkspaceState: PersistedWorkspaceState) {
  tabs.value = restoredWorkspaceState.tabs;
  syncTabIdCounter(restoredWorkspaceState.tabs);
  paneLayout.value = restoredWorkspaceState.paneLayout;
  syncPaneIdCounters(restoredWorkspaceState.paneLayout);

  const activeTabPane = restoredWorkspaceState.activeTabId
    ? findPaneByTabId(restoredWorkspaceState.paneLayout, restoredWorkspaceState.activeTabId)
    : null;
  syncFocusedPaneState(activeTabPane?.paneId ?? restoredWorkspaceState.focusedPaneId ?? undefined);
}

function applyDefaultStartupWorkspace(persistedPaneLayout: unknown) {
  const defaultTabs = [createDefaultLocalShellTab()];
  tabs.value = defaultTabs;
  syncTabIdCounter(defaultTabs);
  const restoredPaneState = restorePaneLayoutState(persistedPaneLayout, defaultTabs);
  paneLayout.value = restoredPaneState.root;
  syncPaneIdCounters(restoredPaneState.root);
  syncFocusedPaneState(restoredPaneState.focusedPaneId ?? undefined);
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

function findPaneById(node: PaneNode, paneId: string): PaneLeafNode | null {
  if (node.kind === "leaf") {
    return node.paneId === paneId ? node : null;
  }

  return findPaneById(node.first, paneId) ?? findPaneById(node.second, paneId);
}

function findPaneByTabId(node: PaneNode, tabId: string): PaneLeafNode | null {
  if (node.kind === "leaf") {
    return node.tabId === tabId ? node : null;
  }

  return findPaneByTabId(node.first, tabId) ?? findPaneByTabId(node.second, tabId);
}

function assignTabToPane(node: PaneNode, paneId: string, tabId: string | null): PaneNode {
  if (node.kind === "leaf") {
    return node.paneId === paneId ? { ...node, tabId } : node;
  }

  return {
    ...node,
    first: assignTabToPane(node.first, paneId, tabId),
    second: assignTabToPane(node.second, paneId, tabId),
  };
}

function clearTabFromLayout(node: PaneNode, tabId: string): PaneNode {
  if (node.kind === "leaf") {
    return node.tabId === tabId ? { ...node, tabId: null } : node;
  }

  return {
    ...node,
    first: clearTabFromLayout(node.first, tabId),
    second: clearTabFromLayout(node.second, tabId),
  };
}

function splitPane(node: PaneNode, paneId: string, axis: PaneAxis, newPane: PaneLeafNode): PaneNode {
  if (node.kind === "leaf") {
    if (node.paneId !== paneId) {
      return node;
    }

    return {
      kind: "split",
      splitId: createSplitId(),
      axis,
      ratio: 0.5,
      first: node,
      second: newPane,
    };
  }

  return {
    ...node,
    first: splitPane(node.first, paneId, axis, newPane),
    second: splitPane(node.second, paneId, axis, newPane),
  };
}

function removePaneWithFallback(node: PaneNode, paneId: string): RemovePaneResult {
  if (node.kind === "leaf") {
    return {
      node: node.paneId === paneId ? null : node,
      removed: node.paneId === paneId,
      fallbackPaneId: null,
    };
  }

  const firstResult = removePaneWithFallback(node.first, paneId);
  if (firstResult.removed) {
    if (!firstResult.node) {
      return {
        node: node.second,
        removed: true,
        fallbackPaneId: getFirstLeafPane(node.second).paneId,
      };
    }

    return {
      node: { ...node, first: firstResult.node, second: node.second },
      removed: true,
      fallbackPaneId: firstResult.fallbackPaneId,
    };
  }

  const secondResult = removePaneWithFallback(node.second, paneId);
  if (secondResult.removed) {
    if (!secondResult.node) {
      return {
        node: node.first,
        removed: true,
        fallbackPaneId: getFirstLeafPane(node.first).paneId,
      };
    }

    return {
      node: { ...node, first: node.first, second: secondResult.node },
      removed: true,
      fallbackPaneId: secondResult.fallbackPaneId,
    };
  }

  return {
    node,
    removed: false,
    fallbackPaneId: null,
  };
}

function clampPaneRatio(ratio: number) {
  return Math.max(MIN_PANE_RATIO, Math.min(MAX_PANE_RATIO, ratio));
}

function updateSplitRatio(node: PaneNode, splitId: string, ratio: number): PaneNode {
  if (node.kind === "leaf") {
    return node;
  }

  if (node.splitId === splitId) {
    return {
      ...node,
      ratio: clampPaneRatio(ratio),
    };
  }

  return {
    ...node,
    first: updateSplitRatio(node.first, splitId, ratio),
    second: updateSplitRatio(node.second, splitId, ratio),
  };
}

function collectPaneLeaves(node: PaneNode, rect: PaneRect = { left: 0, top: 0, width: 100, height: 100 }): PaneLeafLayout[] {
  if (node.kind === "leaf") {
    return [{ paneId: node.paneId, tabId: node.tabId, rect }];
  }

  if (node.axis === "vertical") {
    const firstWidth = rect.width * node.ratio;
    return [
      ...collectPaneLeaves(node.first, { ...rect, width: firstWidth }),
      ...collectPaneLeaves(node.second, {
        left: rect.left + firstWidth,
        top: rect.top,
        width: rect.width - firstWidth,
        height: rect.height,
      }),
    ];
  }

  const firstHeight = rect.height * node.ratio;
  return [
    ...collectPaneLeaves(node.first, { ...rect, height: firstHeight }),
    ...collectPaneLeaves(node.second, {
      left: rect.left,
      top: rect.top + firstHeight,
      width: rect.width,
      height: rect.height - firstHeight,
    }),
  ];
}

function collectSplitHandles(node: PaneNode, rect: PaneRect = { left: 0, top: 0, width: 100, height: 100 }): PaneSplitHandleLayout[] {
  if (node.kind === "leaf") {
    return [];
  }

  const position = node.axis === "vertical"
    ? rect.left + rect.width * node.ratio
    : rect.top + rect.height * node.ratio;
  const firstRect = node.axis === "vertical"
    ? { ...rect, width: rect.width * node.ratio }
    : { ...rect, height: rect.height * node.ratio };
  const secondRect = node.axis === "vertical"
    ? {
        left: rect.left + rect.width * node.ratio,
        top: rect.top,
        width: rect.width - rect.width * node.ratio,
        height: rect.height,
      }
    : {
        left: rect.left,
        top: rect.top + rect.height * node.ratio,
        width: rect.width,
        height: rect.height - rect.height * node.ratio,
      };

  return [
    {
      splitId: node.splitId,
      axis: node.axis,
      parentRect: rect,
      position,
    },
    ...collectSplitHandles(node.first, firstRect),
    ...collectSplitHandles(node.second, secondRect),
  ];
}

function fillEmptyPanes(node: PaneNode, sourceTabs: Tab[]): PaneNode {
  let nextLayout = node;
  const visibleTabIds = new Set(
    collectPaneLeaves(nextLayout)
      .map((pane) => pane.tabId)
      .filter((tabId): tabId is string => Boolean(tabId)),
  );

  for (const pane of collectPaneLeaves(nextLayout)) {
    if (pane.tabId) {
      continue;
    }

    const nextTab = sourceTabs.find((tab) => !visibleTabIds.has(tab.id));
    if (!nextTab) {
      break;
    }

    nextLayout = assignTabToPane(nextLayout, pane.paneId, nextTab.id);
    visibleTabIds.add(nextTab.id);
  }

  return nextLayout;
}

function getTabTitle(tabId: string | null) {
  return tabs.value.find((tab) => tab.id === tabId)?.title ?? "Empty Pane";
}

function getTabById(tabId: string | null) {
  return tabs.value.find((tab) => tab.id === tabId) ?? null;
}

function getTabProtocolLabel(tabId: string | null) {
  const tab = getTabById(tabId);
  if (!tab) {
    return "No Session";
  }

  switch (tab.session.protocol) {
    case "local":
      return "Local";
    case "ssh":
      return "SSH";
    case "telnet":
      return "Telnet";
    case "serial":
      return "Serial";
  }
}

function getFirstLeafPane(node: PaneNode): PaneLeafNode {
  if (node.kind === "leaf") {
    return node;
  }

  return getFirstLeafPane(node.first);
}

function syncFocusedPaneState(preferredPaneId?: string) {
  const nextFocusedPane = preferredPaneId
    ? findPaneById(paneLayout.value, preferredPaneId)
    : null;
  const fallbackPane = nextFocusedPane ?? (paneLayout.value ? getFirstLeafPane(paneLayout.value) : null);

  if (!fallbackPane) {
    focusedPaneId.value = "";
    activeTabId.value = "";
    return;
  }

  focusedPaneId.value = fallbackPane.paneId;
  activeTabId.value = fallbackPane.tabId ?? "";
}

function focusPane(paneId: string) {
  const pane = findPaneById(paneLayout.value, paneId);
  if (!pane) {
    return;
  }

  focusedPaneId.value = paneId;
  activeTabId.value = pane.tabId ?? "";
}

function assignTabToFocusedPane(tabId: string) {
  const targetPane = findPaneById(paneLayout.value, focusedPaneId.value) ?? getFirstLeafPane(paneLayout.value);
  paneLayout.value = clearTabFromLayout(paneLayout.value, tabId);
  paneLayout.value = assignTabToPane(paneLayout.value, targetPane.paneId, tabId);
  focusedPaneId.value = targetPane.paneId;
  activeTabId.value = tabId;
}

function moveTabToPane(tabId: string, paneId: string) {
  const targetPane = findPaneById(paneLayout.value, paneId);
  if (!targetPane) {
    return;
  }

  paneLayout.value = clearTabFromLayout(paneLayout.value, tabId);
  paneLayout.value = assignTabToPane(paneLayout.value, paneId, tabId);
  focusedPaneId.value = paneId;
  activeTabId.value = tabId;
}

function findFirstHiddenTabId() {
  const visibleTabIds = new Set(
    collectPaneLeaves(paneLayout.value)
      .map((pane) => pane.tabId)
      .filter((tabId): tabId is string => Boolean(tabId)),
  );
  return tabs.value.find((tab) => !visibleTabIds.has(tab.id))?.id ?? null;
}

function handleSplitPane(axis: PaneAxis, paneId = focusedPaneId.value) {
  const targetPane = findPaneById(paneLayout.value, paneId);
  if (!targetPane) {
    return;
  }

  const nextPaneId = createPaneId();
  const hiddenTabId = findFirstHiddenTabId();
  paneLayout.value = splitPane(paneLayout.value, paneId, axis, {
    kind: "leaf",
    paneId: nextPaneId,
    tabId: hiddenTabId,
  });
  focusedPaneId.value = nextPaneId;
  activeTabId.value = hiddenTabId ?? "";
}

function handleClosePane(paneId = focusedPaneId.value) {
  const leafPanes = collectPaneLeaves(paneLayout.value);
  if (leafPanes.length <= 1) {
    return;
  }

  const result = removePaneWithFallback(paneLayout.value, paneId);
  if (!result.node) {
    return;
  }

  paneLayout.value = fillEmptyPanes(result.node, tabs.value);
  const nextFocusedPaneId = paneId === focusedPaneId.value
    ? result.fallbackPaneId ?? focusedPaneId.value
    : focusedPaneId.value;
  syncFocusedPaneState(nextFocusedPaneId);
  void nextTick(() => {
    fitVisibleTerminals();
  });
}

function getPaneShellStyle(rect: PaneRect) {
  return {
    left: `calc(${rect.left}% + ${PANE_INSET}px)`,
    top: `calc(${rect.top}% + ${PANE_INSET}px)`,
    width: `calc(${rect.width}% - ${PANE_INSET * 2}px)`,
    height: `calc(${rect.height}% - ${PANE_INSET * 2}px)`,
  };
}

function getTerminalViewportStyle(rect: PaneRect, isVisible: boolean) {
  return {
    display: isVisible ? "block" : "none",
    left: `calc(${rect.left}% + ${PANE_INSET + 1}px)`,
    top: `calc(${rect.top}% + ${PANE_INSET + PANE_HEADER_HEIGHT + 1}px)`,
    width: `calc(${rect.width}% - ${(PANE_INSET + 1) * 2}px)`,
    height: `calc(${rect.height}% - ${PANE_INSET * 2 + PANE_HEADER_HEIGHT + 2}px)`,
  };
}

function isPaneFocused(paneId: string) {
  return isWindowFocused.value && focusedPaneId.value === paneId;
}

function closeOpenMenus() {
  openMenuId.value = null;
  openFileSubmenuId.value = null;
  tabContextMenu.value = null;
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
    } else {
      applyDefaultStartupWorkspace(normalizedSettings.paneLayout);
    }
  } catch {
    const fallbackSettings = normalizeAppSettings();
    settings.value = fallbackSettings;
    settingsRef.value = fallbackSettings;
    applyDefaultStartupWorkspace(fallbackSettings.paneLayout);
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
  cleanupPaneResizeTracking?.();
  cleanupPaneResizeTracking = null;
  if (pendingFitFrame !== null) {
    window.cancelAnimationFrame(pendingFitFrame);
    pendingFitFrame = null;
  }
  paneResizeObserver?.disconnect();
  paneResizeObserver = null;
  cleanupTabPointerTracking();
  while (cleanupFns.length > 0) {
    const cleanup = cleanupFns.pop();
    cleanup?.();
  }
});

const paneLeaves = computed(() => collectPaneLeaves(paneLayout.value));
const paneSplitHandles = computed(() => collectSplitHandles(paneLayout.value));
const paneByTabId = computed<Record<string, PaneLeafLayout>>(() => {
  const result: Record<string, PaneLeafLayout> = {};
  for (const pane of paneLeaves.value) {
    if (pane.tabId) {
      result[pane.tabId] = pane;
    }
  }
  return result;
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

function getSplitHandleStyle(handle: PaneSplitHandleLayout) {
  if (handle.axis === "vertical") {
    return {
      left: `calc(${handle.position}% - 5px)`,
      top: `${handle.parentRect.top}%`,
      width: "10px",
      height: `${handle.parentRect.height}%`,
    };
  }

  return {
    left: `${handle.parentRect.left}%`,
    top: `calc(${handle.position}% - 5px)`,
    width: `${handle.parentRect.width}%`,
    height: "10px",
  };
}

function handlePaneResizePointerDown(event: PointerEvent, handle: PaneSplitHandleLayout) {
  if (event.button !== 0 || !terminalContainerRef.value) {
    return;
  }

  event.preventDefault();
  event.stopPropagation();
  cleanupPaneResizeTracking?.();

  const applyRatioFromPointer = (pointerEvent: PointerEvent) => {
    const containerRect = terminalContainerRef.value?.getBoundingClientRect();
    if (!containerRect || containerRect.width <= 0 || containerRect.height <= 0) {
      return;
    }

    const pointerPercentX = ((pointerEvent.clientX - containerRect.left) / containerRect.width) * 100;
    const pointerPercentY = ((pointerEvent.clientY - containerRect.top) / containerRect.height) * 100;
    const ratio = handle.axis === "vertical"
      ? (pointerPercentX - handle.parentRect.left) / handle.parentRect.width
      : (pointerPercentY - handle.parentRect.top) / handle.parentRect.height;

    paneLayout.value = updateSplitRatio(paneLayout.value, handle.splitId, ratio);
  };

  const handlePointerMove = (pointerEvent: PointerEvent) => {
    applyRatioFromPointer(pointerEvent);
  };

  const stopTracking = () => {
    window.removeEventListener("pointermove", handlePointerMove);
    window.removeEventListener("pointerup", stopTracking);
    window.removeEventListener("pointercancel", stopTracking);
    cleanupPaneResizeTracking = null;
    void nextTick(() => {
      fitVisibleTerminals();
    });
  };

  cleanupPaneResizeTracking = stopTracking;
  window.addEventListener("pointermove", handlePointerMove);
  window.addEventListener("pointerup", stopTracking);
  window.addEventListener("pointercancel", stopTracking);
  applyRatioFromPointer(event);
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

function selectTab(tabId: string) {
  const visiblePane = findPaneByTabId(paneLayout.value, tabId);
  if (visiblePane) {
    focusPane(visiblePane.paneId);
    return;
  }

  assignTabToFocusedPane(tabId);
}

function splitTabToPane(tabId: string, axis: PaneAxis) {
  const visiblePane = findPaneByTabId(paneLayout.value, tabId);
  if (visiblePane) {
    focusPane(visiblePane.paneId);
    handleSplitPane(axis, visiblePane.paneId);
    return;
  }

  const targetPane = findPaneById(paneLayout.value, focusedPaneId.value) ?? getFirstLeafPane(paneLayout.value);
  const nextPaneId = createPaneId();
  paneLayout.value = clearTabFromLayout(paneLayout.value, tabId);
  paneLayout.value = splitPane(paneLayout.value, targetPane.paneId, axis, {
    kind: "leaf",
    paneId: nextPaneId,
    tabId,
  });
  focusedPaneId.value = nextPaneId;
  activeTabId.value = tabId;
}

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

function commitTabRename() {
  if (!renamingTabId.value) {
    return;
  }

  const nextTitle = renamingTabTitle.value.trim();
  if (nextTitle) {
    tabs.value = tabs.value.map((tab) => (
      tab.id === renamingTabId.value ? { ...tab, title: nextTitle } : tab
    ));
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
  hoveredEmptyPaneId.value = null;
  activeTabId.value = tabId;
  tabDragMoved = true;
  suppressTabClick.value = true;
}

function updateDraggedEmptyPaneTarget(clientX: number, clientY: number) {
  if (!draggedTabId.value) {
    hoveredEmptyPaneId.value = null;
    return;
  }

  const hoveredElement = document.elementFromPoint(clientX, clientY);
  const target = hoveredElement instanceof HTMLElement
    ? hoveredElement.closest<HTMLElement>(".terminal-pane-empty[data-pane-id]")
    : null;
  hoveredEmptyPaneId.value = target?.dataset.paneId ?? null;
}

function finishTabDrag() {
  const moved = tabDragMoved;
  hoveredEmptyPaneId.value = null;
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

  updateDraggedEmptyPaneTarget(event.clientX, event.clientY);

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

  const dropPaneId = draggedTabId.value ? hoveredEmptyPaneId.value : null;
  if (draggedTabId.value && dropPaneId) {
    moveTabToPane(draggedTabId.value, dropPaneId);
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
  hoveredEmptyPaneId.value = null;
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

  tabs.value = nextTabs;
  paneLayout.value = clearTabFromLayout(paneLayout.value, id);
  paneLayout.value = fillEmptyPanes(paneLayout.value, nextTabs);
  syncFocusedPaneState(focusedPaneId.value);

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
    <div class="titlebar" @mousedown="handleTitlebarMouseDown" @dblclick="handleToggleMaximize">
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
            >
              <div class="terminal-pane-header">
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