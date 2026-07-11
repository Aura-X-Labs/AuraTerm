import { invoke } from "@tauri-apps/api/core";

export interface CloudBridgeShare {
  localSessionId: string;
  cloudSessionId: string;
  label: string;
}

export interface CloudBridgeStatus {
  enrolled: boolean;
  connected: boolean;
  pendingUserCode?: string | null;
  shares: CloudBridgeShare[];
}

export interface CloudBridgeEnrollment {
  userCode: string;
  fingerprint: string;
  expiresIn: number;
}

export function cloudBridgeStatus(): Promise<CloudBridgeStatus> {
  return invoke("cloud_bridge_status");
}

export function beginCloudBridgeEnrollment(
  baseUrl: string, label: string, platform: string,
): Promise<CloudBridgeEnrollment> {
  return invoke("cloud_bridge_begin_enrollment", { baseUrl, label, platform });
}

export function redeemCloudBridgeEnrollment(): Promise<void> {
  return invoke("cloud_bridge_redeem_enrollment");
}

export function connectCloudBridge(): Promise<void> {
  return invoke("cloud_bridge_connect");
}

export function shareSerialToCloud(localSessionId: string, label: string): Promise<CloudBridgeShare> {
  return invoke("cloud_bridge_share_serial", { localSessionId, label });
}

export function stopCloudShare(localSessionId: string): Promise<void> {
  return invoke("cloud_bridge_stop_share", { localSessionId });
}
