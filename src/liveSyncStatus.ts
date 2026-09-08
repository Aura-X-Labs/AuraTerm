import type { CloudBridgeStatus } from "./cloudBridge";
import type { RelayProviderStatus } from "./liveRelay";
import type { AssistStatus } from "./assist";
import { t } from "./i18n";

export type CloudPillKind = "control" | "monitor" | "online" | "standby" | "reconnecting" | "offline" | "assist";
export interface LiveStatusPill { kind: CloudPillKind; text: string }

/** The bridge is shared infrastructure: its connection alone cannot mean Console is on. */
export function cloudStatusPill(bridge: CloudBridgeStatus, enabled: boolean): LiveStatusPill | null {
  if (!bridge.enrolled) return null;
  // Backend counts already exclude Relay, independent of its grant/approval state.
  const viewers = bridge.shares.reduce((sum, share) => sum + share.consoleViewerCount, 0);
  const controllers = bridge.shares.reduce((sum, share) => sum + share.consoleControllerCount, 0);
  // Real connections remain visible while a switch-off is still taking effect,
  // or if a manually published session remains. Never hide those behind "Off".
  const activity = activityStatus("Console", controllers, viewers, 0);
  if (activity) {
    if (!enabled) activity.text += ` · ${t("cloudShare.autoShareOff")}`;
    if (!bridge.connected) activity.text += ` · ${t("cloudShare.linkUnavailable")}`;
    return activity;
  }
  if (!enabled) return { kind: "offline", text: t("cloudShare.statusDisabled") };
  if (bridge.reconnecting) return { kind: "reconnecting", text: t("cloudShare.statusReconnecting") };
  if (bridge.connected) return { kind: "online", text: t("cloudShare.statusOnline") };
  if (bridge.standby) return { kind: "standby", text: t("cloudShare.statusStandby") };
  return { kind: "offline", text: t("cloudShare.statusOffline") };
}

/** Counts are connections, not deduplicated people or devices. Requests may
 * overlap a viewer connection; pending admission does not count as a viewer. */
function activityStatus(feature: string, controllers: number, viewers: number, pending: number): LiveStatusPill | null {
  const parts: string[] = [];
  if (controllers > 0) parts.push(t("liveStatus.control", { n: controllers }));
  if (viewers > 0) parts.push(t("liveStatus.viewing", { n: viewers }));
  if (pending > 0) parts.push(t("liveStatus.pending", { n: pending }));
  if (!parts.length) return null;
  return { kind: controllers > 0 ? "control" : pending > 0 ? "assist" : "monitor", text: `${feature} · ${parts.join(" · ")}` };
}

export function assistStatusPill(assist: AssistStatus | null): LiveStatusPill | null {
  if (!assist) return null;
  const controllers = assist.guests.filter((g) => g.role === "controller").length;
  const viewers = assist.guests.filter((g) => g.role === "viewer").length;
  const pending = assist.guests.filter((g) => g.role === "pending" || g.controlRequested).length;
  return activityStatus("Share", controllers, viewers, pending)
    ?? { kind: "assist", text: t("assist.statusWaiting") };
}

export function relayStatusPill(relay: RelayProviderStatus): LiveStatusPill | null {
  const controllers = relay.peers.filter((p) => p.state === "controller").length;
  const viewers = relay.peers.filter((p) => p.state === "viewer").length;
  const pending = relay.peers.filter((p) => p.state === "pending" || p.controlRequested).length;
  return activityStatus("Relay", controllers, viewers, pending);
}
