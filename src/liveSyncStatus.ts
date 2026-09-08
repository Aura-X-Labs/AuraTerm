import type { CloudBridgeShare, CloudBridgeStatus } from "./cloudBridge";
import type { RelayProviderStatus } from "./liveRelay";
import type { AssistStatus } from "./assist";
import type { SyncConfigView } from "./cloudSync";
import { t } from "./i18n";

/**
 * Live Sync status derivation (design docs/plans/live-sync-status-bar-design.md).
 *
 * Three dimensions are modelled separately and only then turned into text:
 *
 * - **mode** — is the feature configured at all, switched off, or on?
 * - **link** — what the transport is doing, independent of the switch.
 * - **activity** — how many connections watch, hold control, or wait for an answer.
 *
 * Counts are *connections*, never deduplicated people or devices: nothing in
 * the protocol proves two connections are the same human. `stale` marks a
 * snapshot whose refresh failed — the previous numbers are kept and labelled
 * rather than dropped, so a failed poll never reads as "the share ended".
 */

export type LiveSyncFeature = "sync" | "console" | "share" | "relay";
export type FeatureMode = "unconfigured" | "off" | "on";
export type LinkState = "standby" | "connected" | "reconnecting" | "offline" | "error" | "unknown";
export type CloudPillKind =
  | "control" | "error" | "assist" | "reconnecting" | "monitor" | "online" | "standby" | "offline";

export interface LiveStatusPill {
  kind: CloudPillKind;
  text: string;
  feature: LiveSyncFeature;
}

export interface FeatureStatus {
  feature: LiveSyncFeature;
  /** Product name shown whenever this feature speaks for itself. */
  name: string;
  /** What follows the name: activity counts, or one state phrase. */
  parts: string[];
  mode: FeatureMode;
  link: LinkState;
  /** Connections that hold write access under the local gates. */
  controllers: number;
  /** Connections that can only watch. */
  viewers: number;
  /** Admission approvals plus control requests; one connection counts once. */
  pending: number;
  /** The last refresh failed; every number above is the previous snapshot. */
  stale: boolean;
  /** Always-present one-liner for the Live Sync panel row. */
  detail: string;
  /** Status-bar pill, or null when the feature is idle enough to fold into the entry. */
  pill: LiveStatusPill | null;
}

/** Sync progress, which no backend event reports; the UI owns this. */
export interface SyncRuntime {
  phase: "idle" | "syncing" | "ok" | "error";
  message: string;
  at: number | null;
}

export const IDLE_SYNC_RUNTIME: SyncRuntime = { phase: "idle", message: "", at: null };

/** Higher wins when one pill has to speak for the whole cluster. */
const SEVERITY: Record<CloudPillKind, number> = {
  control: 6, error: 5, assist: 4, reconnecting: 3, monitor: 2, online: 1, standby: 1, offline: 0,
};

function pillSeverity(kind: CloudPillKind): number {
  return SEVERITY[kind];
}

function activityParts(controllers: number, viewers: number, pending: number): string[] {
  const parts: string[] = [];
  if (controllers > 0) parts.push(t("liveStatus.control", { n: controllers }));
  if (viewers > 0) parts.push(t("liveStatus.viewing", { n: viewers }));
  if (pending > 0) parts.push(t("liveStatus.pending", { n: pending }));
  return parts;
}

/** Control outranks approval: a pending request must never mask a controller. */
function activityKind(controllers: number, viewers: number, pending: number): CloudPillKind | null {
  if (controllers > 0) return "control";
  if (pending > 0) return "assist";
  if (viewers > 0) return "monitor";
  return null;
}

/** Panel form: the name and its parts read as one list. */
function join(name: string, parts: string[]): string {
  return parts.length ? `${name} · ${parts.join(" · ")}` : name;
}

/**
 * Label form: the name binds to the first part ("Console 控制 1"), so the
 * cluster's own name can prefix the whole thing without a third level of dots.
 */
function label(name: string, parts: string[], tail: string[]): string {
  const [first, ...rest] = parts;
  return [first ? `${name} ${first}` : name, ...rest, ...tail].join(" · ");
}

