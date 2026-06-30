import { invoke } from "@tauri-apps/api/core";

export type NetworkState =
  | "not_configured"
  | "connecting"
  | "online"
  | "degraded";

export interface PlatformSummary {
  operating_system: string;
  architecture: string;
}

export interface ReadinessSummary {
  provider_target: number;
  healthy_providers: number;
  provider_gap: number;
  slot_target: number;
  available_slots: number;
  slot_gap: number;
  verifier_target: number;
  online_verifiers: number;
  verifier_gap: number;
}

export interface DaemonStatus {
  api_version: number;
  daemon_version: string;
  uptime_seconds: number;
  participation_enabled: boolean;
  network_state: NetworkState;
  active_jobs: number;
  platform: PlatformSummary;
  readiness: ReadinessSummary | null;
}

export function getDaemonStatus(): Promise<DaemonStatus> {
  return invoke<DaemonStatus>("daemon_status");
}

export function launchDaemon(): Promise<DaemonStatus> {
  return invoke<DaemonStatus>("launch_daemon");
}

export function setParticipation(enabled: boolean): Promise<DaemonStatus> {
  return invoke<DaemonStatus>("set_participation", { enabled });
}
