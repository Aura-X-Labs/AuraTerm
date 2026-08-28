import { invoke } from "@tauri-apps/api/core";

/**
 * Live Relay IPC (design docs/plans/live-sync-design.md §5.12).
 *
 * Two halves live behind this module. As a **consumer**, this AuraTerm lists
 * the account's other bound devices and attaches to a session one of them
 * already shares — a `relay` tab whose bytes arrive end-to-end encrypted.
 * As a **provider**, it answers knocks from those devices and can kick them.
 *
 * Phase 2 is attach + view-only: the relay refuses upstream frames from a
 * non-controller peer, so a relay tab sends nothing back. Control arrives in
 * phase 3, open-a-new-session in phase 4.
 */

/** One attachable session a device already publishes to Live Console. */
export interface RelayAttachTarget {
  session_id: string;
  share_label: string | null;
  source_protocol: string | null;
  tx_policy: string | null;
  read_only: boolean;
  state: string | null;
}

/** Live Relay capability summary a device last reported to the server. */
export interface RelayPolicySummary {
  enabled: boolean;
  allow_attach: boolean;
  open_kinds: string[];
}

export interface RelayDeviceEntry {
  device_id: string;
  label: string;
  platform: string | null;
  /** "online" | "idle" | "offline" */
  presence: string;
  last_seen_at: string | null;
  relay_policy: RelayPolicySummary | null;
  attach_targets: RelayAttachTarget[];
}

export interface RelayJoinView {
  sessionId: string;
  connectionId: string;
  /** SAS both ends display so the user can compare out of band. */
  fingerprint: string;
  providerLabel: string;
}

/** Provider-side view of one peer that knocked or was admitted. */
export interface RelayPeerView {
  connectionId: string;
  label: string;
  fingerprint: string;
  shareLabel: string;
  /** "pending" | "viewer" */
  state: string;
  joinedAt: number | null;
}

export interface RelayProviderStatus {
  enabled: boolean;
  peers: RelayPeerView[];
}

/** Payload of the `relay-knock` event: a peer awaiting a local decision. */
export interface RelayKnock {
  connectionId: string;
  label: string;
  fingerprint: string;
  shareLabel: string;
}

/** Mirror of a relay tab's state, emitted as `relay-client-state:<id>`. */
export interface RelayGuestView {
  /** "handshake" | "pending_approval" | "active" | "denied" | "ended" */
  state: string;
  /** "viewer" | "controller" (always "viewer" in phase 2) */
  role: string;
  cols?: number | null;
  rows?: number | null;
  hostLabel?: string | null;
  fingerprint?: string | null;
  controlPolicy?: string | null;
  reason?: string | null;
}

// ── consumer ────────────────────────────────────────────────────────────────

/** The account's other bound devices, with their attachable sessions. */
export function relayListDevices(): Promise<RelayDeviceEntry[]> {
  return invoke("relay_list_devices");
}

/** Attach to `sessionId` on `deviceId` as the new terminal tab `id`. */
export function relayConnect(id: string, deviceId: string, sessionId: string): Promise<RelayJoinView> {
  return invoke("relay_connect", { id, deviceId, sessionId });
}

export function closeRelaySession(id: string): Promise<void> {
  return invoke("close_relay_session", { id });
}

// ── provider ────────────────────────────────────────────────────────────────

export function relayProviderStatus(): Promise<RelayProviderStatus> {
  return invoke("relay_provider_status");
}

/** Answer a knock. Denying tells the peer inside the E2EE channel. */
export function relayRespondKnock(connectionId: string, allow: boolean): Promise<void> {
  return invoke("relay_respond_knock", { connectionId, allow });
}

/** Disconnect an admitted peer (or cancel a pending knock). */
export function relayKick(connectionId: string): Promise<void> {
  return invoke("relay_kick", { connectionId });
}

/** A device is reachable when it is online and its policy allows attaching. */
export function canAttachTo(device: RelayDeviceEntry): boolean {
  return device.presence === "online"
    && device.relay_policy?.enabled === true
    && device.relay_policy?.allow_attach === true;
}
