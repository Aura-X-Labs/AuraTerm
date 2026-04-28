import { invoke } from "@tauri-apps/api/core";
import type { Ref, ShallowRef } from "vue";
import { normalizeAppSettings, type AppSettings } from "../settings";

// Accept either Ref<T> or ShallowRef<T> so callers can pick the cheaper reactivity flavor
// when a value is always replaced wholesale (AppSettings is one such value).
type AnyRef<T> = Ref<T> | ShallowRef<T>;

interface UseWorkspacePersistenceOptions {
  settings: AnyRef<AppSettings>;
  settingsRef: AnyRef<AppSettings>;
  hasLoadedSettings: Ref<boolean>;
  createPersistedPaneLayoutState: () => unknown;
  createPersistedWorkspaceState: (restoreEnabled: boolean) => unknown;
}

export function useWorkspacePersistence({
  settings,
  settingsRef,
  hasLoadedSettings,
  createPersistedPaneLayoutState,
  createPersistedWorkspaceState,
}: UseWorkspacePersistenceOptions) {
  let persistWorkspaceStateTimer: number | null = null;
  let lastPersistedPaneLayoutSnapshot: string | null = null;
  let lastPersistedWorkspaceStateSnapshot: string | null = null;

  // Raised from 700ms: workspace snapshots only need to land on disk well after the user
  // finishes a burst of tab drags / split resizes. Coalescing more aggressively reduces
  // JSON.stringify churn on the pane tree during active interaction.
  const PERSIST_WORKSPACE_DEBOUNCE_MS = 1200;

  function snapshotState(value: unknown) {
    try {
      return JSON.stringify(value);
    } catch {
      // Fallback token to avoid blocking persistence on rare circular payloads.
      return "__snapshot_error__";
    }
  }

  function ensurePersistedStateSnapshots() {
    if (lastPersistedPaneLayoutSnapshot !== null && lastPersistedWorkspaceStateSnapshot !== null) {
      return;
    }

    lastPersistedPaneLayoutSnapshot = snapshotState(settingsRef.value.paneLayout);
    lastPersistedWorkspaceStateSnapshot = snapshotState(settingsRef.value.workspaceState);
  }

  function prepareSettingsForSave(baseSettings: AppSettings, restoreEnabled = baseSettings.restoreTabsOnStartup) {
    return normalizeAppSettings({
      ...baseSettings,
      paneLayout: createPersistedPaneLayoutState(),
      workspaceState: createPersistedWorkspaceState(restoreEnabled),
    });
  }

  function clearScheduledWorkspacePersistence() {
    if (persistWorkspaceStateTimer !== null) {
      window.clearTimeout(persistWorkspaceStateTimer);
      persistWorkspaceStateTimer = null;
    }
  }

  function persistNormalizedSettings(normalizedSettings: AppSettings) {
    lastPersistedPaneLayoutSnapshot = snapshotState(normalizedSettings.paneLayout);
    lastPersistedWorkspaceStateSnapshot = snapshotState(normalizedSettings.workspaceState);
    settingsRef.value = normalizedSettings;
    settings.value = normalizedSettings;
    void invoke("save_settings", { settings: normalizedSettings }).catch((error) => {
      console.error("save_settings failed", error);
    });
  }

  function persistSettingsSilently(newSettings: AppSettings) {
    const normalizedSettings = prepareSettingsForSave(newSettings, newSettings.restoreTabsOnStartup);
    persistNormalizedSettings(normalizedSettings);
  }

  function scheduleWorkspaceStatePersistence() {
    if (!hasLoadedSettings.value) {
      return;
    }

    clearScheduledWorkspacePersistence();
    persistWorkspaceStateTimer = window.setTimeout(() => {
      persistWorkspaceStateTimer = null;
      ensurePersistedStateSnapshots();

      const nextSettings = prepareSettingsForSave(settingsRef.value);
      const nextPaneLayoutSnapshot = snapshotState(nextSettings.paneLayout);
      const nextWorkspaceStateSnapshot = snapshotState(nextSettings.workspaceState);
      if (lastPersistedPaneLayoutSnapshot === nextPaneLayoutSnapshot
        && lastPersistedWorkspaceStateSnapshot === nextWorkspaceStateSnapshot) {
        return;
      }

      persistNormalizedSettings(nextSettings);
    }, PERSIST_WORKSPACE_DEBOUNCE_MS);
  }

  return {
    prepareSettingsForSave,
    persistSettingsSilently,
    scheduleWorkspaceStatePersistence,
    clearScheduledWorkspacePersistence,
  };
}