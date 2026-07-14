import { invoke } from "@tauri-apps/api/core";

export interface CloudBridgeShare {
  localSessionId: string;
  cloudSessionId: string;
  label: string;
  protocol: "serial" | "ssh" | "telnet" | "local";
  txPolicy: "read_only" | "read_write" | "temporary";
  txExpiresAt?: number | null;
  txAllowed: boolean;
}

export interface CloudBridgeStatus {
  enrolled: boolean;
  connected: boolean;
  reconnecting: boolean;
  /** Enrolled and online via presence pings, holding no relay connection. */
  standby: boolean;
  pendingUserCode?: string | null;
  deviceId?: string | null;
  deviceLabel?: string | null;
  baseUrl?: string | null;
  fingerprint?: string | null;
  shares: CloudBridgeShare[];
}

export interface CloudBridgeEnrollment {
  userCode: string;
  fingerprint: string;
  expiresIn: number;
}

export type RedeemStatus = "ok" | "pending" | "denied" | "expired";

export function cloudBridgeStatus(): Promise<CloudBridgeStatus> {
  return invoke("cloud_bridge_status");
}

export function beginCloudBridgeEnrollment(
  baseUrl: string, label: string, platform: string,
): Promise<CloudBridgeEnrollment> {
  return invoke("cloud_bridge_begin_enrollment", { baseUrl, label, platform });
}

/** Login-and-bind: approve the pending enrollment with the account
 * password. The password is used for this single request, never stored. */
export function authorizeCloudBridgeEnrollment(
  email: string, password: string,
): Promise<void> {
  return invoke("cloud_bridge_authorize_enrollment", { email, password });
}

export function redeemCloudBridgeEnrollment(): Promise<{ status: RedeemStatus }> {
  return invoke("cloud_bridge_redeem_enrollment");
}

export function connectCloudBridge(): Promise<void> {
  return invoke("cloud_bridge_connect");
}

/** Restore the persisted device identity (if any) and reconnect in the
 * background. Returns whether a device identity was found. */
export function restoreCloudBridge(): Promise<boolean> {
  return invoke("cloud_bridge_restore");
}

/** Server-side self-revocation plus local credential removal. */
export function unbindCloudBridge(): Promise<void> {
  return invoke("cloud_bridge_unbind");
}

/** Rotate the device credential + identity key (old key signs the rotation). */
export function rotateCloudBridgeCredential(): Promise<void> {
  return invoke("cloud_bridge_rotate_credential");
}

export function shareSessionToCloud(
  localSessionId: string,
  protocol: "serial" | "ssh" | "telnet" | "local",
  label: string,
  txPolicy: "read_only" | "read_write" | "temporary",
  txExpiresInSeconds?: number,
  rxRingBytes = 256 * 1024,
): Promise<CloudBridgeShare> {
  return invoke("cloud_bridge_share_session", {
    localSessionId, protocol, label, txPolicy, txExpiresInSeconds, rxRingBytes,
  });
}

export function stopCloudShare(localSessionId: string): Promise<void> {
  return invoke("cloud_bridge_stop_share", { localSessionId });
}

/** Mirror the persisted "Allow Remote Send" setting into the bridge. Until
 * the first push the bridge fails closed and drops all remote INPUT. */
export function setCloudBridgeAllowRemoteSend(allowed: boolean): Promise<void> {
  return invoke("cloud_bridge_set_allow_remote_send", { allowed });
}
