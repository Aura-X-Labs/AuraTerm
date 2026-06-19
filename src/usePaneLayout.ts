import { computed, ref, watch, type Ref } from "vue";
import type { SessionConfig } from "./types";

export interface PaneLayoutTab {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}

export type PaneAxis = "horizontal" | "vertical";
export type DropPosition = "top" | "bottom" | "left" | "right" | "center";
type DragSourceKind = "tab" | "pane";

export interface PaneLeafNode {
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

export type PaneNode = PaneLeafNode | PaneSplitNode;

export interface PaneRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface PaneLeafLayout {
  paneId: string;
  tabId: string | null;
  rect: PaneRect;
}

export interface PaneSplitHandleLayout {
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

export interface PersistedPaneLayoutState {
  root: PaneNode;
  focusedPaneId: string | null;
}

interface PersistedTabSnapshot {
  id: string;
  title: string;
  session: SessionConfig;
  logPath?: string;
}

export interface PersistedWorkspaceState {
  version: 1;
  tabs: PersistedTabSnapshot[];
  paneLayout: PaneNode;
  focusedPaneId: string | null;
  activeTabId: string | null;
}

interface PendingDragState {
  tabId: string;
  sourcePaneId: string | null;
  sourceKind: DragSourceKind;
  startX: number;
  startY: number;
}

interface UsePaneLayoutOptions {
  tabs: Ref<PaneLayoutTab[]>;
  isWindowFocused: Ref<boolean>;
  terminalContainerRef: Ref<Element | null>;
}

let nextPaneId = 1;
let nextSplitId = 1;

const PANE_HEADER_HEIGHT = 30;
const PANE_INSET = 4;
const MIN_PANE_RATIO = 0.15;
const MAX_PANE_RATIO = 0.85;

function parseSequenceId(value: string | null | undefined, prefix: string) {
  if (!value || !value.startsWith(prefix)) {
    return null;
  }

  const numericValue = Number.parseInt(value.slice(prefix.length), 10);
  return Number.isFinite(numericValue) ? numericValue : null;
}

function createPaneId() {
  return `pane-${nextPaneId++}`;
}

function createSplitId() {
  return `split-${nextSplitId++}`;
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
        passphrase: typeof sshConfig.passphrase === "string" ? sshConfig.passphrase : undefined,
        authType: sshConfig.authType === "password" || sshConfig.authType === "key" || sshConfig.authType === "agent" || sshConfig.authType === "none"
          ? sshConfig.authType
          : undefined,
        agentForwarding: typeof sshConfig.agentForwarding === "boolean" ? sshConfig.agentForwarding : undefined,
        jumpHosts: Array.isArray(sshConfig.jumpHosts) ? sshConfig.jumpHosts as never : undefined,
        autoLoginRules: Array.isArray(sshConfig.autoLoginRules) ? sshConfig.autoLoginRules as never : undefined,
        postConnectCommands: Array.isArray(sshConfig.postConnectCommands)
          ? sshConfig.postConnectCommands.filter((item): item is string => typeof item === "string")
          : undefined,
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

function sanitizeSessionForPersistence(session: SessionConfig): SessionConfig {
  if (session.protocol !== "ssh") {
    return session;
  }

  const config = session.sshConfig;
  return {
    protocol: "ssh",
    sshConfig: {
      ...config,
      password: undefined,
      privateKey: undefined,
      passphrase: undefined,
      jumpHosts: config.jumpHosts?.map((jump) => ({
        ...jump,
        password: undefined,
        privateKey: undefined,
        passphrase: undefined,
      })),
      autoLoginRules: config.autoLoginRules?.map((rule) => ({ ...rule, response: undefined })),
      postConnectCommands: undefined,
    },
  };
}

function restorePersistedTabSnapshot(value: unknown): PaneLayoutTab | null {
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

function clampPaneRatio(ratio: number) {
  return Math.max(MIN_PANE_RATIO, Math.min(MAX_PANE_RATIO, ratio));
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

function createDefaultPaneLayoutState(sourceTabs: PaneLayoutTab[]): PersistedPaneLayoutState {
  return {
    root: {
      kind: "leaf",
      paneId: "pane-0",
      tabId: sourceTabs[0]?.id ?? null,
    },
    focusedPaneId: "pane-0",
  };
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

function getFirstLeafPane(node: PaneNode): PaneLeafNode {
  if (node.kind === "leaf") {
    return node;
  }

  return getFirstLeafPane(node.first);
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

function splitPaneAtPosition(node: PaneNode, targetPaneId: string, position: DropPosition, newPane: PaneLeafNode): PaneNode {
  if (node.kind === "leaf") {
    if (node.paneId !== targetPaneId) {
      return node;
    }

    if (position === "center") {
      return { ...node, tabId: newPane.tabId };
    }

    const axis: PaneAxis = (position === "left" || position === "right") ? "vertical" : "horizontal";
    const placeAfterTarget = position === "right" || position === "bottom";

    return {
      kind: "split",
      splitId: createSplitId(),
      axis,
      ratio: 0.5,
      first: placeAfterTarget ? node : newPane,
      second: placeAfterTarget ? newPane : node,
    };
  }

  return {
    ...node,
    first: splitPaneAtPosition(node.first, targetPaneId, position, newPane),
    second: splitPaneAtPosition(node.second, targetPaneId, position, newPane),
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

function sanitizePaneTabs(node: PaneNode, sourceTabs: PaneLayoutTab[]): PaneNode {
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

function fillEmptyPanes(node: PaneNode, sourceTabs: PaneLayoutTab[]): PaneNode {
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

function restorePaneLayoutState(value: unknown, sourceTabs: PaneLayoutTab[]): PersistedPaneLayoutState {
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

export function usePaneLayout({ tabs, isWindowFocused, terminalContainerRef }: UsePaneLayoutOptions) {
  const paneLayout = ref<PaneNode>({
    kind: "leaf",
    paneId: "pane-0",
    tabId: null,
  });
  const focusedPaneId = ref("pane-0");
  const activeTabId = ref("");
  const draggedTabId = ref<string | null>(null);
  const hoveredEmptyPaneId = ref<string | null>(null);
  const dropTargetPaneId = ref<string | null>(null);
  const dropTargetPosition = ref<DropPosition | null>(null);
  const dragSourcePaneId = ref<string | null>(null);
  const dragSourceKind = ref<DragSourceKind | null>(null);
  const paneTabHistory = ref<Record<string, string[]>>({});

  let pendingDrag: PendingDragState | null = null;

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

  function createPersistedPaneLayoutState(): PersistedPaneLayoutState {
    return {
      root: paneLayout.value,
      focusedPaneId: focusedPaneId.value || null,
    };
  }

  function createPersistedWorkspaceState(restoreEnabled = true): PersistedWorkspaceState | null {
    if (!restoreEnabled || tabs.value.length === 0) {
      return null;
    }

    return {
      version: 1,
      tabs: tabs.value.map((tab) => ({
        id: tab.id,
        title: tab.title,
        session: sanitizeSessionForPersistence(tab.session),
        logPath: tab.logPath,
      })),
      paneLayout: paneLayout.value,
      focusedPaneId: focusedPaneId.value || null,
      activeTabId: activeTabId.value || null,
    };
  }

  function restoreWorkspaceState(value: unknown): PersistedWorkspaceState | null {
    if (!isRecord(value) || !Array.isArray(value.tabs) || value.version !== 1) {
      return null;
    }

    const restoredTabs: PaneLayoutTab[] = [];
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
    const restoredActiveTabId = typeof value.activeTabId === "string" && restoredTabs.some((tab) => tab.id === value.activeTabId)
      ? value.activeTabId
      : null;

    return {
      version: 1,
      tabs: restoredTabs,
      paneLayout: paneState.root,
      focusedPaneId: paneState.focusedPaneId,
      activeTabId: restoredActiveTabId,
    };
  }

  function clonePaneTabHistory() {
    return Object.fromEntries(
      Object.entries(paneTabHistory.value).map(([paneId, tabIds]) => [paneId, [...tabIds]]),
    ) as Record<string, string[]>;
  }

  function removeTabFromHistoryMap(historyMap: Record<string, string[]>, tabId: string) {
    for (const paneId of Object.keys(historyMap)) {
      historyMap[paneId] = historyMap[paneId].filter((candidateTabId) => candidateTabId !== tabId);
    }
  }

  function getPaneFallbackTabId(historyMap: Record<string, string[]>, paneId: string) {
    const history = historyMap[paneId] ?? [];
    return history.length > 0 ? history[history.length - 1] : null;
  }

  function syncPaneTabHistory(node: PaneNode = paneLayout.value, sourceTabs: PaneLayoutTab[] = tabs.value) {
    const availableTabIds = new Set(sourceTabs.map((tab) => tab.id));
    const nextHistory: Record<string, string[]> = {};

    for (const pane of collectPaneLeaves(node)) {
      const existingHistory = paneTabHistory.value[pane.paneId] ?? [];
      const dedupedHistory: string[] = [];

      for (const tabId of existingHistory) {
        if (!availableTabIds.has(tabId) || dedupedHistory.includes(tabId) || tabId === pane.tabId) {
          continue;
        }
        dedupedHistory.push(tabId);
      }

      nextHistory[pane.paneId] = pane.tabId ? [...dedupedHistory, pane.tabId] : dedupedHistory;
    }

    paneTabHistory.value = nextHistory;
  }

  function syncFocusedPaneState(preferredPaneId?: string) {
    const nextFocusedPane = preferredPaneId
      ? findPaneById(paneLayout.value, preferredPaneId)
      : null;
    const fallbackPane = nextFocusedPane ?? getFirstLeafPane(paneLayout.value);

    focusedPaneId.value = fallbackPane.paneId;
    activeTabId.value = fallbackPane.tabId ?? "";
  }

  function applyRestoredWorkspaceState(restoredWorkspaceState: PersistedWorkspaceState) {
    tabs.value = restoredWorkspaceState.tabs;
    paneLayout.value = restoredWorkspaceState.paneLayout;
    syncPaneIdCounters(restoredWorkspaceState.paneLayout);
    syncPaneTabHistory(restoredWorkspaceState.paneLayout, restoredWorkspaceState.tabs);

    const activeTabPane = restoredWorkspaceState.activeTabId
      ? findPaneByTabId(restoredWorkspaceState.paneLayout, restoredWorkspaceState.activeTabId)
      : null;
    syncFocusedPaneState(activeTabPane?.paneId ?? restoredWorkspaceState.focusedPaneId ?? undefined);
  }

  function applyPaneLayoutFromTabs(sourceTabs: PaneLayoutTab[], persistedPaneLayout: unknown) {
    tabs.value = sourceTabs;
    const restoredPaneState = restorePaneLayoutState(persistedPaneLayout, sourceTabs);
    paneLayout.value = restoredPaneState.root;
    syncPaneIdCounters(restoredPaneState.root);
    syncPaneTabHistory(restoredPaneState.root, sourceTabs);
    syncFocusedPaneState(restoredPaneState.focusedPaneId ?? undefined);
  }

  function activateTabInPane(paneId: string, tabId: string) {
    const targetPane = findPaneById(paneLayout.value, paneId);
    if (!targetPane) {
      return;
    }

    const nextHistory = clonePaneTabHistory();
    removeTabFromHistoryMap(nextHistory, tabId);

    let nextLayout = paneLayout.value;
    const sourcePane = findPaneByTabId(nextLayout, tabId);
    if (sourcePane && sourcePane.paneId !== paneId) {
      nextLayout = assignTabToPane(nextLayout, sourcePane.paneId, getPaneFallbackTabId(nextHistory, sourcePane.paneId));
    }

    nextHistory[paneId] = [...(nextHistory[paneId] ?? []), tabId];
    nextLayout = assignTabToPane(nextLayout, paneId, tabId);

    paneTabHistory.value = nextHistory;
    paneLayout.value = nextLayout;
    focusedPaneId.value = paneId;
    activeTabId.value = tabId;
  }

  function moveTabIntoNewPane(
    tabId: string,
    nextPaneId: string,
    insertPane: (layout: PaneNode, newPane: PaneLeafNode) => PaneNode,
  ) {
    const nextHistory = clonePaneTabHistory();
    removeTabFromHistoryMap(nextHistory, tabId);

    let nextLayout = paneLayout.value;
    const sourcePane = findPaneByTabId(nextLayout, tabId);
    if (sourcePane) {
      nextLayout = assignTabToPane(nextLayout, sourcePane.paneId, getPaneFallbackTabId(nextHistory, sourcePane.paneId));
    }

    nextHistory[nextPaneId] = [tabId];
    nextLayout = insertPane(nextLayout, {
      kind: "leaf",
      paneId: nextPaneId,
      tabId,
    });

    paneTabHistory.value = nextHistory;
    paneLayout.value = nextLayout;
    focusedPaneId.value = nextPaneId;
    activeTabId.value = tabId;
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
    activateTabInPane(targetPane.paneId, tabId);
  }

  function moveTabToPane(tabId: string, paneId: string) {
    activateTabInPane(paneId, tabId);
  }

  function moveTabToDropPosition(tabId: string, targetPaneId: string, position: DropPosition) {
    const sourcePane = findPaneByTabId(paneLayout.value, tabId);
    if (sourcePane?.paneId === targetPaneId) {
      return;
    }

    if (position === "center") {
      moveTabToPane(tabId, targetPaneId);
      return;
    }

    const nextPaneId = createPaneId();
    moveTabIntoNewPane(tabId, nextPaneId, (layout, newPane) => splitPaneAtPosition(layout, targetPaneId, position, newPane));
  }

  function movePaneToDropPosition(sourcePaneId: string, targetPaneId: string, position: DropPosition) {
    if (sourcePaneId === targetPaneId) {
      return;
    }

    const sourcePane = findPaneById(paneLayout.value, sourcePaneId);
    if (!sourcePane?.tabId) {
      return;
    }

    const removalResult = removePaneWithFallback(paneLayout.value, sourcePaneId);
    if (!removalResult.node) {
      return;
    }

    const movedTabId = sourcePane.tabId;
    if (position === "center") {
      paneLayout.value = assignTabToPane(removalResult.node, targetPaneId, movedTabId);
      focusedPaneId.value = targetPaneId;
      activeTabId.value = movedTabId;
      return;
    }

    paneLayout.value = splitPaneAtPosition(removalResult.node, targetPaneId, position, {
      kind: "leaf",
      paneId: sourcePane.paneId,
      tabId: movedTabId,
    });
    focusedPaneId.value = sourcePane.paneId;
    activeTabId.value = movedTabId;
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
    paneTabHistory.value = {
      ...paneTabHistory.value,
      [nextPaneId]: hiddenTabId ? [hiddenTabId] : [],
    };
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
    syncPaneTabHistory(paneLayout.value, tabs.value);
    const nextFocusedPaneId = paneId === focusedPaneId.value
      ? result.fallbackPaneId ?? focusedPaneId.value
      : focusedPaneId.value;
    syncFocusedPaneState(nextFocusedPaneId);
  }

  function handleTabRemoved(tabId: string, nextTabs: PaneLayoutTab[]) {
    const nextHistory = clonePaneTabHistory();
    removeTabFromHistoryMap(nextHistory, tabId);

    tabs.value = nextTabs;
    const sourcePane = findPaneByTabId(paneLayout.value, tabId);
    if (sourcePane) {
      paneLayout.value = assignTabToPane(paneLayout.value, sourcePane.paneId, getPaneFallbackTabId(nextHistory, sourcePane.paneId));
    }
    paneLayout.value = fillEmptyPanes(paneLayout.value, nextTabs);
    paneTabHistory.value = nextHistory;
    syncPaneTabHistory(paneLayout.value, nextTabs);
    syncFocusedPaneState(focusedPaneId.value);
  }

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
    moveTabIntoNewPane(tabId, nextPaneId, (layout, newPane) => splitPane(layout, targetPane.paneId, axis, newPane));
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
    const isSplit = paneLeaves.value.length > 1;
    const headerHeight = isSplit ? PANE_HEADER_HEIGHT : 0;
    const borderCorrection = isSplit ? 2 : 0;
    
    return {
      display: isVisible ? "block" : "none",
      left: `calc(${rect.left}% + ${PANE_INSET + 1}px)`,
      top: `calc(${rect.top}% + ${PANE_INSET + headerHeight + 1}px)`,
      width: `calc(${rect.width}% - ${(PANE_INSET + 1) * 2}px)`,
      height: `calc(${rect.height}% - ${PANE_INSET * 2 + headerHeight + borderCorrection}px)`,
    };
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

  function isPaneFocused(paneId: string) {
    return isWindowFocused.value && focusedPaneId.value === paneId;
  }

  function getTabById(tabId: string | null) {
    return tabs.value.find((tab) => tab.id === tabId) ?? null;
  }

  function getTabTitle(tabId: string | null) {
    return getTabById(tabId)?.title ?? "Empty Pane";
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

  function cleanupTabPointerTracking() {
    // No-op: pointer tracking is via setPointerCapture on tab/header elements.
  }

  function beginDrag(state: PendingDragState) {
    draggedTabId.value = state.tabId;
    hoveredEmptyPaneId.value = null;
    dropTargetPaneId.value = null;
    dropTargetPosition.value = null;
    dragSourcePaneId.value = state.sourcePaneId;
    dragSourceKind.value = state.sourceKind;
    activeTabId.value = state.tabId;
  }

  function updateDraggedDropTarget(clientX: number, clientY: number) {
    if (!draggedTabId.value) {
      hoveredEmptyPaneId.value = null;
      dropTargetPaneId.value = null;
      dropTargetPosition.value = null;
      return;
    }

    const containerRect = terminalContainerRef.value?.getBoundingClientRect();
    if (!containerRect || containerRect.width <= 0 || containerRect.height <= 0) {
      hoveredEmptyPaneId.value = null;
      dropTargetPaneId.value = null;
      dropTargetPosition.value = null;
      return;
    }

    const hitPane = paneLeaves.value.find((pane) => {
      const left = containerRect.left + (containerRect.width * pane.rect.left) / 100 + PANE_INSET;
      const top = containerRect.top + (containerRect.height * pane.rect.top) / 100 + PANE_INSET;
      const width = (containerRect.width * pane.rect.width) / 100 - PANE_INSET * 2;
      const height = (containerRect.height * pane.rect.height) / 100 - PANE_INSET * 2;

      return width > 0
        && height > 0
        && clientX >= left
        && clientX <= left + width
        && clientY >= top
        && clientY <= top + height;
    });

    if (!hitPane || (dragSourcePaneId.value && hitPane.paneId === dragSourcePaneId.value)) {
      hoveredEmptyPaneId.value = null;
      dropTargetPaneId.value = null;
      dropTargetPosition.value = null;
      return;
    }

    if (!hitPane.tabId) {
      hoveredEmptyPaneId.value = hitPane.paneId;
      dropTargetPaneId.value = null;
      dropTargetPosition.value = null;
      return;
    }

    const paneLeft = containerRect.left + (containerRect.width * hitPane.rect.left) / 100 + PANE_INSET;
    const paneTop = containerRect.top + (containerRect.height * hitPane.rect.top) / 100 + PANE_INSET;
    const paneWidth = (containerRect.width * hitPane.rect.width) / 100 - PANE_INSET * 2;
    const paneHeight = (containerRect.height * hitPane.rect.height) / 100 - PANE_INSET * 2;
    const x = Math.min(Math.max((clientX - paneLeft) / paneWidth, 0), 1);
    const y = Math.min(Math.max((clientY - paneTop) / paneHeight, 0), 1);

    hoveredEmptyPaneId.value = null;
    dropTargetPaneId.value = hitPane.paneId;
    if (x > 0.25 && x < 0.75 && y > 0.25 && y < 0.75) {
      dropTargetPosition.value = "center";
      return;
    }

    const distTop = y;
    const distBottom = 1 - y;
    const distLeft = x;
    const distRight = 1 - x;
    const minDist = Math.min(distTop, distBottom, distLeft, distRight);

    if (minDist === distTop) {
      dropTargetPosition.value = "top";
    } else if (minDist === distBottom) {
      dropTargetPosition.value = "bottom";
    } else if (minDist === distLeft) {
      dropTargetPosition.value = "left";
    } else {
      dropTargetPosition.value = "right";
    }
  }

  function applyDragDrop() {
    if (!draggedTabId.value) {
      return;
    }

    if (hoveredEmptyPaneId.value) {
      if (dragSourceKind.value === "pane" && dragSourcePaneId.value) {
        movePaneToDropPosition(dragSourcePaneId.value, hoveredEmptyPaneId.value, "center");
      } else {
        moveTabToPane(draggedTabId.value, hoveredEmptyPaneId.value);
      }
      return;
    }

    if (!dropTargetPaneId.value || !dropTargetPosition.value) {
      return;
    }

    if (dragSourceKind.value === "pane" && dragSourcePaneId.value) {
      movePaneToDropPosition(dragSourcePaneId.value, dropTargetPaneId.value, dropTargetPosition.value);
      return;
    }

    moveTabToDropPosition(draggedTabId.value, dropTargetPaneId.value, dropTargetPosition.value);
  }

  function finishTabDrag() {
    hoveredEmptyPaneId.value = null;
    dropTargetPaneId.value = null;
    dropTargetPosition.value = null;
    cleanupTabPointerTracking();
    pendingDrag = null;
    draggedTabId.value = null;
    dragSourcePaneId.value = null;
    dragSourceKind.value = null;
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

    pendingDrag = {
      tabId,
      sourcePaneId: findPaneByTabId(paneLayout.value, tabId)?.paneId ?? null,
      sourceKind: "tab",
      startX: event.clientX,
      startY: event.clientY,
    };
  }

  function handleTabPointerMove(event: PointerEvent, tabId: string) {
    if (!pendingDrag || pendingDrag.tabId !== tabId || pendingDrag.sourceKind !== "tab") {
      return;
    }

    if (!draggedTabId.value) {
      const deltaX = event.clientX - pendingDrag.startX;
      const deltaY = event.clientY - pendingDrag.startY;
      if (Math.hypot(deltaX, deltaY) < 6) {
        return;
      }

      beginDrag(pendingDrag);
    }

    updateDraggedDropTarget(event.clientX, event.clientY);

    const allTabEls = document.querySelectorAll<HTMLElement>(".tab-item[data-tab-id]");
    let targetTabEl: HTMLElement | null = null;
    for (const el of allTabEls) {
      const rect = el.getBoundingClientRect();
      if (event.clientX >= rect.left && event.clientX <= rect.right && event.clientY >= rect.top && event.clientY <= rect.bottom) {
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
  }

  function handleTabPointerUp(_event: PointerEvent, tabId: string) {
    if (!pendingDrag || pendingDrag.tabId !== tabId || pendingDrag.sourceKind !== "tab") {
      return;
    }

    applyDragDrop();
    finishTabDrag();
  }

  function handleTabPointerCancel(tabId: string) {
    if (!pendingDrag || pendingDrag.tabId !== tabId || pendingDrag.sourceKind !== "tab") {
      return;
    }

    pendingDrag = null;
    draggedTabId.value = null;
    hoveredEmptyPaneId.value = null;
    dropTargetPaneId.value = null;
    dropTargetPosition.value = null;
    dragSourcePaneId.value = null;
    dragSourceKind.value = null;
  }

  function handlePaneHeaderPointerDown(event: PointerEvent, paneId: string) {
    if (event.button !== 0) {
      return;
    }

    const target = event.target as HTMLElement | null;
    if (target?.closest("button")) {
      return;
    }

    const pane = findPaneById(paneLayout.value, paneId);
    if (!pane?.tabId) {
      return;
    }

    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);

    pendingDrag = {
      tabId: pane.tabId,
      sourcePaneId: paneId,
      sourceKind: "pane",
      startX: event.clientX,
      startY: event.clientY,
    };
  }

  function handlePaneHeaderPointerMove(event: PointerEvent, paneId: string) {
    if (!pendingDrag || pendingDrag.sourceKind !== "pane" || pendingDrag.sourcePaneId !== paneId) {
      return;
    }

    if (!draggedTabId.value) {
      const deltaX = event.clientX - pendingDrag.startX;
      const deltaY = event.clientY - pendingDrag.startY;
      if (Math.hypot(deltaX, deltaY) < 6) {
        return;
      }

      beginDrag(pendingDrag);
    }

    updateDraggedDropTarget(event.clientX, event.clientY);
  }

  function handlePaneHeaderPointerUp(_event: PointerEvent, paneId: string) {
    if (!pendingDrag || pendingDrag.sourceKind !== "pane" || pendingDrag.sourcePaneId !== paneId) {
      return;
    }

    applyDragDrop();
    finishTabDrag();
  }

  function handlePaneHeaderPointerCancel(paneId: string) {
    if (!pendingDrag || pendingDrag.sourceKind !== "pane" || pendingDrag.sourcePaneId !== paneId) {
      return;
    }

    pendingDrag = null;
    draggedTabId.value = null;
    hoveredEmptyPaneId.value = null;
    dropTargetPaneId.value = null;
    dropTargetPosition.value = null;
    dragSourcePaneId.value = null;
    dragSourceKind.value = null;
  }

  function handlePaneResizePointerDown(event: PointerEvent, handle: PaneSplitHandleLayout) {
    if (event.button !== 0 || !terminalContainerRef.value) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();

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
    };

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", stopTracking);
    window.addEventListener("pointercancel", stopTracking);
    applyRatioFromPointer(event);
  }

  watch([tabs, paneLayout], () => {
    syncPaneTabHistory();
  }, { deep: true });

  return {
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
    syncFocusedPaneState,
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
    getTabById,
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
  };
}
