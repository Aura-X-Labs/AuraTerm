import type { SearchAddon, ISearchOptions } from "@xterm/addon-search";
import type { Terminal } from "@xterm/xterm";
import { ref, type Ref } from "vue";
import type { TerminalSearchOptions, TerminalSearchResults } from "../types";

const SEARCH_DECORATIONS = {
  matchOverviewRuler: "transparent",
  activeMatchBackground: "#FFB347",
  activeMatchBorder: "#FFD38A",
  activeMatchColorOverviewRuler: "#FFB347",
};

interface UseTerminalSearchOptions {
  terminal: Ref<Terminal | null>;
  searchAddon: Ref<SearchAddon | null>;
  onResultsChange: (results: TerminalSearchResults) => void;
}

export function useTerminalSearch({ terminal, searchAddon, onResultsChange }: UseTerminalSearchOptions) {
  const activeSearchQuery = ref("");

  function emitSearchResults(results: TerminalSearchResults) {
    onResultsChange(results);
  }

  function clearSearch() {
    activeSearchQuery.value = "";
    searchAddon.value?.clearDecorations();
    emitSearchResults({
      query: "",
      resultIndex: -1,
      resultCount: 0,
      limitExceeded: false,
    });
  }

  function clearSearchActiveDecoration() {
    searchAddon.value?.clearActiveDecoration();
  }

  function runSearch(direction: "next" | "previous", term: string, options: TerminalSearchOptions = {}) {
    if (!searchAddon.value || !terminal.value) {
      return false;
    }

    activeSearchQuery.value = term;
    if (!term) {
      clearSearch();
      return false;
    }

    const searchOptions: ISearchOptions = {
      caseSensitive: options.caseSensitive,
      wholeWord: options.wholeWord,
      regex: options.regex,
      incremental: direction === "next" ? options.incremental : false,
      decorations: SEARCH_DECORATIONS,
    };

    try {
      const matched = direction === "previous"
        ? searchAddon.value.findPrevious(term, searchOptions)
        : searchAddon.value.findNext(term, searchOptions);

      emitSearchResults({
        query: term,
        resultIndex: matched ? 0 : -1,
        resultCount: matched ? 1 : 0,
        limitExceeded: false,
      });

      return matched;
    } catch (error) {
      console.warn("Search failed (possibly invalid regex):", error);
      emitSearchResults({ query: term, resultIndex: -1, resultCount: 0, limitExceeded: false });
      return false;
    }
  }

  return {
    activeSearchQuery,
    clearSearch,
    clearSearchActiveDecoration,
    runSearch,
  };
}