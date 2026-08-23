import { invoke } from "@tauri-apps/api/core";

export interface CloudBridgeShare {
  localSessionId: string;
  cloudSessionId: string;
  label: string;
  protocol: "serial" | "ssh" | "telnet" | "local";
  txPolicy: "read_only" | "read_write" | "temporary";
  txExpiresAt?: number | null;
  txAllowed: boolean;
  /** Attached E2EE peers (viewers and controllers alike). */
  viewerCount: number;
  /** At least one attached peer connected in the controller role. */
  controllerAttached: boolean;
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
  accountSubject?: string | null;
  fingerprint?: string | null;
  shares: CloudBridgeShare[];
}

export function cloudBridgeStatus(): Promise<CloudBridgeStatus> {
  return invoke("cloud_bridge_status");
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
