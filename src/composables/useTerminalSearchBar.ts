import { computed, nextTick, onScopeDispose, ref, shallowRef, watch, type Ref } from "vue";
import type { TerminalHandle, TerminalSearchOptions, TerminalSearchResults } from "../types";

type TerminalSearchToggleKey = "caseSensitive" | "wholeWord" | "regex";

const EMPTY_TERMINAL_SEARCH_RESULTS: TerminalSearchResults = {
  query: "",
  resultIndex: -1,
  resultCount: 0,
  limitExceeded: false,
};

interface UseTerminalSearchBarOptions {
  /** The find bar's `<input>`, bound by App.vue's template. */
  terminalSearchInputRef: Ref<HTMLInputElement | null>;
  activeTabId: Ref<string>;
  /** Live terminal handles by tab id (owned by App.vue). */
  termRefs: Map<string, TerminalHandle>;
  hasActiveTab: () => boolean;
  closeOpenMenus: () => void;
  focusActiveTerminal: () => void;
  /** The bar takes vertical space, so terminals re-fit when it toggles. */
  fitVisibleTerminals: () => void;
}

/**
 * The app-level find bar: query/toggle state, per-tab results, and driving
 * the active terminal's search. The per-terminal search addon lives in
 * `useTerminalSearch` (inside TerminalComponent).
 */
export function useTerminalSearchBar({
  terminalSearchInputRef,
  activeTabId,
  termRefs,
  hasActiveTab,
  closeOpenMenus,
  focusActiveTerminal,
  fitVisibleTerminals,
}: UseTerminalSearchBarOptions) {
  const terminalSearchVisible = ref(false);
  const terminalSearchQuery = ref("");
  const terminalSearchOptions = ref<Required<Pick<TerminalSearchOptions, TerminalSearchToggleKey>>>({
    caseSensitive: false,
    wholeWord: false,
    regex: false,
  });
  // Replaced wholesale on every change, so a shallow ref is enough.
  const terminalSearchResults = shallowRef<Record<string, TerminalSearchResults>>({});
  let searchDebounceTimer: number | null = null;

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

  /** A terminal just mounted: if it is the active one, search it right away. */
  function handleTerminalAttached(tabId: string) {
    if (terminalSearchVisible.value && activeTabId.value === tabId) {
      void nextTick(() => {
        syncSearchToActiveTerminal("next", true);
      });
    }
  }

  function handleOpenTerminalSearch() {
    closeOpenMenus();
    if (!hasActiveTab()) {
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

  onScopeDispose(() => {
    if (searchDebounceTimer !== null) {
      window.clearTimeout(searchDebounceTimer);
      searchDebounceTimer = null;
    }
  });

  return {
    terminalSearchVisible,
    terminalSearchQuery,
    terminalSearchOptions,
    terminalSearchSummary,
    focusTerminalSearchInput,
    updateTerminalSearchResults,
    removeTerminalSearchResults,
    handleTerminalAttached,
    handleOpenTerminalSearch,
    handleCloseTerminalSearch,
    handleFindNextInTerminal,
    handleFindPreviousInTerminal,
    toggleTerminalSearchOption,
    handleTerminalSearchKeyDown,
    handleTerminalSearchBlur,
  };
}
