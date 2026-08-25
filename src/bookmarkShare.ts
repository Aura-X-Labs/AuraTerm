import { invoke } from "@tauri-apps/api/core";

/**
 * Share-code IPC layer.
 *
 * A share code is `RRRR-SSSS-SSSS-SSSS`: a 4-character route segment AuraXLab
 * assigns, plus a 12-character secret that is generated on this machine and
 * never uploaded. The server holds ciphertext it cannot open — the same
 * zero-knowledge posture as the sync vault. All crypto and networking happen in
 * `bookmark_share.rs`; this module is a thin typed wrapper.
 */

export interface ShareTicket {
  route: string;
  /** The full code. Shown once — the secret half exists nowhere else. */
  code: string;
  link: string;
  expiresAt?: string | null;
  maxRedeems: number;
}

export interface ShareRecord {
  routeCode: string;
  state: string;
  redeemCount: number;
  maxRedeems: number;
  redeemable: boolean;
  expiresAt?: string | null;
  createdAt?: string | null;
  lastRedeemedAt?: string | null;
  /** From the local sidecar; the server never learned it. */
  label?: string | null;
}

export interface ShareOptions {
  label?: string;
  note?: string;
  ttlHours?: number;
  maxRedeems?: number;
}

/** Time-to-live choices offered in the share dialog, in hours. */
export const SHARE_TTL_CHOICES = [24, 168, 720] as const;

export function createBookmarkShare(
  root: string,
  explicitGroups: readonly string[],
  options: ShareOptions = {},
): Promise<ShareTicket> {
  return invoke<ShareTicket>("create_bookmark_share", {
    root,
    explicitGroups: [...explicitGroups],
    label: options.label ?? null,
    note: options.note ?? null,
    ttlHours: options.ttlHours ?? null,
    maxRedeems: options.maxRedeems ?? null,
  });
}

/** Fetch and decrypt a share, returning the bundle JSON to import. */
export function redeemBookmarkShare(code: string): Promise<string> {
  return invoke<string>("redeem_bookmark_share", { code });
}

export function listBookmarkShares(): Promise<ShareRecord[]> {
  return invoke<ShareRecord[]>("list_bookmark_shares");
}

export function revokeBookmarkShare(route: string): Promise<void> {
  return invoke("revoke_bookmark_share", { route });
}

/** A share this machine imported and can re-open for updates. The code itself
 *  stays in the backend's encrypted sidecar and never reaches the UI. */
export interface ShareSubscription {
  bundleId: string;
  route: string;
  label: string;
  importedAt: number;
  lastCheckedAt?: number | null;
}

/** Record a share the user just imported, so its updates are one click away. */
export function rememberBookmarkSubscription(
  code: string,
  bundleId: string,
  label: string,
): Promise<void> {
  return invoke("remember_bookmark_subscription", { code, bundleId, label });
}

export function listBookmarkSubscriptions(): Promise<ShareSubscription[]> {
  return invoke<ShareSubscription[]>("list_bookmark_subscriptions");
}

export function forgetBookmarkSubscription(bundleId: string): Promise<void> {
  return invoke("forget_bookmark_subscription", { bundleId });
}

/** Re-fetch a subscribed share; the bundle goes through the usual review. */
export function refreshBookmarkSubscription(bundleId: string): Promise<string> {
  return invoke<string>("refresh_bookmark_subscription", { bundleId });
}

/**
 * Whether `input` could be a share code rather than a file's contents: 16
 * alphanumerics from the code alphabet, however the user separated them. The
 * backend re-validates; this only decides which command to call.
 */
export function looksLikeShareCode(input: string): boolean {
  const raw = input.trim();
  const body = raw.includes("#") ? raw.slice(raw.lastIndexOf("#") + 1) : raw;
  const normalized = body.replace(/[^A-Za-z0-9]/g, "").toUpperCase();
  return normalized.length === 16 && /^[BCDFGHJKLMNPQRSTVWXZ23456789]+$/.test(normalized);
}

/** `2026-09-01 15:30` — local time, no seconds; the precision is not useful. */
export function formatShareTime(value?: string | null): string {
  if (!value) {
    return "";
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return "";
  }
  const pad = (part: number) => String(part).padStart(2, "0");
  return `${parsed.getFullYear()}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())}`
    + ` ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
}