/** Live Console. The bridge is shared infrastructure: its link alone is not Console. */
export function consoleStatus(bridge: CloudBridgeStatus, enabled: boolean, stale = false): FeatureStatus {
  // Backend counts already exclude Relay, independent of its grant state.
  const viewers = bridge.shares.reduce((sum, share) => sum + share.consoleViewerCount, 0);
  const controllers = bridge.shares.reduce((sum, share) => sum + share.consoleControllerCount, 0);
  const mode: FeatureMode = !bridge.enrolled ? "unconfigured" : enabled ? "on" : "off";
  const link: LinkState = !bridge.enrolled ? "unknown"
    : bridge.reconnecting ? "reconnecting"
      : bridge.connected ? "connected"
        : bridge.standby ? "standby" : "offline";
  const parts = activityParts(controllers, viewers, 0);
  // Real connections stay visible while a switch-off is still taking effect.
  // Never hide those behind "Off".
  if (parts.length) {
    if (!enabled) parts.push(t("cloudShare.autoShareOff"));
    if (link !== "connected") parts.push(t("cloudShare.linkUnavailable"));
  }
  if (!parts.length) {
    parts.push(t(
      mode === "unconfigured" ? "liveSync.consoleUnconfigured"
        : mode === "off" ? "cloudShare.statusDisabled"
          : link === "reconnecting" ? "cloudShare.statusReconnecting"
            : link === "connected" ? "cloudShare.statusOnline"
              : link === "standby" ? "cloudShare.statusStandby" : "cloudShare.statusOffline",
    ));
  }
  const activity = activityKind(controllers, viewers, 0);
  // Idle-but-healthy folds into the entry; only trouble earns a pill of its own.
  const kind: CloudPillKind | null = activity
    ?? (mode === "on" && link === "reconnecting" ? "reconnecting"
      : mode === "on" && link === "offline" ? "error" : null);
  return finish({ feature: "console", name: "Console", parts, mode, link, controllers, viewers, pending: 0, stale, kind });
}

/** Live Share. A running share stays visible even before anyone joins. */
export function shareStatus(assist: AssistStatus | null, stale = false): FeatureStatus {
  const guests = assist?.guests ?? [];
  const controllers = guests.filter((g) => g.role === "controller").length;
  const viewers = guests.filter((g) => g.role === "viewer").length;
  const pending = guests.filter((g) => g.role === "pending" || g.controlRequested).length;
  const mode: FeatureMode = assist ? "on" : "off";
  const parts = activityParts(controllers, viewers, pending);
  if (!parts.length) parts.push(t(assist ? "assist.statusWaiting" : "liveSync.shareOff"));
  const activity = activityKind(controllers, viewers, pending);
  return finish({
    feature: "share",
    name: "Share",
    parts,
    mode,
    link: assist?.locked ? "error" : assist ? "connected" : "standby",
    controllers,
    viewers,
    pending,
    stale,
    // Waiting for a guest is a share in progress, not a warning.
    kind: assist ? activity ?? "standby" : null,
  });
}

/**
 * Live Relay, provider side — other devices reaching *this* machine. The
 * consumer side (this machine reaching out) has its own terminal banner and
 * only contributes `outbound` to the panel row.
 */
export function relayStatus(relay: RelayProviderStatus, outbound = 0, stale = false): FeatureStatus {
  const controllers = relay.peers.filter((p) => p.state === "controller").length;
  const viewers = relay.peers.filter((p) => p.state === "viewer").length;
  const pending = relay.peers.filter((p) => p.state === "pending" || p.controlRequested).length;
  const parts = activityParts(controllers, viewers, pending);
  if (!parts.length) parts.push(t(relay.enabled ? "liveSync.relayIdle" : "liveSync.relayOff"));
  // The consumer side has its own terminal banner; here it is a detail only.
  if (outbound > 0) parts.push(t("liveSync.relayOutbound", { n: outbound }));
  return finish({
    feature: "relay",
    name: "Relay",
    parts,
    mode: relay.enabled ? "on" : "off",
    link: relay.peers.length ? "connected" : "standby",
    controllers,
    viewers,
    pending,
    stale,
    kind: activityKind(controllers, viewers, pending),
  });
}

/** Configuration sync: the whole run, not just its closing toast. */
export function syncStatus(
  view: SyncConfigView | null,
  runtime: SyncRuntime = IDLE_SYNC_RUNTIME,
  stale = false,
): FeatureStatus {
  const mode: FeatureMode = !view ? "unconfigured" : view.provider ? "on" : "unconfigured";
  if (mode === "unconfigured") {
    return finish({
      feature: "sync", name: "Sync", mode, link: "unknown", controllers: 0, viewers: 0, pending: 0, stale,
      parts: [t(view ? "liveSync.syncUnconfigured" : "liveSync.syncUnknown")],
      kind: null,
    });
  }
  // Ordered by what the user has to act on first.
  const [phrase, kind, link]: [string, CloudPillKind | null, LinkState] =
    runtime.phase === "syncing" ? [t("liveSync.syncRunning"), "monitor", "connected"]
      : runtime.phase === "error" ? [t("liveSync.syncFailed", { message: runtime.message }), "error", "error"]
        : !view!.passphraseUnlocked ? [t("liveSync.syncLocked"), "assist", "standby"]
          : [view!.lastSyncAt
            ? t("liveSync.syncLastOk", { time: formatSyncTime(view!.lastSyncAt) })
            : t("liveSync.syncNever"), null, "connected"];
  return finish({ feature: "sync", name: "Sync", parts: [phrase], mode, link, controllers: 0, viewers: 0, pending: 0, stale, kind });
}

