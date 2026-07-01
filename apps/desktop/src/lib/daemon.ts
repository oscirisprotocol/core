import { invoke } from "@tauri-apps/api/core";

export type NetworkState =
  | "not_configured"
  | "connecting"
  | "online"
  | "degraded";

export type JobKind = "training" | "inference";
export type JobState =
  | "draft"
  | "awaiting_funding"
  | "queued"
  | "matching"
  | "running"
  | "verifying"
  | "completed"
  | "failed";
export type PrivacyMode =
  | "raw_baseline"
  | "dsp_prepared"
  | "dp_model_release";

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

export interface CreateJobInput {
  kind: JobKind;
  title: string;
  model_id: string;
  workload: string;
  privacy_mode: PrivacyMode;
  hardware_profile: string;
  required_verifier_count: number;
  challenge_window_seconds: number;
  budget_usdc_micros: number;
}

export interface EvidenceIngestionInput {
  job_id: string;
  evidence_dir: string;
}

export interface JobEvidenceSummary {
  execution_receipt_sha256: string | null;
  verification_status: string | null;
  verifier_count: number;
  bundle_sha256: string | null;
  chain_tx_hash: string | null;
}

export interface DesktopJob extends CreateJobInput {
  job_id: string;
  state: JobState;
  progress_percent: number;
  provider_node_id: string | null;
  created_at: string;
  updated_at: string;
  evidence: JobEvidenceSummary;
}

export interface WalletConfigInput {
  address: string;
  settlement_token_address: string | null;
  settlement_token_symbol: string;
  settlement_token_decimals: number;
}

export interface TokenBalance {
  symbol: string;
  contract_address: string;
  decimals: number;
  balance_atomic: string;
}

export interface WalletStatus {
  configured: boolean;
  network_name: string;
  chain_id: number;
  rpc_url: string;
  explorer_url: string;
  address: string | null;
  native_balance_wei: string | null;
  settlement_token: TokenBalance | null;
  committed_usdc_micros: number;
  last_synced_at: string | null;
  sync_error: string | null;
  custody_mode: string;
}

export interface WorkspaceSnapshot {
  jobs: DesktopJob[];
  wallet: WalletStatus;
  protocol_announcement_count: number;
}

export interface WithdrawalInput {
  recipient: string;
  amount_atomic: string;
}

export interface UnsignedTokenTransfer {
  chain_id: number;
  from: string;
  to: string;
  value: string;
  data: string;
  amount_atomic: string;
  symbol: string;
  signing_instruction: string;
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

export function getWorkspace(): Promise<WorkspaceSnapshot> {
  return invoke<WorkspaceSnapshot>("workspace_snapshot");
}

export function createJob(input: CreateJobInput): Promise<DesktopJob> {
  return invoke<DesktopJob>("create_job", { input });
}

export function submitJob(jobId: string): Promise<DesktopJob> {
  return invoke<DesktopJob>("submit_job", { jobId });
}

export function publishJob(jobId: string): Promise<DesktopJob> {
  return invoke<DesktopJob>("publish_job", { jobId });
}

export function matchProvider(jobId: string): Promise<WorkspaceSnapshot> {
  return invoke<WorkspaceSnapshot>("match_provider", { jobId });
}

export function refreshProtocolJobs(): Promise<WorkspaceSnapshot> {
  return invoke<WorkspaceSnapshot>("refresh_protocol_jobs");
}

export function ingestEvidence(
  input: EvidenceIngestionInput,
): Promise<WorkspaceSnapshot> {
  return invoke<WorkspaceSnapshot>("ingest_evidence", { input });
}

export function configureWallet(
  input: WalletConfigInput,
): Promise<WalletStatus> {
  return invoke<WalletStatus>("configure_wallet", { input });
}

export function refreshWallet(): Promise<WalletStatus> {
  return invoke<WalletStatus>("refresh_wallet");
}

export function prepareWithdrawal(
  input: WithdrawalInput,
): Promise<UnsignedTokenTransfer> {
  return invoke<UnsignedTokenTransfer>("prepare_withdrawal", { input });
}
