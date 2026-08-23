import { invoke } from "@tauri-apps/api/core";

/**
 * Cloud sync IPC layer.
 *
 * AuraTerm syncs bookmarks / settings / known-hosts as a single blob that is
 * end-to-end encrypted with a user-chosen *sync passphrase* before it ever
 * leaves the device. The storage provider (a GitHub/Gitee Gist, a WebDAV
 * server, or an AuraXLab account) only ever sees ciphertext. All crypto and
 * networking happen in the Rust backend; this module is a thin typed wrapper.
 */

export type SyncProvider = "" | "github" | "gitee" | "webdav" | "auraxlab";

export interface GistView {
  tokenSet: boolean;
  gistId: string;
}

export interface WebdavView {
  url: string;
  username: string;
  passwordSet: boolean;
}

export interface AuraxlabView {
  username: string;
  email: string;
  tokenSet: boolean;
}

/** Redacted view of the persisted sync config (secrets are reduced to flags). */
export interface SyncConfigView {
  provider: SyncProvider;
  includeSettings: boolean;
  includeKnownHosts: boolean;
  includeCredentials: boolean;
  autoSync: boolean;
  deviceId: string;
  deviceLabel: string;
  lastSyncAt: number | null;
  lastRemoteVersion: string | null;
  passphraseUnlocked: boolean;
  github: GistView;
  gitee: GistView;
  webdav: WebdavView;
  auraxlab: AuraxlabView;
}

/**
 * Editable config patch. Secret fields are `string | null`: `null` keeps the
 * stored value, `""` clears it, a value replaces it.
 */
export interface SyncSettingsInput {
  provider: SyncProvider;
  includeSettings: boolean;
  includeKnownHosts: boolean;
  includeCredentials: boolean;
  autoSync: boolean;
  deviceLabel: string;
  githubToken: string | null;
  githubGistId: string | null;
  giteeToken: string | null;
  giteeGistId: string | null;
  webdavUrl: string | null;
  webdavUsername: string | null;
  webdavPassword: string | null;
}

export interface SyncResult {
  pushed: boolean;
  pulled: boolean;
  bookmarksTotal: number;
  bookmarksAdded: number;
  knownHostsAdded: number;
  credentialsSynced: number;
  settingsApplied: boolean;
  remoteVersion: string | null;
  message: string;
}

export function getSyncConfig(): Promise<SyncConfigView> {
  return invoke<SyncConfigView>("get_sync_config");
}

export function setSyncConfig(input: SyncSettingsInput): Promise<SyncConfigView> {
  return invoke<SyncConfigView>("set_sync_config", { input });
}

export function setSyncPassphrase(passphrase: string): Promise<void> {
  return invoke("set_sync_passphrase", { passphrase });
}

export function lockSyncPassphrase(): Promise<void> {
  return invoke("lock_sync_passphrase");
}

export function isSyncUnlocked(): Promise<boolean> {
  return invoke<boolean>("is_sync_unlocked");
}

export function cloudSyncPush(passphrase?: string | null): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_push", { passphrase: passphrase ?? null });
}

export function cloudSyncPull(replace: boolean, passphrase?: string | null): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_pull", { passphrase: passphrase ?? null, replace });
}

export function cloudSyncNow(passphrase?: string | null): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_now", { passphrase: passphrase ?? null });
}

export function cloudSyncTestConnection(): Promise<string> {
  return invoke<string>("cloud_sync_test_connection");
}

export function auraxlabRequestEmailCode(email: string): Promise<string> {
  return invoke<string>("auraxlab_request_email_code", { email });
}

export function auraxlabVerifyEmailCode(
  email: string,
  code: string,
): Promise<string> {
  return invoke<string>("auraxlab_verify_email_code", { email, code });
}

export function auraxlabRegister(
  email: string,
  username: string,
  password: string,
): Promise<string> {
  return invoke<string>("auraxlab_register", { email, username, password });
}

export const PROVIDER_LABELS: Record<Exclude<SyncProvider, "">, string> = {
  github: "GitHub Gist",
  gitee: "Gitee Gist",
  webdav: "WebDAV",
  auraxlab: "AuraXLab Account",
};

// ---------------------------------------------------------------------------
// Registration validation
//
// These MUST mirror the server's rules in AuraXLab `app/api/sync.py`
// (`auraterm_sync_register`) so the client and server agree. Validating locally
// first gives immediate feedback and avoids a pointless round-trip; the server
// still re-validates (and is the only place that can detect duplicates).
// ---------------------------------------------------------------------------

export const SYNC_EMAIL_RE = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;
export const SYNC_USERNAME_RE = /^[A-Za-z][A-Za-z0-9_.]*$/;
export const SYNC_MIN_PASSWORD_LENGTH = 8;

/**
 * Validate AuraXLab account registration with the same rules the server
 * enforces. Returns an error message, or `null` when the fields are valid.
 * (Duplicate email/username can only be checked server-side.)
 */
export function validateRegistration(
  email: string,
  username: string,
  password: string,
): string | null {
  if (!SYNC_EMAIL_RE.test(email.trim())) {
    return "A valid email address is required";
  }
  if (!SYNC_USERNAME_RE.test(username.trim())) {
    return "Username must start with a letter and contain only letters, numbers, dots or underscores";
  }
  if (password.length < SYNC_MIN_PASSWORD_LENGTH) {
    return `Password must be at least ${SYNC_MIN_PASSWORD_LENGTH} characters`;
  }
  return null;
}

/** Build a fresh input patch from a view, leaving all secret fields untouched. */
export function inputFromView(view: SyncConfigView): SyncSettingsInput {
  return {
    provider: view.provider,
    includeSettings: view.includeSettings,
    includeKnownHosts: view.includeKnownHosts,
    includeCredentials: view.includeCredentials,
    autoSync: view.autoSync,
    deviceLabel: view.deviceLabel,
    githubToken: null,
    githubGistId: view.github.gistId,
    giteeToken: null,
    giteeGistId: view.gitee.gistId,
    webdavUrl: view.webdav.url,
    webdavUsername: view.webdav.username,
    webdavPassword: null,
  };
}
