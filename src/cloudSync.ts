import { invoke } from "@tauri-apps/api/core";

/**
 * Configuration sync IPC layer (design docs/plans/sync-passphrase-removal-design.md).
 *
 * Signing in to the AuraXLab account *is* the configuration: the backend
 * keeps the scoped sync credential, and there is no separate sync passphrase
 * any more. Bookmarks, the settings subset and known-hosts travel as plain
 * JSON that the server encrypts at rest; saved credentials are sealed under
 * the master password before upload (`AURACRED`) and the server never opens
 * them. All crypto and networking happen in the Rust backend; this module is
 * a thin typed wrapper.
 */

export type SyncProvider = "" | "auraxlab";

/** Which key protects the local credential store; only the former can sync credentials. */
export type CredentialsMode = "masterPassword" | "localKey";

/** Why the credentials part of a sync did not run (`cloud_sync::skip`). */
export type CredentialsSkipReason = "localKeyMode" | "masterLocked" | "mismatch" | "corrupt";

export interface AuraxlabView {
  username: string;
  email: string;
  tokenSet: boolean;
}

/** Redacted view of the persisted sync config (the credential is reduced to a flag). */
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
  credentialsMode: CredentialsMode;
  masterUnlocked: boolean;
  /** A removed provider (Gist / WebDAV) was found in the stored config. */
  legacyProviderNotice: boolean;
  auraxlab: AuraxlabView;
}

/** Editable config patch. Provider and credentials are not part of it. */
export interface SyncSettingsInput {
  includeSettings: boolean;
  includeKnownHosts: boolean;
  includeCredentials: boolean;
  autoSync: boolean;
  deviceLabel: string;
}

export interface SyncResult {
  pushed: boolean;
  pulled: boolean;
  bookmarksTotal: number;
  bookmarksAdded: number;
  knownHostsAdded: number;
  credentialsSynced: number;
  credentialsSkipped: CredentialsSkipReason | null;
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

export function acknowledgeLegacyProviderNotice(): Promise<void> {
  return invoke("acknowledge_legacy_provider_notice");
}

export function cloudSyncPush(): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_push");
}

export function cloudSyncPull(replace: boolean): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_pull", { replace });
}

export function cloudSyncNow(): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_now");
}

export function cloudSyncTestConnection(): Promise<string> {
  return invoke<string>("cloud_sync_test_connection");
}

/**
 * One-time migration of a vault uploaded by a passphrase-era build. Either
 * decrypt it with the old passphrase and merge, or overwrite it with this
 * device's data. Removed in Phase 3 together with the backend module.
 */
export function cloudSyncMigrateLegacy(passphrase: string | null, overwrite: boolean): Promise<SyncResult> {
  return invoke<SyncResult>("cloud_sync_migrate_legacy", { passphrase, overwrite });
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

// ---------------------------------------------------------------------------
// Error classification
//
// The backend returns plain strings; a few of them drive UI decisions (open
// the account center, open the migration step). The patterns mirror the
// ERR_* constants in `src-tauri/src/cloud_sync.rs` — keep them in sync.
// ---------------------------------------------------------------------------

export type SyncErrorKind = "signIn" | "notSignedIn" | "legacyVault" | "other";

export function classifySyncError(message: unknown): SyncErrorKind {
  const text = String(message);
  if (/old sync passphrase format/i.test(text)) return "legacyVault";
  if (/sign in to your auraxlab account again/i.test(text)) return "signIn";
  if (/sign in to your auraxlab account first/i.test(text)) return "notSignedIn";
  return "other";
}

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

/** Build a fresh input patch from a view. */
export function inputFromView(view: SyncConfigView): SyncSettingsInput {
  return {
    includeSettings: view.includeSettings,
    includeKnownHosts: view.includeKnownHosts,
    includeCredentials: view.includeCredentials,
    autoSync: view.autoSync,
    deviceLabel: view.deviceLabel,
  };
}
