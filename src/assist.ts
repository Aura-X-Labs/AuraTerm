import { invoke } from "@tauri-apps/api/core";

/** Remote Assist (design docs/plans/remote-assist-design.md) — host-side IPC. */

export type AssistControlPolicy = "view_only" | "on_request" | "auto_grant";
export type AssistProtocol = "serial" | "ssh" | "telnet" | "local";

export interface AssistPolicy {
  control: AssistControlPolicy;
  approvalRequired: boolean;
  singleUse: boolean;
  maxGuests: number;
}

export interface AssistGuest {
  connectionId: string;
  /** "pending" | "viewer" | "controller" */
  role: "pending" | "viewer" | "controller";
  client: string;
  displayName: string;
  fingerprint: string;
  joinedAt?: number | null;
  controlExpiresAt?: number | null;
  controlRequested: boolean;
}

export interface AssistStatus {
  assistId: string;
  /** Full code `XXXX-XXXX-XXXX`; the last 8 characters are the secret. */
  code: string;
  link: string;
  localSessionId: string;
  protocol: AssistProtocol;
  label: string;
  policy: AssistPolicy;
  followActiveTab: boolean;
  joinExpiresAt: number;
  joinOpen: boolean;
  createdAt: number;
  /** Server-side end of the session unless extended (unix seconds). */
  expiresAt: number;
  failedAttempts: number;
  fence: number;
  locked: boolean;
  guests: AssistGuest[];
}

export interface AssistStartOptions {
  localSessionId: string;
  protocol: AssistProtocol;
  label: string;
  controlPolicy: AssistControlPolicy;
  approvalRequired: boolean;
  singleUse: boolean;
  maxGuests: number;
  joinTtlSeconds: number;
  followActiveTab: boolean;
}

export interface AssistStartResult {
  assistId: string;
  code: string;
  link: string;
  joinExpiresAt: number;
}

export interface AssistKnock {
  connectionId: string;
  displayName: string;
  client: string;
  fingerprint: string;
  /** "join" (waiting for admission) | "control" (asks for control) */
  kind: "join" | "control";
}

export function startAssist(options: AssistStartOptions): Promise<AssistStartResult> {
  return invoke("assist_start", { ...options });
}

export function stopAssist(reason?: string): Promise<void> {
  return invoke("assist_stop", { reason: reason ?? null });
}

export function assistStatus(): Promise<AssistStatus | null> {
  return invoke("assist_status");
}

export function respondAssistJoin(connectionId: string, decision: "allow_view" | "allow_control" | "deny"): Promise<void> {
  return invoke("assist_respond_join", { connectionId, decision });
}

export function setAssistRole(connectionId: string, role: "viewer" | "controller", durationSeconds?: number): Promise<void> {
  return invoke("assist_set_role", { connectionId, role, durationSeconds: durationSeconds ?? null });
}

export function kickAssistGuest(connectionId: string): Promise<void> {
  return invoke("assist_kick", { connectionId });
}

export function revokeAllAssistControl(): Promise<void> {
  return invoke("assist_revoke_all_control");
}

export function switchAssistSession(localSessionId: string, protocol: AssistProtocol, label: string): Promise<void> {
  return invoke("assist_switch_session", { localSessionId, protocol, label });
}

export function setAssistFollowActiveTab(follow: boolean): Promise<void> {
  return invoke("assist_set_follow_active_tab", { follow });
}

/** Push the server-side lifetime cap out by `seconds` (default 1 h) from now; resolves to the new deadline. */
export function extendAssist(seconds?: number): Promise<number> {
  return invoke("assist_extend", { seconds: seconds ?? null });
}

/** Report the fitted terminal grid so cloud viewers/guests render the real size. */
export function reportTerminalSize(localSessionId: string, cols: number, rows: number): Promise<void> {
  return invoke("cloud_bridge_report_size", { localSessionId, cols, rows });
}
