import { invoke } from "@tauri-apps/api/core";
import type { Ref } from "vue";
import { normalizeAppSettings, type AppSettings } from "../settings";

interface UseWorkspacePersistenceOptions {
  settings: Ref<AppSettings>;
  settingsRef: Ref<AppSettings>;
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

  const PERSIST_WORKSPACE_DEBOUNCE_MS = 700;

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