import { nextTick, ref, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { PaneLayoutTab } from "../usePaneLayout";
import type { SerialParams, SessionConfig } from "../types";
import { serialConfigOf } from "../serialTransport";

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

/** `8N1` — takes bare params so it works on a live status as well as a config. */
export function formatSerialFrame(params: SerialParams) {
  const parity = params.parity === "none" ? "N" : params.parity === "even" ? "E" : "O";
  return `${params.dataBits}${parity}${params.stopBits}`;
}

export function buildBaseTabTitle(session: SessionConfig): string {
  const serialConfig = serialConfigOf(session);
  if (serialConfig) {
    return `${serialConfig.portName} @ ${serialConfig.baudRate}`;
  }
  if (session.protocol === "telnet") {
    return `telnet://${session.telnetConfig.host}:${session.telnetConfig.port}`;
  }
  if (session.protocol === "ssh") {
    return `${session.sshConfig.user}@${session.sshConfig.host}`;
  }
  return "Local Shell";
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

interface UseTabManagerOptions {
  /** The reactive tab list (owned by `App.vue`, shared with `usePaneLayout`). */
  tabs: Ref<PaneLayoutTab[]>;
  /** Active tab id (from `usePaneLayout`); rename focuses the renamed tab. */
  activeTabId: Ref<string>;
  /** Dismiss the tab context menu when a rename begins. */
  closeTabContextMenu: () => void;
}

/**
 * Tab title generation/de-duplication, inline rename flow, and the monotonic
 * tab-id counter. Extracted from `App.vue` so the orchestrator no longer carries
 * the NATO-suffix uniqueness logic or rename state machine.
 */
export function useTabManager({ tabs, activeTabId, closeTabContextMenu }: UseTabManagerOptions) {
  let nextTabId = 1;
  const renamingTabId = ref<string | null>(null);
  const renamingTabTitle = ref("");

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

  function createSessionTab(
    tabId: string,
    session: SessionConfig,
    bookmarkName?: string,
    logPath?: string,
  ): PaneLayoutTab {
    return {
      id: tabId,
      title: generateUniqueTabTitle(bookmarkName, buildBaseTabTitle(session)),
      session,
      logPath,
    };
  }

  /** Mint a fresh, never-reused tab id. */
  function mintTabId() {
    return `tab-${nextTabId++}`;
  }

  function syncTabIdCounter(sourceTabs: Array<{ id: string }>) {
    let maxTabIndex = -1;

    for (const item of sourceTabs) {
      const numericValue = item.id.startsWith("tab-") ? Number.parseInt(item.id.slice(4), 10) : Number.NaN;
      maxTabIndex = Math.max(maxTabIndex, Number.isFinite(numericValue) ? numericValue : -1);
    }

    nextTabId = Math.max(nextTabId, maxTabIndex + 1);
  }

  function startTabRename(tabId: string) {
    const tab = tabs.value.find((item) => item.id === tabId);
    if (!tab) {
      return;
    }

    closeTabContextMenu();
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

  return {
    renamingTabId,
    renamingTabTitle,
    generateUniqueTabTitle,
    createSessionTab,
    buildBaseTabTitle,
    formatSerialFrame,
    mintTabId,
    syncTabIdCounter,
    startTabRename,
    cancelTabRename,
    commitTabRename,
    handleTabRenameKeyDown,
  };
}