function formatSyncTime(at: number): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? "—" : date.toLocaleString();
}

/**
 * Attach the stale marker last, so a failed refresh annotates the previous
 * result instead of replacing it — and keeps its pill on screen.
 */
function finish(base: Omit<FeatureStatus, "pill" | "detail"> & { kind: CloudPillKind | null }): FeatureStatus {
  const { kind, ...status } = base;
  const parts = status.stale ? [...status.parts, t("liveSync.stale")] : status.parts;
  const detail = join(status.name, parts);
  const pill = kind ? { kind, text: detail, feature: status.feature } : null;
  return { ...status, parts, detail, pill };
}

/**
 * The one label the status bar shows (design §7). The entry *is* the activity
 * chip: with nothing happening it is just "Live Sync", otherwise it carries
 * the most important feature by name and counts the rest as "+n".
 *
 * `verbose` is the wide-window form. Narrow windows drop the feature name and
 * plain viewer counts and report control, approvals and trouble across all
 * features — unless there is nothing countable to report, in which case the
 * named short form comes back so the label never goes silent.
 */
export interface LiveSyncLabel {
  kind: CloudPillKind;
  text: string;
  /** Which panel row to open; null when nothing is active. */
  feature: LiveSyncFeature | null;
}

export function liveSyncLabel(statuses: FeatureStatus[], verbose = true): LiveSyncLabel {
  const entry = t("liveSync.entry");
  const active = statuses.filter((status) => status.pill !== null);
  if (!active.length) return { kind: "offline", text: entry, feature: null };
  const primary = active.reduce(moreImportant);
  const others = active.length - 1;
  const tail = others > 0 ? [t("liveSync.labelMore", { n: others })] : [];

  if (!verbose) {
    const controllers = active.reduce((sum, status) => sum + status.controllers, 0);
    const pending = active.reduce((sum, status) => sum + status.pending, 0);
    const problems = active.filter((status) => (
      status.stale || status.pill!.kind === "error" || status.pill!.kind === "reconnecting"
    )).length;
    const counts: string[] = [];
    if (controllers > 0) counts.push(t("liveStatus.control", { n: controllers }));
    if (pending > 0) counts.push(t("liveStatus.pending", { n: pending }));
    if (problems > 0) counts.push(t("liveSync.problems", { n: problems }));
    // A state that needs an answer but has nothing to count — a locked sync —
    // would summarise to an empty string, so fall through to the named form.
    if (counts.length) {
      return { kind: primary.pill!.kind, text: [entry, ...counts].join(" · "), feature: primary.feature };
    }
  }

  return {
    kind: primary.pill!.kind,
    text: `${entry} · ${label(primary.name, primary.parts, tail)}`,
    feature: primary.feature,
  };
}

/** Severity first, then who is doing the most; ties keep the panel's order. */
function moreImportant(a: FeatureStatus, b: FeatureStatus): FeatureStatus {
  const rank = (status: FeatureStatus): number[] => [
    pillSeverity(status.pill!.kind), status.controllers, status.pending, status.viewers,
  ];
  const [left, right] = [rank(a), rank(b)];
  for (let i = 0; i < left.length; i += 1) {
    if (left[i]! !== right[i]!) return left[i]! > right[i]! ? a : b;
  }
  return a;
}

/** Hover text: nothing the single label compressed away is unreachable. */
export function liveSyncTooltip(statuses: FeatureStatus[]): string {
  return statuses.map((status) => status.detail).join("\n");
}

/** How long a remote-input burst stays on the session chip. */
export const REMOTE_TX_WINDOW_MS = 15_000;

/**
 * Session scope, so it belongs to the left half of the bar: what the *active
 * tab* has granted the cloud. Never renders the raw `txPolicy` enum.
 */
export function sessionShareLabel(
  share: CloudBridgeShare | null | undefined,
  remoteTx?: { byteCount: number; at: number } | null,
  now = Date.now(),
): string | null {
  if (!share) return null;
  const parts = [t("cloudShare.sessionShared")];
  if (!share.txAllowed) {
    parts.push(t("cloudShare.sessionReadOnly"));
  } else if (share.txPolicy === "temporary") {
    // tx_expires_at is seconds from the backend; round up so "1 min" never
    // reads as expired while input is still allowed.
    const left = share.txExpiresAt ? Math.max(0, Math.ceil((share.txExpiresAt * 1000 - now) / 60_000)) : 0;
    parts.push(t("cloudShare.sessionTemporary", { n: left }));
  } else {
    parts.push(t("cloudShare.sessionInput"));
  }
  if (remoteTx && now - remoteTx.at <= REMOTE_TX_WINDOW_MS) {
    parts.push(t("cloudShare.sessionRemoteTx", { count: remoteTx.byteCount }));
  }
  return parts.join(" · ");
}
