import { invoke } from "@tauri-apps/api/core";

export type AccountConsistency =
  | "signed_out"
  | "consistent"
  | "sync_only"
  | "device_only"
  | "mismatch";

export interface AccountTraffic {
  bytesUp: number;
  bytesDown: number;
  bytesTotal: number;
  sessions: number;
}

export interface ConsoleAccountState {
  enrolled: boolean;
  connected: boolean;
  deviceId: string | null;
  deviceLabel: string | null;
}

export interface AuraXLabAccountState {
  signedIn: boolean;
  accountSubject: string | null;
  email: string;
  username: string;
  confirmed: boolean;
  syncCredentialSet: boolean;
  consistency: AccountConsistency;
  console: ConsoleAccountState;
  traffic: AccountTraffic | null;
}

export interface AccountLoginInput {
  email: string;
  password: string;
  deviceLabel: string;
  platform: string;
  enableConsole: boolean;
}

export function accountState(): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_state");
}

export function restoreAccount(): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_restore");
}

export function accountLogin(input: AccountLoginInput): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_login", { ...input });
}

export function accountLogout(): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_logout");
}

export function enableConsole(
  email: string,
  password: string,
  deviceLabel: string,
  platform: string,
): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_enable_console", { email, password, deviceLabel, platform });
}

export function pauseConsole(): Promise<AuraXLabAccountState> {
  return invoke("auraxlab_account_pause_console");
}
