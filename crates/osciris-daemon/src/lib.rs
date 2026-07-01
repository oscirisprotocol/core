use std::{
    env, fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use alloy_primitives::{Address, U256};
use anyhow::{anyhow, bail, Context, Result};
use atomic_write_file::AtomicWriteFile;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use tokio_util::codec::{Framed, LinesCodec};

use osciris_core::{
    bundle_hash, load_signing_key_from_base64_seed, sha256_file, sign_job_announcement,
    sign_job_assignment, verify_execution_receipt_signature, verify_job_announcement_signature,
    verify_job_claim_signature, verify_provider_capability_signature,
    verify_receipt_availability_signature, verify_verification_receipt_signature,
    verifying_key_from_base64, verifying_key_to_base64, ExecutionReceipt, JobAnnouncement,
    JobAssignment, JobClaim, JobSpec, JobType, NodeIdentity, NodeRole, NodeStatus, PrivacyMode,
    PrivacyPolicy, ProviderCapability, ReceiptAvailability, ReceiptBundle,
    VerificationReceiptAnnouncement,
};
use osciris_node::network::{
    create_inference_request, job_matches_provider_capability, serve_presence,
    wait_for_inference_response, InferenceSubmitConfig, InferenceWaitConfig, NetworkServeConfig,
};
use osciris_node::status::{build_inference_readiness_report, calculate_quorum_status};
use osciris_node::store::ProtocolStore;

pub const API_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const HORIZEN_TESTNET_CHAIN_ID: u64 = 2_651_420;
pub const HORIZEN_TESTNET_RPC_URL: &str = "https://horizen-testnet.rpc.caldera.xyz/http";
pub const HORIZEN_TESTNET_EXPLORER_URL: &str = "https://horizen-testnet.explorer.caldera.xyz";

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const DESKTOP_INFERENCE_PROFILE_ID: &str = "osciris-qwen3-4b-q4-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalEndpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
}

impl LocalEndpoint {
    pub fn default_for_user() -> Self {
        if let Ok(endpoint) = env::var("OSCIRIS_DAEMON_ENDPOINT") {
            return Self::from_override(endpoint);
        }

        #[cfg(unix)]
        {
            Self::Unix(default_state_dir().join("daemon.sock"))
        }

        #[cfg(windows)]
        {
            let user = env::var("USERNAME")
                .unwrap_or_else(|_| "user".to_string())
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() {
                        character
                    } else {
                        '-'
                    }
                })
                .collect::<String>();
            Self::NamedPipe(format!(r"\\.\pipe\osciris-node-{user}"))
        }
    }

    pub fn from_override(value: String) -> Self {
        #[cfg(unix)]
        {
            Self::Unix(PathBuf::from(value))
        }

        #[cfg(windows)]
        {
            Self::NamedPipe(value)
        }
    }
}

impl fmt::Display for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => write!(formatter, "{}", path.display()),
            #[cfg(windows)]
            Self::NamedPipe(name) => formatter.write_str(name),
        }
    }
}

pub fn default_state_dir() -> PathBuf {
    if let Some(path) = env::var_os("OSCIRIS_STATE_DIR") {
        return PathBuf::from(path);
    }

    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library")
            .join("Application Support")
            .join("OSCIRIS")
    }

    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join("OSCIRIS")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".local").join("state"))
            .join("osciris")
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonRequest {
    pub api_version: u16,
    pub request_id: u64,
    pub auth_token: String,
    pub command: DaemonCommand,
}

impl DaemonRequest {
    pub fn new(auth_token: String, command: DaemonCommand) -> Self {
        Self {
            api_version: API_VERSION,
            request_id: NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            auth_token,
            command,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonCommand {
    Ping,
    GetStatus,
    GetWorkspace,
    SetParticipation {
        enabled: bool,
    },
    StartNetwork {
        input: NetworkControlInput,
    },
    StopNetwork,
    CreateJob {
        input: CreateJobInput,
    },
    SubmitJob {
        job_id: String,
    },
    PublishJob {
        job_id: String,
    },
    MatchProvider {
        job_id: String,
    },
    RefreshProtocolJobs,
    IngestEvidence {
        input: EvidenceIngestionInput,
    },
    ImportVerificationReceipt {
        input: VerificationReceiptImportInput,
    },
    SubmitInference {
        input: InferencePromptInput,
    },
    ConfigureWallet {
        input: WalletConfigInput,
    },
    RefreshWallet,
    PrepareWithdrawal {
        input: WithdrawalInput,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonResponse {
    pub api_version: u16,
    pub request_id: u64,
    pub result: Option<DaemonResult>,
    pub error: Option<DaemonError>,
}

impl DaemonResponse {
    fn success(request_id: u64, result: DaemonResult) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: Some(result),
            error: None,
        }
    }

    fn error(request_id: u64, code: &str, message: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION,
            request_id,
            result: None,
            error: Some(DaemonError {
                code: code.to_string(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DaemonResult {
    Pong { daemon_version: String },
    Status(DaemonStatus),
    Workspace(WorkspaceSnapshot),
    Network(NetworkControlResult),
    Job(DesktopJob),
    Wallet(WalletStatus),
    Withdrawal(UnsignedTokenTransfer),
    Inference(InferencePromptResult),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatus {
    pub api_version: u16,
    pub daemon_version: String,
    pub uptime_seconds: u64,
    pub participation_enabled: bool,
    pub network_state: NetworkState,
    pub network_listen_addr: Option<String>,
    pub network_bootstrap_peers: Vec<String>,
    pub network_error: Option<String>,
    pub active_jobs: u32,
    pub platform: PlatformSummary,
    pub readiness: Option<ReadinessSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkState {
    NotConfigured,
    Connecting,
    Online,
    Degraded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformSummary {
    pub operating_system: String,
    pub architecture: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessSummary {
    pub provider_target: u32,
    pub healthy_providers: u32,
    pub provider_gap: u32,
    pub slot_target: u32,
    pub available_slots: u32,
    pub slot_gap: u32,
    pub verifier_target: u32,
    pub online_verifiers: u32,
    pub verifier_gap: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopJobKind {
    Training,
    Inference,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopJobState {
    Draft,
    AwaitingFunding,
    Queued,
    Matching,
    Running,
    Verifying,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopPrivacyMode {
    RawBaseline,
    DspPrepared,
    DpModelRelease,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateJobInput {
    pub kind: DesktopJobKind,
    pub title: String,
    pub model_id: String,
    pub workload: String,
    pub privacy_mode: DesktopPrivacyMode,
    pub hardware_profile: String,
    pub required_verifier_count: u8,
    pub challenge_window_seconds: u64,
    pub budget_usdc_micros: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobEvidenceSummary {
    pub execution_receipt_sha256: Option<String>,
    pub verification_status: Option<String>,
    pub verifier_count: u8,
    pub bundle_sha256: Option<String>,
    pub chain_tx_hash: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopJob {
    pub job_id: String,
    pub kind: DesktopJobKind,
    pub title: String,
    pub model_id: String,
    pub workload: String,
    pub privacy_mode: DesktopPrivacyMode,
    pub hardware_profile: String,
    pub required_verifier_count: u8,
    pub challenge_window_seconds: u64,
    pub budget_usdc_micros: u64,
    pub state: DesktopJobState,
    pub progress_percent: u8,
    pub provider_node_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub evidence: JobEvidenceSummary,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletConfigInput {
    pub address: String,
    pub settlement_token_address: Option<String>,
    pub settlement_token_symbol: String,
    pub settlement_token_decimals: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletConfig {
    pub address: String,
    pub settlement_token_address: Option<String>,
    pub settlement_token_symbol: String,
    pub settlement_token_decimals: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenBalance {
    pub symbol: String,
    pub contract_address: String,
    pub decimals: u8,
    pub balance_atomic: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WalletStatus {
    pub configured: bool,
    pub network_name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub explorer_url: String,
    pub address: Option<String>,
    pub native_balance_wei: Option<String>,
    pub settlement_token: Option<TokenBalance>,
    pub committed_usdc_micros: u64,
    pub last_synced_at: Option<String>,
    pub sync_error: Option<String>,
    pub custody_mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WithdrawalInput {
    pub recipient: String,
    pub amount_atomic: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnsignedTokenTransfer {
    pub chain_id: u64,
    pub from: String,
    pub to: String,
    pub value: String,
    pub data: String,
    pub amount_atomic: String,
    pub symbol: String,
    pub signing_instruction: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceIngestionInput {
    pub job_id: String,
    pub evidence_dir: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationReceiptImportInput {
    pub job_id: String,
    pub receipt_json_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkControlInput {
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkControlResult {
    pub status: DaemonStatus,
    pub peer_id: Option<String>,
    pub listen_addr: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InferencePromptInput {
    pub requester_id: String,
    pub profile_id: String,
    pub prompt: String,
    pub max_output_tokens: u32,
    pub provider_peer_id: String,
    pub bootstrap_peers: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InferencePromptResult {
    pub request_id: String,
    pub profile_id: String,
    pub provider_node_id: String,
    pub response_text: String,
    pub request_sha256: String,
    pub response_sha256: String,
    pub prompt_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u64,
    pub evidence_dir: String,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub jobs: Vec<DesktopJob>,
    pub wallet: WalletStatus,
    pub protocol_announcement_count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct PersistedState {
    participation_enabled: bool,
    jobs: Vec<DesktopJob>,
    wallet: Option<WalletConfig>,
    wallet_status: Option<WalletStatus>,
}

#[derive(Clone)]
pub struct DaemonService {
    inner: Arc<DaemonServiceInner>,
}

struct DaemonServiceInner {
    started_at: Instant,
    state_dir: PathBuf,
    auth_token: String,
    state: RwLock<PersistedState>,
    network: Mutex<NetworkTaskState>,
    http: reqwest::Client,
    protocol_identity: ed25519_dalek::SigningKey,
}

#[derive(Default)]
struct NetworkTaskState {
    handle: Option<JoinHandle<()>>,
    listen_addr: Option<String>,
    bootstrap_peers: Vec<String>,
    last_error: Option<String>,
}

impl DaemonService {
    pub fn new(state_dir: PathBuf) -> Result<Self> {
        secure_state_dir(&state_dir)?;
        let state = load_state(&state_dir)?;
        let auth_token = ensure_auth_token(&state_dir)?;
        let protocol_identity = load_or_create_protocol_identity(&state_dir)?;
        let http = reqwest::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(6))
            .build()
            .context("build Horizen RPC client")?;
        Ok(Self {
            inner: Arc::new(DaemonServiceInner {
                started_at: Instant::now(),
                state_dir,
                auth_token,
                state: RwLock::new(state),
                network: Mutex::new(NetworkTaskState::default()),
                http,
                protocol_identity,
            }),
        })
    }

    pub async fn handle(&self, request: DaemonRequest) -> DaemonResponse {
        if request
            .auth_token
            .as_bytes()
            .ct_eq(self.inner.auth_token.as_bytes())
            .unwrap_u8()
            != 1
        {
            return DaemonResponse::error(
                request.request_id,
                "authentication_failed",
                "invalid local daemon credential",
            );
        }
        if request.api_version != API_VERSION {
            return DaemonResponse::error(
                request.request_id,
                "unsupported_api_version",
                format!(
                    "daemon supports API version {API_VERSION}, request used {}",
                    request.api_version
                ),
            );
        }

        match request.command {
            DaemonCommand::Ping => DaemonResponse::success(
                request.request_id,
                DaemonResult::Pong {
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                },
            ),
            DaemonCommand::GetStatus => DaemonResponse::success(
                request.request_id,
                DaemonResult::Status(self.status().await),
            ),
            DaemonCommand::GetWorkspace => DaemonResponse::success(
                request.request_id,
                DaemonResult::Workspace(self.workspace().await),
            ),
            DaemonCommand::SetParticipation { enabled } => {
                let mut state = self.inner.state.write().await;
                state.participation_enabled = enabled;
                if let Err(error) = persist_state(&self.inner.state_dir, &state) {
                    return DaemonResponse::error(
                        request.request_id,
                        "state_persistence_failed",
                        error.to_string(),
                    );
                }
                drop(state);
                DaemonResponse::success(
                    request.request_id,
                    DaemonResult::Status(self.status().await),
                )
            }
            DaemonCommand::StartNetwork { input } => match self.start_network(input).await {
                Ok(result) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Network(result))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "network_start_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::StopNetwork => match self.stop_network().await {
                Ok(result) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Network(result))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "network_stop_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::CreateJob { input } => match self.create_job(input).await {
                Ok(job) => DaemonResponse::success(request.request_id, DaemonResult::Job(job)),
                Err(error) => {
                    DaemonResponse::error(request.request_id, "invalid_job", error.to_string())
                }
            },
            DaemonCommand::SubmitJob { job_id } => match self.submit_job(&job_id).await {
                Ok(job) => DaemonResponse::success(request.request_id, DaemonResult::Job(job)),
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "job_submit_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::PublishJob { job_id } => match self.publish_job(&job_id).await {
                Ok(job) => DaemonResponse::success(request.request_id, DaemonResult::Job(job)),
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "job_publish_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::MatchProvider { job_id } => match self.match_provider(&job_id).await {
                Ok(workspace) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Workspace(workspace))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "provider_matching_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::RefreshProtocolJobs => match self.refresh_protocol_jobs().await {
                Ok(workspace) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Workspace(workspace))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "protocol_refresh_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::IngestEvidence { input } => match self.ingest_evidence(input).await {
                Ok(workspace) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Workspace(workspace))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "evidence_ingestion_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::ImportVerificationReceipt { input } => {
                match self.import_verification_receipt(input).await {
                    Ok(workspace) => DaemonResponse::success(
                        request.request_id,
                        DaemonResult::Workspace(workspace),
                    ),
                    Err(error) => DaemonResponse::error(
                        request.request_id,
                        "verification_receipt_import_failed",
                        error.to_string(),
                    ),
                }
            }
            DaemonCommand::SubmitInference { input } => match self.submit_inference(input).await {
                Ok(result) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Inference(result))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "inference_submit_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::ConfigureWallet { input } => match self.configure_wallet(input).await {
                Ok(wallet) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Wallet(wallet))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "wallet_configuration_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::RefreshWallet => match self.refresh_wallet().await {
                Ok(wallet) => {
                    DaemonResponse::success(request.request_id, DaemonResult::Wallet(wallet))
                }
                Err(error) => DaemonResponse::error(
                    request.request_id,
                    "wallet_refresh_failed",
                    error.to_string(),
                ),
            },
            DaemonCommand::PrepareWithdrawal { input } => {
                match self.prepare_withdrawal(input).await {
                    Ok(transfer) => DaemonResponse::success(
                        request.request_id,
                        DaemonResult::Withdrawal(transfer),
                    ),
                    Err(error) => DaemonResponse::error(
                        request.request_id,
                        "withdrawal_preparation_failed",
                        error.to_string(),
                    ),
                }
            }
        }
    }

    pub async fn status(&self) -> DaemonStatus {
        let state = self.inner.state.read().await;
        let network = self.inner.network.lock().await;
        let network_running = network
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        let network_state = if network_running {
            NetworkState::Online
        } else if network.last_error.is_some() {
            NetworkState::Degraded
        } else {
            NetworkState::NotConfigured
        };
        let readiness = self.load_readiness_summary().await.ok().flatten();
        DaemonStatus {
            api_version: API_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.inner.started_at.elapsed().as_secs(),
            participation_enabled: state.participation_enabled,
            network_state,
            network_listen_addr: network.listen_addr.clone(),
            network_bootstrap_peers: network.bootstrap_peers.clone(),
            network_error: network.last_error.clone(),
            active_jobs: state
                .jobs
                .iter()
                .filter(|job| {
                    matches!(
                        job.state,
                        DesktopJobState::Queued
                            | DesktopJobState::Matching
                            | DesktopJobState::Running
                            | DesktopJobState::Verifying
                    )
                })
                .count() as u32,
            platform: PlatformSummary {
                operating_system: env::consts::OS.to_string(),
                architecture: env::consts::ARCH.to_string(),
            },
            readiness,
        }
    }

    async fn load_readiness_summary(&self) -> Result<Option<ReadinessSummary>> {
        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        let peer_presences = protocol_store
            .list_peer_presences()
            .await
            .context("list peer presences for readiness")?;
        let stored_capabilities = protocol_store
            .list_provider_capabilities()
            .await
            .context("list provider capabilities for readiness")?;
        if peer_presences.is_empty() && stored_capabilities.is_empty() {
            return Ok(None);
        }

        let mut capabilities = Vec::with_capacity(stored_capabilities.len());
        for capability in stored_capabilities {
            if let Some(full) = protocol_store
                .load_provider_capability(&capability.node_id)
                .await
                .with_context(|| {
                    format!("load provider capability {} for readiness", capability.node_id)
                })?
            {
                capabilities.push(full);
            }
        }

        let report = build_inference_readiness_report(
            DESKTOP_INFERENCE_PROFILE_ID,
            &peer_presences,
            &capabilities,
        );
        Ok(Some(ReadinessSummary {
            provider_target: report.provider_target,
            healthy_providers: report.healthy_providers,
            provider_gap: report.provider_gap,
            slot_target: report.slot_target,
            available_slots: report.available_slots,
            slot_gap: report.slot_gap,
            verifier_target: report.verifier_target,
            online_verifiers: report.online_verifiers,
            verifier_gap: report.verifier_gap,
        }))
    }

    async fn workspace(&self) -> WorkspaceSnapshot {
        let state = self.inner.state.read().await;
        WorkspaceSnapshot {
            jobs: state.jobs.clone(),
            wallet: state
                .wallet_status
                .clone()
                .unwrap_or_else(|| wallet_status(None, &state.jobs)),
            protocol_announcement_count: state
                .jobs
                .iter()
                .filter(|job| job.state != DesktopJobState::Draft)
                .count() as u32,
        }
    }

    async fn create_job(&self, input: CreateJobInput) -> Result<DesktopJob> {
        validate_job_input(&input)?;
        let timestamp = Utc::now().to_rfc3339();
        let job = DesktopJob {
            job_id: uuid::Uuid::now_v7().to_string(),
            kind: input.kind,
            title: input.title.trim().to_string(),
            model_id: input.model_id.trim().to_string(),
            workload: input.workload.trim().to_string(),
            privacy_mode: input.privacy_mode,
            hardware_profile: input.hardware_profile.trim().to_string(),
            required_verifier_count: input.required_verifier_count,
            challenge_window_seconds: input.challenge_window_seconds,
            budget_usdc_micros: input.budget_usdc_micros,
            state: DesktopJobState::Draft,
            progress_percent: 0,
            provider_node_id: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            evidence: JobEvidenceSummary {
                execution_receipt_sha256: None,
                verification_status: None,
                verifier_count: 0,
                bundle_sha256: None,
                chain_tx_hash: None,
            },
        };
        let mut state = self.inner.state.write().await;
        state.jobs.insert(0, job.clone());
        update_wallet_commitment(&mut state);
        persist_state(&self.inner.state_dir, &state)?;
        Ok(job)
    }

    async fn submit_job(&self, job_id: &str) -> Result<DesktopJob> {
        let mut state = self.inner.state.write().await;
        let job = state
            .jobs
            .iter_mut()
            .find(|job| job.job_id == job_id)
            .ok_or_else(|| anyhow!("job {job_id} was not found"))?;
        if job.state != DesktopJobState::Draft {
            bail!("only draft jobs can enter funding review");
        }
        job.state = DesktopJobState::AwaitingFunding;
        job.updated_at = Utc::now().to_rfc3339();
        let job = job.clone();
        update_wallet_commitment(&mut state);
        persist_state(&self.inner.state_dir, &state)?;
        Ok(job)
    }

    async fn publish_job(&self, job_id: &str) -> Result<DesktopJob> {
        let job = {
            let state = self.inner.state.read().await;
            state
                .jobs
                .iter()
                .find(|job| job.job_id == job_id)
                .cloned()
                .ok_or_else(|| anyhow!("job {job_id} was not found"))?
        };
        if job.state != DesktopJobState::AwaitingFunding {
            bail!("only jobs in funding review can be published");
        }

        let announcement = desktop_job_to_announcement(&job, &self.inner.protocol_identity)?;
        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        protocol_store
            .record_job_announcement(&announcement)
            .await
            .context("record desktop job announcement")?;
        let mut state = self.inner.state.write().await;
        let job = state
            .jobs
            .iter_mut()
            .find(|candidate| candidate.job_id == job_id)
            .ok_or_else(|| anyhow!("job {job_id} was not found"))?;
        job.state = DesktopJobState::Queued;
        job.provider_node_id = None;
        job.progress_percent = 25;
        job.evidence.chain_tx_hash = Some("local_protocol_pending".to_string());
        job.updated_at = Utc::now().to_rfc3339();
        let job = job.clone();
        persist_state(&self.inner.state_dir, &state)?;
        Ok(job)
    }

    async fn match_provider(&self, job_id: &str) -> Result<WorkspaceSnapshot> {
        let parsed_job_id =
            uuid::Uuid::parse_str(job_id).context("desktop job ID is not a UUID")?;
        {
            let state = self.inner.state.read().await;
            let job = state
                .jobs
                .iter()
                .find(|candidate| candidate.job_id == job_id)
                .ok_or_else(|| anyhow!("job {job_id} was not found"))?;
            if !matches!(
                job.state,
                DesktopJobState::Queued | DesktopJobState::Matching
            ) {
                bail!("only queued or matching jobs can be matched");
            }
        }

        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        let announcement = protocol_store
            .load_job_announcement(job_id)
            .await
            .context("load protocol job announcement")?
            .ok_or_else(|| anyhow!("cannot match unpublished job {job_id}"))?;
        validate_job_announcement(&announcement)?;
        if let Some(existing) = protocol_store
            .load_job_assignment(job_id)
            .await
            .context("load existing job assignment")?
        {
            if existing.job_id != parsed_job_id {
                bail!("stored assignment job_id does not match requested job");
            }
            return self.refresh_protocol_jobs().await;
        }

        let claims = protocol_store
            .load_job_claims_by_job(job_id)
            .await
            .context("load provider claims")?;
        let selected = select_provider_match(&announcement, &claims, &protocol_store).await?;
        let mut assignment = JobAssignment {
            job_id: parsed_job_id,
            assigned_provider_node_id: selected.provider_node_id,
            assigner_node_id: "desktop-workspace".to_string(),
            assigner_ed25519_public_key_base64: verifying_key_to_base64(
                &self.inner.protocol_identity.verifying_key(),
            ),
            assignment_reason: "desktop_auto_match".to_string(),
            assigned_at: Utc::now().to_rfc3339(),
            signature: String::new(),
        };
        assignment.signature = sign_job_assignment(&assignment, &self.inner.protocol_identity)?;
        protocol_store
            .record_job_assignment(&assignment)
            .await
            .context("record job assignment")?;
        self.refresh_protocol_jobs().await
    }

    async fn refresh_protocol_jobs(&self) -> Result<WorkspaceSnapshot> {
        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        let mut state = self.inner.state.write().await;
        for job in &mut state.jobs {
            let Ok(job_id) = uuid::Uuid::parse_str(&job.job_id) else {
                continue;
            };
            let assignment = protocol_store
                .load_job_assignment(&job.job_id)
                .await
                .context("load protocol job assignment")?;
            if let Some(assignment) = assignment {
                job.provider_node_id = Some(assignment.assigned_provider_node_id);
                if matches!(
                    job.state,
                    DesktopJobState::Queued | DesktopJobState::Matching
                ) {
                    job.state = DesktopJobState::Running;
                    job.progress_percent = job.progress_percent.max(50);
                }
            } else if job.state == DesktopJobState::Queued {
                job.state = DesktopJobState::Matching;
                job.progress_percent = job.progress_percent.max(35);
            }

            if let Some(availability) = protocol_store
                .load_receipt_availability_by_job(&job.job_id)
                .await
                .context("load protocol receipt availability")?
                .into_iter()
                .next()
            {
                job.provider_node_id = Some(availability.provider_node_id);
                job.evidence.execution_receipt_sha256 = Some(availability.execution_receipt_sha256);
                job.evidence.bundle_sha256 = Some(availability.bundle_sha256);
                job.state = DesktopJobState::Verifying;
                job.progress_percent = job.progress_percent.max(75);
            }

            if let Some(bundle) = protocol_store
                .load_receipt_bundle(&job.job_id)
                .await
                .context("load protocol receipt bundle")?
            {
                job.evidence.bundle_sha256 = Some(bundle.bundle_sha256);
                job.evidence.execution_receipt_sha256 = Some(bundle.execution_receipt_sha256);
                job.state = DesktopJobState::Verifying;
                job.progress_percent = job.progress_percent.max(80);
            }

            let receipts = protocol_store
                .load_verification_receipts_by_job(&job.job_id)
                .await
                .context("load protocol verification receipts")?;
            let quorum = calculate_quorum_status(job_id, job.required_verifier_count, &receipts);
            job.evidence.verifier_count = quorum.accepted_verifier_count as u8;
            job.evidence.verification_status = Some(format!("{:?}", quorum.status).to_lowercase());
            if quorum.accepted_verifier_count >= usize::from(job.required_verifier_count) {
                job.state = DesktopJobState::Completed;
                job.progress_percent = 100;
            }
            job.updated_at = Utc::now().to_rfc3339();
        }
        update_wallet_commitment(&mut state);
        persist_state(&self.inner.state_dir, &state)?;
        Ok(WorkspaceSnapshot {
            jobs: state.jobs.clone(),
            wallet: state
                .wallet_status
                .clone()
                .unwrap_or_else(|| wallet_status(None, &state.jobs)),
            protocol_announcement_count: state
                .jobs
                .iter()
                .filter(|job| job.state != DesktopJobState::Draft)
                .count() as u32,
        })
    }

    async fn ingest_evidence(&self, input: EvidenceIngestionInput) -> Result<WorkspaceSnapshot> {
        let evidence_dir = PathBuf::from(input.evidence_dir);
        if !evidence_dir.is_dir() {
            bail!(
                "evidence directory {} does not exist",
                evidence_dir.display()
            );
        }
        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        let availability = protocol_store
            .load_receipt_availability_by_job(&input.job_id)
            .await
            .context("load protocol receipt availability")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no receipt availability found for job {}", input.job_id))?;
        verify_availability_signature(&availability)?;
        let validated = validate_fetched_evidence(&evidence_dir, &availability)?;
        if validated.job_spec.job_id.to_string() != input.job_id {
            bail!(
                "evidence job_id {} does not match requested job_id {}",
                validated.job_spec.job_id,
                input.job_id
            );
        }
        protocol_store
            .upsert_job_spec(
                &validated.job_spec,
                &json_enum_label(&validated.execution_receipt.status)?,
                Some(&evidence_dir),
                Some(&validated.execution_receipt.metrics_path),
            )
            .await
            .context("record ingested job spec")?;
        protocol_store
            .record_execution_receipt(
                &validated.execution_receipt,
                &evidence_dir,
                &validated.execution_receipt.metrics_path,
            )
            .await
            .context("record ingested execution receipt")?;
        protocol_store
            .record_receipt_bundle(&validated.bundle)
            .await
            .context("record ingested receipt bundle")?;
        self.refresh_protocol_jobs().await
    }

    async fn import_verification_receipt(
        &self,
        input: VerificationReceiptImportInput,
    ) -> Result<WorkspaceSnapshot> {
        let receipt_path = PathBuf::from(input.receipt_json_path);
        if !receipt_path.is_file() {
            bail!(
                "verification receipt file {} does not exist",
                receipt_path.display()
            );
        }
        let announcement: VerificationReceiptAnnouncement = serde_json::from_slice(
            &fs::read(&receipt_path)
                .with_context(|| format!("failed to read {}", receipt_path.display()))?,
        )
        .context("decode verification receipt announcement")?;
        if announcement.verification_receipt.job_id.to_string() != input.job_id {
            bail!(
                "verification receipt job_id {} does not match requested job_id {}",
                announcement.verification_receipt.job_id,
                input.job_id
            );
        }
        if announcement.verification_receipt.verifier_id != announcement.verifier_node_id {
            bail!(
                "verification receipt verifier_id {} does not match announcement verifier {}",
                announcement.verification_receipt.verifier_id,
                announcement.verifier_node_id
            );
        }
        let verifier_key =
            verifying_key_from_base64(&announcement.verifier_ed25519_public_key_base64)
                .context("decode verifier public key")?;
        verify_verification_receipt_signature(&announcement.verification_receipt, &verifier_key)
            .context("verification receipt signature invalid")?;

        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        protocol_store
            .record_verification_receipt(&announcement.verification_receipt)
            .await
            .context("record verification receipt")?;
        self.refresh_protocol_jobs().await
    }

    async fn start_network(&self, input: NetworkControlInput) -> Result<NetworkControlResult> {
        let listen_addr = if input.listen_addr.trim().is_empty() {
            "/ip4/127.0.0.1/tcp/0".to_string()
        } else {
            input.listen_addr.trim().to_string()
        };
        let bootstrap_peers = input
            .bootstrap_peers
            .into_iter()
            .map(|peer| peer.trim().to_string())
            .filter(|peer| !peer.is_empty())
            .collect::<Vec<_>>();
        let protocol_root = self.inner.state_dir.join("protocol");
        let signing_key_seed_base64 = STANDARD.encode(self.inner.protocol_identity.to_bytes());
        self.ensure_protocol_identity(&bootstrap_peers).await?;
        let peer_id = osciris_node::network::peer_id_from_signing_seed(&signing_key_seed_base64)?;

        let mut network = self.inner.network.lock().await;
        if network
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            let listen_addr = network.listen_addr.clone();
            drop(network);
            return Ok(NetworkControlResult {
                status: self.status().await,
                peer_id: Some(peer_id),
                listen_addr,
            });
        }

        let config = NetworkServeConfig {
            protocol_root,
            signing_key_seed_base64,
            listen_addr: listen_addr.clone(),
            bootstrap_peers: bootstrap_peers.clone(),
            status: NodeStatus::OnlineIdle,
            current_load: 0.0,
            active_job_count: 0,
            presence_interval: Duration::from_secs(5),
            run_for: None,
        };
        let inner = self.inner.clone();
        let handle = tokio::spawn(async move {
            if let Err(error) = serve_presence(&config).await {
                let mut network = inner.network.lock().await;
                network.last_error = Some(error.to_string());
                tracing::warn!("desktop network serve stopped: {error}");
            }
        });

        network.handle = Some(handle);
        network.listen_addr = Some(listen_addr.clone());
        network.bootstrap_peers = bootstrap_peers;
        network.last_error = None;
        drop(network);

        Ok(NetworkControlResult {
            status: self.status().await,
            peer_id: Some(peer_id),
            listen_addr: Some(listen_addr),
        })
    }

    async fn stop_network(&self) -> Result<NetworkControlResult> {
        let mut network = self.inner.network.lock().await;
        if let Some(handle) = network.handle.take() {
            handle.abort();
        }
        network.last_error = None;
        drop(network);
        Ok(NetworkControlResult {
            status: self.status().await,
            peer_id: None,
            listen_addr: None,
        })
    }

    async fn ensure_protocol_identity(&self, bootstrap_peers: &[String]) -> Result<NodeIdentity> {
        let protocol_store = ProtocolStore::open(&self.inner.state_dir.join("protocol"))
            .await
            .context("open daemon protocol store")?;
        if let Some(identity) = protocol_store.load_node_identity().await? {
            return Ok(identity);
        }
        let identity = NodeIdentity {
            node_id: "desktop-workspace".to_string(),
            role: NodeRole::Enterprise,
            ed25519_public_key_base64: verifying_key_to_base64(
                &self.inner.protocol_identity.verifying_key(),
            ),
            evm_address: None,
            display_name: "OSCIRIS Desktop Workspace".to_string(),
            bootstrap_peers: bootstrap_peers.to_vec(),
            created_at: Utc::now().to_rfc3339(),
        };
        protocol_store
            .record_node_identity(&identity)
            .await
            .context("record daemon protocol identity")?;
        Ok(identity)
    }

    async fn submit_inference(&self, input: InferencePromptInput) -> Result<InferencePromptResult> {
        if input.prompt.trim().is_empty() {
            bail!("prompt must not be empty");
        }
        if input.prompt.len() > 16_384 {
            bail!("prompt exceeds the desktop safety limit");
        }
        if input.profile_id.trim().is_empty() {
            bail!("profile_id must not be empty");
        }
        if input.provider_peer_id.trim().is_empty() {
            bail!("provider_peer_id must not be empty");
        }
        let max_output_tokens = input.max_output_tokens.clamp(1, 4096);
        let signing_key_seed_base64 = STANDARD.encode(self.inner.protocol_identity.to_bytes());
        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: signing_key_seed_base64.clone(),
            requester_id: if input.requester_id.trim().is_empty() {
                "desktop-workspace".to_string()
            } else {
                input.requester_id.trim().to_string()
            },
            profile_id: input.profile_id,
            prompt: input.prompt,
            max_output_tokens,
        })?;
        let summary = wait_for_inference_response(&InferenceWaitConfig {
            protocol_root: self.inner.state_dir.join("protocol"),
            signing_key_seed_base64,
            request,
            provider_peer_id: input.provider_peer_id,
            listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
            bootstrap_peers: input.bootstrap_peers,
            timeout: Duration::from_secs(input.timeout_seconds.clamp(1, 600)),
        })
        .await?;
        Ok(InferencePromptResult {
            request_id: summary.request.request_id.to_string(),
            profile_id: summary.response.profile_id,
            provider_node_id: summary.response.provider_node_id,
            response_text: summary.response.response_text,
            request_sha256: summary.response.request_sha256,
            response_sha256: summary.response.response_sha256,
            prompt_tokens: summary.response.prompt_tokens,
            output_tokens: summary.response.output_tokens,
            latency_ms: summary.response.latency_ms,
            evidence_dir: summary.evidence_dir.display().to_string(),
            execution_receipt_sha256: summary.execution_receipt_sha256,
            bundle_sha256: summary.bundle_sha256,
        })
    }

    async fn configure_wallet(&self, input: WalletConfigInput) -> Result<WalletStatus> {
        let address = normalize_nonzero_address(&input.address)?;
        let settlement_token_address = input
            .settlement_token_address
            .as_deref()
            .map(normalize_nonzero_address)
            .transpose()?;
        if input.settlement_token_symbol.trim().is_empty()
            || input.settlement_token_symbol.trim().len() > 12
        {
            bail!("settlement token symbol must contain 1 to 12 characters");
        }
        if input.settlement_token_decimals > 36 {
            bail!("settlement token decimals must not exceed 36");
        }

        let config = WalletConfig {
            address,
            settlement_token_address,
            settlement_token_symbol: input.settlement_token_symbol.trim().to_uppercase(),
            settlement_token_decimals: input.settlement_token_decimals,
        };
        {
            let mut state = self.inner.state.write().await;
            state.wallet = Some(config);
            state.wallet_status = None;
            persist_state(&self.inner.state_dir, &state)?;
        }
        self.refresh_wallet().await
    }

    async fn refresh_wallet(&self) -> Result<WalletStatus> {
        let (config, jobs) = {
            let state = self.inner.state.read().await;
            (
                state
                    .wallet
                    .clone()
                    .ok_or_else(|| anyhow!("configure a watch-only wallet first"))?,
                state.jobs.clone(),
            )
        };
        let mut status = wallet_status(Some(&config), &jobs);
        match self.fetch_wallet_balances(&config).await {
            Ok((native, token)) => {
                status.native_balance_wei = Some(native);
                status.settlement_token = token;
                status.last_synced_at = Some(Utc::now().to_rfc3339());
            }
            Err(error) => {
                status.sync_error = Some(error.to_string());
            }
        }

        let mut state = self.inner.state.write().await;
        state.wallet_status = Some(status.clone());
        persist_state(&self.inner.state_dir, &state)?;
        Ok(status)
    }

    async fn fetch_wallet_balances(
        &self,
        config: &WalletConfig,
    ) -> Result<(String, Option<TokenBalance>)> {
        let chain_hex = self.rpc("eth_chainId", json!([])).await?;
        let chain_id = parse_rpc_u256(&chain_hex)?;
        if chain_id != U256::from(HORIZEN_TESTNET_CHAIN_ID) {
            bail!("RPC returned unexpected chain ID {chain_id}");
        }

        let native_hex = self
            .rpc("eth_getBalance", json!([config.address, "latest"]))
            .await?;
        let native = parse_rpc_u256(&native_hex)?.to_string();
        let token = if let Some(contract) = &config.settlement_token_address {
            let data = encode_balance_of(&config.address)?;
            let result = self
                .rpc(
                    "eth_call",
                    json!([{"to": contract, "data": data}, "latest"]),
                )
                .await?;
            Some(TokenBalance {
                symbol: config.settlement_token_symbol.clone(),
                contract_address: contract.clone(),
                decimals: config.settlement_token_decimals,
                balance_atomic: parse_rpc_u256(&result)?.to_string(),
            })
        } else {
            None
        };
        Ok((native, token))
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<String> {
        let response = self
            .inner
            .http
            .post(HORIZEN_TESTNET_RPC_URL)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params
            }))
            .send()
            .await
            .with_context(|| format!("Horizen RPC {method} request failed"))?
            .error_for_status()
            .with_context(|| format!("Horizen RPC {method} returned an HTTP error"))?;
        let body: Value = response
            .json()
            .await
            .with_context(|| format!("decode Horizen RPC {method} response"))?;
        if let Some(error) = body.get("error") {
            bail!("Horizen RPC {method} failed: {error}");
        }
        body.get("result")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Horizen RPC {method} omitted a string result"))
    }

    async fn prepare_withdrawal(&self, input: WithdrawalInput) -> Result<UnsignedTokenTransfer> {
        let state = self.inner.state.read().await;
        let config = state
            .wallet
            .as_ref()
            .ok_or_else(|| anyhow!("configure a watch-only wallet first"))?;
        let token = config.settlement_token_address.as_ref().ok_or_else(|| {
            anyhow!("configure an OSCIRIS test-token contract before preparing withdrawals")
        })?;
        let recipient = normalize_nonzero_address(&input.recipient)?;
        let amount = U256::from_str(input.amount_atomic.trim())
            .context("withdrawal amount must be an unsigned atomic-unit integer")?;
        if amount.is_zero() {
            bail!("withdrawal amount must be greater than zero");
        }
        Ok(UnsignedTokenTransfer {
            chain_id: HORIZEN_TESTNET_CHAIN_ID,
            from: config.address.clone(),
            to: token.clone(),
            value: "0x0".to_string(),
            data: encode_transfer(&recipient, amount)?,
            amount_atomic: amount.to_string(),
            symbol: config.settlement_token_symbol.clone(),
            signing_instruction:
                "Review and sign this transaction in an external EVM wallet on Horizen testnet."
                    .to_string(),
        })
    }

    pub async fn serve(self, endpoint: LocalEndpoint) -> Result<()> {
        match endpoint {
            #[cfg(unix)]
            LocalEndpoint::Unix(path) => self.serve_unix(path).await,
            #[cfg(windows)]
            LocalEndpoint::NamedPipe(name) => self.serve_named_pipe(name).await,
        }
    }

    #[cfg(unix)]
    async fn serve_unix(self, path: PathBuf) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::net::{UnixListener, UnixStream};

        if let Some(parent) = path.parent() {
            secure_state_dir(parent)?;
        }
        if path.exists() {
            if UnixStream::connect(&path).await.is_ok() {
                bail!("OSCIRIS daemon is already listening at {}", path.display());
            }
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("remove stale socket {}", path.display()))?;
        }

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("bind local socket {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure local socket {}", path.display()))?;
        let _cleanup = UnixSocketCleanup(path);

        loop {
            let (stream, _) = listener.accept().await.context("accept local client")?;
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream, service).await {
                    tracing::warn!(%error, "local daemon client failed");
                }
            });
        }
    }

    #[cfg(windows)]
    async fn serve_named_pipe(self, name: String) -> Result<()> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let mut first = true;
        loop {
            let server = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create(&name)
                .with_context(|| format!("create local named pipe {name}"))?;
            first = false;
            server
                .connect()
                .await
                .with_context(|| format!("accept local named pipe client {name}"))?;
            let service = self.clone();
            tokio::spawn(async move {
                if let Err(error) = handle_connection(server, service).await {
                    tracing::warn!(%error, "local daemon client failed");
                }
            });
        }
    }
}

#[cfg(unix)]
struct UnixSocketCleanup(PathBuf);

#[cfg(unix)]
impl Drop for UnixSocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn handle_connection<T>(stream: T, service: DaemonService) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = Framed::new(stream, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    while let Some(frame) = framed.next().await {
        let line = frame.context("read local daemon frame")?;
        let response = match serde_json::from_str::<DaemonRequest>(&line) {
            Ok(request) => service.handle(request).await,
            Err(error) => DaemonResponse::error(0, "invalid_request", error.to_string()),
        };
        framed
            .send(serde_json::to_string(&response)?)
            .await
            .context("write local daemon response")?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DaemonClient {
    endpoint: LocalEndpoint,
    state_dir: PathBuf,
    timeout: Duration,
}

impl DaemonClient {
    pub fn new(endpoint: LocalEndpoint, state_dir: PathBuf) -> Self {
        Self {
            endpoint,
            state_dir,
            timeout: Duration::from_secs(3),
        }
    }

    pub fn default_for_user() -> Self {
        Self::new(LocalEndpoint::default_for_user(), default_state_dir())
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub async fn status(&self) -> Result<DaemonStatus> {
        match self.send(DaemonCommand::GetStatus).await? {
            DaemonResult::Status(status) => Ok(status),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn set_participation(&self, enabled: bool) -> Result<DaemonStatus> {
        match self
            .send(DaemonCommand::SetParticipation { enabled })
            .await?
        {
            DaemonResult::Status(status) => Ok(status),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn start_network(&self, input: NetworkControlInput) -> Result<NetworkControlResult> {
        match self.send(DaemonCommand::StartNetwork { input }).await? {
            DaemonResult::Network(result) => Ok(result),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn stop_network(&self) -> Result<NetworkControlResult> {
        match self.send(DaemonCommand::StopNetwork).await? {
            DaemonResult::Network(result) => Ok(result),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn workspace(&self) -> Result<WorkspaceSnapshot> {
        match self.send(DaemonCommand::GetWorkspace).await? {
            DaemonResult::Workspace(workspace) => Ok(workspace),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn create_job(&self, input: CreateJobInput) -> Result<DesktopJob> {
        match self.send(DaemonCommand::CreateJob { input }).await? {
            DaemonResult::Job(job) => Ok(job),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn submit_job(&self, job_id: String) -> Result<DesktopJob> {
        match self.send(DaemonCommand::SubmitJob { job_id }).await? {
            DaemonResult::Job(job) => Ok(job),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn publish_job(&self, job_id: String) -> Result<DesktopJob> {
        match self.send(DaemonCommand::PublishJob { job_id }).await? {
            DaemonResult::Job(job) => Ok(job),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn match_provider(&self, job_id: String) -> Result<WorkspaceSnapshot> {
        match self.send(DaemonCommand::MatchProvider { job_id }).await? {
            DaemonResult::Workspace(workspace) => Ok(workspace),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn refresh_protocol_jobs(&self) -> Result<WorkspaceSnapshot> {
        match self.send(DaemonCommand::RefreshProtocolJobs).await? {
            DaemonResult::Workspace(workspace) => Ok(workspace),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn ingest_evidence(
        &self,
        input: EvidenceIngestionInput,
    ) -> Result<WorkspaceSnapshot> {
        match self.send(DaemonCommand::IngestEvidence { input }).await? {
            DaemonResult::Workspace(workspace) => Ok(workspace),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn import_verification_receipt(
        &self,
        input: VerificationReceiptImportInput,
    ) -> Result<WorkspaceSnapshot> {
        match self
            .send(DaemonCommand::ImportVerificationReceipt { input })
            .await?
        {
            DaemonResult::Workspace(workspace) => Ok(workspace),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn submit_inference(
        &self,
        input: InferencePromptInput,
    ) -> Result<InferencePromptResult> {
        match self.send(DaemonCommand::SubmitInference { input }).await? {
            DaemonResult::Inference(result) => Ok(result),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn configure_wallet(&self, input: WalletConfigInput) -> Result<WalletStatus> {
        match self.send(DaemonCommand::ConfigureWallet { input }).await? {
            DaemonResult::Wallet(wallet) => Ok(wallet),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn refresh_wallet(&self) -> Result<WalletStatus> {
        match self.send(DaemonCommand::RefreshWallet).await? {
            DaemonResult::Wallet(wallet) => Ok(wallet),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn prepare_withdrawal(
        &self,
        input: WithdrawalInput,
    ) -> Result<UnsignedTokenTransfer> {
        match self
            .send(DaemonCommand::PrepareWithdrawal { input })
            .await?
        {
            DaemonResult::Withdrawal(transfer) => Ok(transfer),
            result => bail!("unexpected daemon result: {result:?}"),
        }
    }

    pub async fn send(&self, command: DaemonCommand) -> Result<DaemonResult> {
        let auth_token = read_auth_token(&self.state_dir)?;
        let request = DaemonRequest::new(auth_token, command);
        let request_id = request.request_id;
        let response = tokio::time::timeout(self.timeout, self.send_request(request))
            .await
            .map_err(|_| anyhow!("daemon request timed out after {:?}", self.timeout))??;
        if response.request_id != request_id {
            bail!(
                "daemon response ID {} did not match request ID {request_id}",
                response.request_id
            );
        }
        if let Some(error) = response.error {
            bail!("daemon error {}: {}", error.code, error.message);
        }
        response
            .result
            .ok_or_else(|| anyhow!("daemon response did not contain a result"))
    }

    async fn send_request(&self, request: DaemonRequest) -> Result<DaemonResponse> {
        match &self.endpoint {
            #[cfg(unix)]
            LocalEndpoint::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(path)
                    .await
                    .with_context(|| format!("connect to daemon at {}", path.display()))?;
                exchange(stream, request).await
            }
            #[cfg(windows)]
            LocalEndpoint::NamedPipe(name) => {
                let stream = tokio::net::windows::named_pipe::ClientOptions::new()
                    .open(name)
                    .with_context(|| format!("connect to daemon at {name}"))?;
                exchange(stream, request).await
            }
        }
    }
}

async fn exchange<T>(stream: T, request: DaemonRequest) -> Result<DaemonResponse>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = Framed::new(stream, LinesCodec::new_with_max_length(MAX_FRAME_BYTES));
    framed
        .send(serde_json::to_string(&request)?)
        .await
        .context("send daemon request")?;
    let line = framed
        .next()
        .await
        .ok_or_else(|| anyhow!("daemon closed the local connection"))?
        .context("read daemon response")?;
    serde_json::from_str(&line).context("decode daemon response")
}

fn secure_state_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("create daemon state directory {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure daemon state directory {}", path.display()))?;
    }
    Ok(())
}

fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon-state.json")
}

fn auth_token_path(state_dir: &Path) -> PathBuf {
    state_dir.join("daemon-auth-token")
}

fn protocol_identity_path(state_dir: &Path) -> PathBuf {
    state_dir.join("protocol-ed25519-seed")
}

fn ensure_auth_token(state_dir: &Path) -> Result<String> {
    let path = auth_token_path(state_dir);
    if path.exists() {
        return read_auth_token(state_dir);
    }

    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_auth_token(state_dir);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("create daemon credential {}", path.display()));
        }
    };
    file.write_all(token.as_bytes())
        .with_context(|| format!("write daemon credential {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync daemon credential {}", path.display()))?;
    secure_private_file(&path)?;
    Ok(token)
}

fn load_or_create_protocol_identity(state_dir: &Path) -> Result<ed25519_dalek::SigningKey> {
    let path = protocol_identity_path(state_dir);
    if path.exists() {
        secure_private_file(&path)?;
        let seed = std::fs::read_to_string(&path)
            .with_context(|| format!("read daemon protocol identity {}", path.display()))?;
        return load_signing_key_from_base64_seed(seed.trim())
            .context("decode daemon protocol identity");
    }

    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let seed = STANDARD.encode(bytes);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return load_or_create_protocol_identity(state_dir);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("create daemon protocol identity {}", path.display()));
        }
    };
    file.write_all(seed.as_bytes())
        .with_context(|| format!("write daemon protocol identity {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync daemon protocol identity {}", path.display()))?;
    secure_private_file(&path)?;
    load_signing_key_from_base64_seed(&seed).context("decode new daemon protocol identity")
}

fn read_auth_token(state_dir: &Path) -> Result<String> {
    let path = auth_token_path(state_dir);
    secure_private_file(&path)?;
    let token = std::fs::read_to_string(&path)
        .with_context(|| format!("read daemon credential {}", path.display()))?;
    let token = token.trim().to_string();
    if token.len() < 32 {
        bail!("daemon credential at {} is invalid", path.display());
    }
    Ok(token)
}

fn load_state(state_dir: &Path) -> Result<PersistedState> {
    let path = state_path(state_dir);
    if !path.exists() {
        return Ok(PersistedState::default());
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("read daemon state {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("decode daemon state {}", path.display()))
}

fn persist_state(state_dir: &Path, state: &PersistedState) -> Result<()> {
    let path = state_path(state_dir);
    let mut file = AtomicWriteFile::open(&path)
        .with_context(|| format!("open atomic daemon state {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("write daemon state {}", path.display()))?;
    file.commit()
        .with_context(|| format!("commit daemon state {}", path.display()))
}

fn secure_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure private file {}", path.display()))?;
    }
    Ok(())
}

fn desktop_job_to_announcement(
    job: &DesktopJob,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<JobAnnouncement> {
    let job_id = uuid::Uuid::parse_str(&job.job_id).context("desktop job ID is not a UUID")?;
    let job_type = match job.kind {
        DesktopJobKind::Training => JobType::LlmLoraEconomics,
        DesktopJobKind::Inference => JobType::InferenceEconomics,
    };
    let privacy_mode = match job.privacy_mode {
        DesktopPrivacyMode::RawBaseline => PrivacyMode::RawBaseline,
        DesktopPrivacyMode::DspPrepared => PrivacyMode::DspPrepared,
        DesktopPrivacyMode::DpModelRelease => PrivacyMode::DpModelRelease,
    };
    let job_spec = JobSpec {
        job_id,
        job_type: job_type.clone(),
        dataset: Some(job.workload.clone()),
        model_id: Some(job.model_id.clone()),
        command: default_protocol_command(&job_type).to_string(),
        args: vec!["--desktop-workload".to_string(), job.workload.clone()],
        privacy_policy: PrivacyPolicy {
            privacy_mode: privacy_mode.clone(),
            release_object: match job_type {
                JobType::LlmLoraEconomics => "model",
                JobType::InferenceEconomics => "inference_output",
                JobType::ProductionProof => "evidence_bundle",
            }
            .to_string(),
            formal_dp_claim: matches!(job.privacy_mode, DesktopPrivacyMode::DpModelRelease),
            sensitive_field_policy: "desktop_configured_guard".to_string(),
            evidence_profile: "desktop_protocol_submission".to_string(),
        },
        required_verifier_count: job.required_verifier_count,
        challenge_window_seconds: job.challenge_window_seconds,
        payment_token: "USDC_TEST".to_string(),
        escrow_amount_atomic: job.budget_usdc_micros.to_string(),
        created_at: job.created_at.clone(),
    };
    let mut announcement = JobAnnouncement {
        job_id,
        job_spec: job_spec.clone(),
        submitter_node_id: "desktop-workspace".to_string(),
        submitter_ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
        job_type,
        privacy_mode,
        required_capability: desktop_required_capability(&job.hardware_profile),
        estimated_runtime_class: "desktop_requested".to_string(),
        payment_token: job_spec.payment_token.clone(),
        escrow_amount_atomic: job_spec.escrow_amount_atomic.clone(),
        required_verifier_count: job.required_verifier_count,
        announced_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    announcement.signature = sign_job_announcement(&announcement, signing_key)?;
    Ok(announcement)
}

fn default_protocol_command(job_type: &JobType) -> &'static str {
    match job_type {
        JobType::LlmLoraEconomics => "uv run osciris llm-lora-economics",
        JobType::InferenceEconomics => "uv run osciris inference-economics",
        JobType::ProductionProof => "uv run osciris production-proof",
    }
}

fn desktop_required_capability(profile: &str) -> String {
    let normalized = profile.trim().to_ascii_lowercase();
    if normalized.contains("24") {
        "gpu>=24gb".to_string()
    } else if normalized.contains("16") {
        "gpu>=16gb".to_string()
    } else if normalized.is_empty() || normalized == "any" {
        "any".to_string()
    } else {
        normalized
    }
}

#[derive(Debug, Clone)]
struct ProviderMatchCandidate {
    provider_node_id: String,
    current_load: f64,
    active_job_count: u32,
    claimed_at: String,
}

async fn select_provider_match(
    announcement: &JobAnnouncement,
    claims: &[JobClaim],
    store: &ProtocolStore,
) -> Result<ProviderMatchCandidate> {
    let mut candidates = Vec::new();
    for claim in claims {
        if claim.job_id != announcement.job_id {
            continue;
        }
        let Some(capability) = store
            .load_provider_capability(&claim.provider_node_id)
            .await
            .context("load provider capability")?
        else {
            continue;
        };
        if let Some(candidate) =
            validate_provider_match_candidate(announcement, claim, &capability)?
        {
            candidates.push(candidate);
        }
    }
    candidates.sort_by(compare_provider_match_candidates);
    candidates.into_iter().next().with_context(|| {
        format!(
            "no valid provider claims matched job {} and capability {}",
            announcement.job_id, announcement.required_capability
        )
    })
}

fn validate_provider_match_candidate(
    announcement: &JobAnnouncement,
    claim: &JobClaim,
    capability: &ProviderCapability,
) -> Result<Option<ProviderMatchCandidate>> {
    if capability.node_id != claim.provider_node_id {
        return Ok(None);
    }
    if capability.ed25519_public_key_base64 != claim.provider_ed25519_public_key_base64 {
        return Ok(None);
    }
    let provider_key = verifying_key_from_base64(&claim.provider_ed25519_public_key_base64)
        .context("failed to decode provider claim public key")?;
    verify_job_claim_signature(claim, &provider_key).context("provider claim signature invalid")?;
    verify_provider_capability_signature(capability, &provider_key)
        .context("provider capability signature invalid")?;
    if !matches!(
        capability.status,
        NodeStatus::OnlineIdle | NodeStatus::OnlineBusy
    ) {
        return Ok(None);
    }
    if !job_matches_provider_capability(announcement, capability) {
        return Ok(None);
    }
    Ok(Some(ProviderMatchCandidate {
        provider_node_id: claim.provider_node_id.clone(),
        current_load: capability.current_load,
        active_job_count: capability.active_job_count,
        claimed_at: claim.claimed_at.clone(),
    }))
}

fn compare_provider_match_candidates(
    left: &ProviderMatchCandidate,
    right: &ProviderMatchCandidate,
) -> std::cmp::Ordering {
    left.current_load
        .total_cmp(&right.current_load)
        .then_with(|| left.active_job_count.cmp(&right.active_job_count))
        .then_with(|| left.claimed_at.cmp(&right.claimed_at))
        .then_with(|| left.provider_node_id.cmp(&right.provider_node_id))
}

fn validate_job_announcement(announcement: &JobAnnouncement) -> Result<()> {
    let submitter_key =
        verifying_key_from_base64(&announcement.submitter_ed25519_public_key_base64)
            .context("failed to decode job submitter public key")?;
    verify_job_announcement_signature(announcement, &submitter_key)
        .context("job announcement signature invalid")
}

#[derive(Debug, Clone)]
struct ValidatedFetchedEvidence {
    job_spec: JobSpec,
    execution_receipt: ExecutionReceipt,
    bundle: ReceiptBundle,
}

fn validate_fetched_evidence(
    evidence_dir: &Path,
    availability: &ReceiptAvailability,
) -> Result<ValidatedFetchedEvidence> {
    let job_spec_path = evidence_dir.join("job_spec.json");
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
    let job_spec: JobSpec = serde_json::from_slice(
        &fs::read(&job_spec_path)
            .with_context(|| format!("failed to read {}", job_spec_path.display()))?,
    )?;
    let execution_receipt: ExecutionReceipt = serde_json::from_slice(
        &fs::read(&execution_receipt_path)
            .with_context(|| format!("failed to read {}", execution_receipt_path.display()))?,
    )?;
    if job_spec.job_id != availability.job_id {
        bail!(
            "job spec job_id {} does not match availability job_id {}",
            job_spec.job_id,
            availability.job_id
        );
    }
    if job_spec.job_id != execution_receipt.job_id {
        bail!(
            "job spec job_id {} does not match execution receipt job_id {}",
            job_spec.job_id,
            execution_receipt.job_id
        );
    }
    if execution_receipt.provider_id != availability.provider_node_id {
        bail!(
            "fetched execution receipt provider_id {} does not match availability provider {}",
            execution_receipt.provider_id,
            availability.provider_node_id
        );
    }
    let execution_sha256 = sha256_file(&execution_receipt_path)?;
    if execution_sha256 != availability.execution_receipt_sha256 {
        bail!(
            "execution receipt hash mismatch: fetched {} but availability advertised {}",
            execution_sha256,
            availability.execution_receipt_sha256
        );
    }
    let provider_key = verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
    verify_execution_receipt_signature(&execution_receipt, &provider_key)
        .context("fetched execution receipt signature invalid")?;

    let bundle: ReceiptBundle = serde_json::from_slice(
        &fs::read(&receipt_bundle_path)
            .with_context(|| format!("failed to read {}", receipt_bundle_path.display()))?,
    )?;
    if bundle.job_id != availability.job_id {
        bail!(
            "fetched bundle job_id {} does not match availability job_id {}",
            bundle.job_id,
            availability.job_id
        );
    }
    if bundle.execution_receipt_sha256 != availability.execution_receipt_sha256 {
        bail!(
            "bundle execution hash {} does not match availability execution hash {}",
            bundle.execution_receipt_sha256,
            availability.execution_receipt_sha256
        );
    }
    let recomputed_bundle_hash = bundle_hash(&bundle)?;
    if recomputed_bundle_hash != bundle.bundle_sha256 {
        bail!(
            "bundle hash field {} does not match recomputed hash {}",
            bundle.bundle_sha256,
            recomputed_bundle_hash
        );
    }
    if bundle.bundle_sha256 != availability.bundle_sha256 {
        bail!(
            "bundle hash {} does not match availability bundle hash {}",
            bundle.bundle_sha256,
            availability.bundle_sha256
        );
    }

    Ok(ValidatedFetchedEvidence {
        job_spec,
        execution_receipt,
        bundle,
    })
}

fn verify_availability_signature(availability: &ReceiptAvailability) -> Result<()> {
    let verifying_key =
        verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
    verify_receipt_availability_signature(availability, &verifying_key)?;
    Ok(())
}

fn json_enum_label<T: Serialize>(value: &T) -> Result<String> {
    match serde_json::to_value(value)? {
        Value::String(label) => Ok(label),
        _ => bail!("enum did not serialize to a string label"),
    }
}

fn validate_job_input(input: &CreateJobInput) -> Result<()> {
    if input.title.trim().is_empty() || input.title.trim().len() > 96 {
        bail!("job title must contain 1 to 96 characters");
    }
    if input.model_id.trim().is_empty() || input.model_id.trim().len() > 160 {
        bail!("model ID must contain 1 to 160 characters");
    }
    if input.workload.trim().is_empty() || input.workload.trim().len() > 2_000 {
        bail!("workload must contain 1 to 2000 characters");
    }
    if input.hardware_profile.trim().is_empty() || input.hardware_profile.trim().len() > 96 {
        bail!("hardware profile must contain 1 to 96 characters");
    }
    if !(1..=10).contains(&input.required_verifier_count) {
        bail!("required verifier count must be between 1 and 10");
    }
    if !(60..=604_800).contains(&input.challenge_window_seconds) {
        bail!("challenge window must be between 60 seconds and 7 days");
    }
    if input.budget_usdc_micros == 0 {
        bail!("job budget must be greater than zero");
    }
    if input.budget_usdc_micros > 1_000_000_000_000_000_000 {
        bail!("job budget exceeds the desktop safety limit");
    }
    Ok(())
}

fn normalize_address(value: &str) -> Result<String> {
    let address = Address::from_str(value.trim()).context("invalid EVM address")?;
    Ok(format!("{address:#x}"))
}

fn normalize_nonzero_address(value: &str) -> Result<String> {
    let address = Address::from_str(value.trim()).context("invalid EVM address")?;
    if address.is_zero() {
        bail!("EVM address must not be the zero address");
    }
    Ok(format!("{address:#x}"))
}

fn parse_rpc_u256(value: &str) -> Result<U256> {
    let value = value
        .strip_prefix("0x")
        .ok_or_else(|| anyhow!("RPC quantity must use a 0x prefix"))?;
    if value.is_empty() {
        return Ok(U256::ZERO);
    }
    U256::from_str_radix(value, 16).context("invalid RPC quantity")
}

fn encode_balance_of(address: &str) -> Result<String> {
    let address = normalize_address(address)?;
    Ok(format!(
        "0x70a08231{:0>64}",
        address.trim_start_matches("0x")
    ))
}

fn encode_transfer(recipient: &str, amount: U256) -> Result<String> {
    let recipient = normalize_address(recipient)?;
    Ok(format!(
        "0xa9059cbb{:0>64}{amount:064x}",
        recipient.trim_start_matches("0x")
    ))
}

fn wallet_status(config: Option<&WalletConfig>, jobs: &[DesktopJob]) -> WalletStatus {
    WalletStatus {
        configured: config.is_some(),
        network_name: "Horizen Testnet".to_string(),
        chain_id: HORIZEN_TESTNET_CHAIN_ID,
        rpc_url: HORIZEN_TESTNET_RPC_URL.to_string(),
        explorer_url: HORIZEN_TESTNET_EXPLORER_URL.to_string(),
        address: config.map(|config| config.address.clone()),
        native_balance_wei: None,
        settlement_token: None,
        committed_usdc_micros: committed_budget(jobs),
        last_synced_at: None,
        sync_error: None,
        custody_mode: "watch_only_external_signing".to_string(),
    }
}

fn committed_budget(jobs: &[DesktopJob]) -> u64 {
    jobs.iter()
        .filter(|job| job.state != DesktopJobState::Draft)
        .map(|job| job.budget_usdc_micros)
        .fold(0_u64, u64::saturating_add)
}

fn update_wallet_commitment(state: &mut PersistedState) {
    let committed = committed_budget(&state.jobs);
    if let Some(status) = &mut state.wallet_status {
        status.committed_usdc_micros = committed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        canonical_json_sha256, sign_execution_receipt, sign_job_claim, sign_provider_capability,
        sign_receipt_availability, sign_verification_receipt, verify_job_assignment_signature,
        ArtifactManifest, ChainSubmissionStatus, ExecutionStatus, GpuMetadata, VerificationChecks,
        VerificationReceipt, VerificationStatus, SHA256_ALGORITHM,
    };

    fn valid_job_input() -> CreateJobInput {
        CreateJobInput {
            kind: DesktopJobKind::Inference,
            title: "Qwen developer inference".to_string(),
            model_id: "Qwen/Qwen3-4B".to_string(),
            workload: "Return one bounded completion".to_string(),
            privacy_mode: DesktopPrivacyMode::DspPrepared,
            hardware_profile: "gpu-24gb".to_string(),
            required_verifier_count: 2,
            challenge_window_seconds: 3_600,
            budget_usdc_micros: 5_000_000,
        }
    }

    fn test_signing_key(byte: u8) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[byte; 32])
    }

    fn signed_provider_capability(
        provider_id: &str,
        signing_key: &ed25519_dalek::SigningKey,
        current_load: f64,
        active_job_count: u32,
    ) -> ProviderCapability {
        let mut capability = ProviderCapability {
            node_id: provider_id.to_string(),
            ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
            host_class: "test-gpu-host".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            cuda_available: true,
            mps_available: false,
            supported_job_types: vec![JobType::InferenceEconomics, JobType::LlmLoraEconomics],
            supported_runtimes: vec!["python".to_string()],
            pricing_hint: Some("test".to_string()),
            current_load,
            active_job_count,
            status: NodeStatus::OnlineIdle,
            updated_at: Utc::now().to_rfc3339(),
            signature: String::new(),
        };
        capability.signature = sign_provider_capability(&capability, signing_key).unwrap();
        capability
    }

    fn signed_job_claim(
        provider_id: &str,
        signing_key: &ed25519_dalek::SigningKey,
        job_id: uuid::Uuid,
        claimed_at: &str,
    ) -> JobClaim {
        let mut claim = JobClaim {
            job_id,
            provider_node_id: provider_id.to_string(),
            provider_ed25519_public_key_base64: verifying_key_to_base64(
                &signing_key.verifying_key(),
            ),
            claimed_at: claimed_at.to_string(),
            claim_note: Some("test claim".to_string()),
            signature: String::new(),
        };
        claim.signature = sign_job_claim(&claim, signing_key).unwrap();
        claim
    }

    fn signed_verification_announcement(
        verifier_id: &str,
        signing_key: &ed25519_dalek::SigningKey,
        job_id: uuid::Uuid,
        receipt_id: uuid::Uuid,
        bundle_sha256: &str,
    ) -> VerificationReceiptAnnouncement {
        let mut receipt = VerificationReceipt {
            verification_receipt_id: uuid::Uuid::now_v7(),
            receipt_id,
            job_id,
            verifier_id: verifier_id.to_string(),
            verification_status: VerificationStatus::Accepted,
            verified_at: Utc::now().to_rfc3339(),
            checks: VerificationChecks {
                manifest_valid: true,
                stdout_hash_valid: true,
                stderr_hash_valid: true,
                artifact_root_valid: true,
                required_metrics_present: true,
                signature_valid: true,
                hardware_claim_valid: true,
            },
            failure_reasons: vec![],
            bundle_sha256: bundle_sha256.to_string(),
            signature: String::new(),
            signing_key_id: format!("{verifier_id}-key"),
        };
        receipt.signature = sign_verification_receipt(&receipt, signing_key).unwrap();
        VerificationReceiptAnnouncement {
            verifier_node_id: verifier_id.to_string(),
            verifier_ed25519_public_key_base64: verifying_key_to_base64(
                &signing_key.verifying_key(),
            ),
            verification_receipt: receipt,
        }
    }

    #[test]
    fn request_serialization_is_versioned_and_tagged() {
        let request = DaemonRequest {
            api_version: API_VERSION,
            request_id: 9,
            auth_token: "test-token-with-sufficient-length".to_string(),
            command: DaemonCommand::SetParticipation { enabled: true },
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["api_version"], API_VERSION);
        assert_eq!(
            value["auth_token"],
            "test-token-with-sufficient-length".to_string()
        );
        assert_eq!(value["command"]["type"], "set_participation");
        assert_eq!(value["command"]["enabled"], true);
        assert_eq!(
            serde_json::from_value::<DaemonRequest>(value).unwrap(),
            request
        );
    }

    #[tokio::test]
    async fn participation_state_is_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION,
                request_id: 3,
                auth_token: service.inner.auth_token.clone(),
                command: DaemonCommand::SetParticipation { enabled: true },
            })
            .await;
        assert!(response.error.is_none());
        drop(service);

        let restarted = DaemonService::new(directory.path().to_path_buf()).unwrap();
        assert!(restarted.status().await.participation_enabled);
    }

    #[tokio::test]
    async fn network_start_records_identity_and_stop_resets_status() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();

        let started = service
            .start_network(NetworkControlInput {
                listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
                bootstrap_peers: vec![],
            })
            .await
            .unwrap();
        assert_eq!(started.status.network_state, NetworkState::Online);
        assert!(started.peer_id.is_some());

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let identity = store.load_node_identity().await.unwrap().unwrap();
        assert_eq!(identity.node_id, "desktop-workspace");
        assert_eq!(identity.role, NodeRole::Enterprise);
        assert_eq!(
            identity.ed25519_public_key_base64,
            verifying_key_to_base64(&service.inner.protocol_identity.verifying_key())
        );

        let stopped = service.stop_network().await.unwrap();
        assert_eq!(stopped.status.network_state, NetworkState::NotConfigured);
    }

    #[tokio::test]
    async fn job_draft_and_funding_state_are_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let job = service.create_job(valid_job_input()).await.unwrap();
        assert_eq!(job.state, DesktopJobState::Draft);
        let submitted = service.submit_job(&job.job_id).await.unwrap();
        assert_eq!(submitted.state, DesktopJobState::AwaitingFunding);
        drop(service);

        let restarted = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let workspace = restarted.workspace().await;
        assert_eq!(workspace.jobs.len(), 1);
        assert_eq!(workspace.jobs[0].state, DesktopJobState::AwaitingFunding);
        assert_eq!(workspace.wallet.committed_usdc_micros, 5_000_000);
    }

    #[tokio::test]
    async fn publish_job_records_protocol_announcement() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let job = service.create_job(valid_job_input()).await.unwrap();
        service.submit_job(&job.job_id).await.unwrap();
        let published = service.publish_job(&job.job_id).await.unwrap();
        assert_eq!(published.state, DesktopJobState::Queued);
        assert_eq!(published.progress_percent, 25);

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let announcement = store
            .load_job_announcement(&job.job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(announcement.job_id.to_string(), job.job_id);
        assert_eq!(announcement.job_type, JobType::InferenceEconomics);
        assert_eq!(announcement.required_capability, "gpu>=24gb");

        let workspace = service.workspace().await;
        assert_eq!(workspace.protocol_announcement_count, 1);
    }

    #[tokio::test]
    async fn submit_inference_rejects_invalid_desktop_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let valid = InferencePromptInput {
            requester_id: "desktop-test".to_string(),
            profile_id: "qwen3-4b-local".to_string(),
            prompt: "Say hello".to_string(),
            max_output_tokens: 128,
            provider_peer_id: "12D3KooWProvider".to_string(),
            bootstrap_peers: vec![],
            timeout_seconds: 5,
        };

        let mut missing_prompt = valid.clone();
        missing_prompt.prompt = "   ".to_string();
        assert!(service
            .submit_inference(missing_prompt)
            .await
            .unwrap_err()
            .to_string()
            .contains("prompt must not be empty"));

        let mut missing_profile = valid.clone();
        missing_profile.profile_id = " ".to_string();
        assert!(service
            .submit_inference(missing_profile)
            .await
            .unwrap_err()
            .to_string()
            .contains("profile_id must not be empty"));

        let mut missing_provider = valid;
        missing_provider.provider_peer_id = " ".to_string();
        assert!(service
            .submit_inference(missing_provider)
            .await
            .unwrap_err()
            .to_string()
            .contains("provider_peer_id must not be empty"));
    }

    #[tokio::test]
    async fn status_reports_readiness_when_protocol_state_exists() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();

        let presence = osciris_core::PeerPresence {
            node_id: "verifier-1".to_string(),
            role: NodeRole::Verifier,
            ed25519_public_key_base64: "verifier-public-key".to_string(),
            evm_address: None,
            listen_addrs: vec!["/ip4/127.0.0.1/tcp/9000".to_string()],
            relay_capable: false,
            protocol_version: "0.1.0".to_string(),
            client_version: "osciris-node/0.1.1".to_string(),
            status: NodeStatus::OnlineIdle,
            current_load: 0.0,
            active_job_count: 0,
            last_seen_at: Utc::now().to_rfc3339(),
            capability_version: Some("interactive-inference-v1".to_string()),
            signature: "signature".to_string(),
        };
        store.record_peer_presence(&presence).await.unwrap();

        let capability = ProviderCapability {
            node_id: "provider-1".to_string(),
            ed25519_public_key_base64: "provider-public-key".to_string(),
            host_class: "interactive-inference".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            cuda_available: true,
            mps_available: false,
            supported_job_types: vec![JobType::InferenceEconomics],
            supported_runtimes: vec!["llama-cpp".to_string()],
            pricing_hint: None,
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: Utc::now().to_rfc3339(),
            signature: "signature".to_string(),
        };
        store.record_provider_capability(&capability).await.unwrap();

        let status = service.status().await;
        let readiness = status.readiness.expect("expected readiness summary");
        assert_eq!(readiness.provider_target, 4);
        assert_eq!(readiness.healthy_providers, 1);
        assert_eq!(readiness.provider_gap, 3);
        assert_eq!(readiness.slot_target, 3);
        assert_eq!(readiness.available_slots, 1);
        assert_eq!(readiness.slot_gap, 2);
        assert_eq!(readiness.verifier_target, 2);
        assert_eq!(readiness.online_verifiers, 1);
        assert_eq!(readiness.verifier_gap, 1);
    }

    #[tokio::test]
    async fn match_provider_records_lowest_load_assignment() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let job = service.create_job(valid_job_input()).await.unwrap();
        service.submit_job(&job.job_id).await.unwrap();
        service.publish_job(&job.job_id).await.unwrap();

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let job_id = uuid::Uuid::parse_str(&job.job_id).unwrap();
        let provider_a_key = test_signing_key(2);
        let provider_b_key = test_signing_key(3);
        let capability_a = signed_provider_capability("provider-a", &provider_a_key, 0.70, 0);
        let capability_b = signed_provider_capability("provider-b", &provider_b_key, 0.10, 2);
        store
            .record_provider_capability(&capability_a)
            .await
            .unwrap();
        store
            .record_provider_capability(&capability_b)
            .await
            .unwrap();
        store
            .record_job_claim(&signed_job_claim(
                "provider-a",
                &provider_a_key,
                job_id,
                "2026-07-01T00:00:01Z",
            ))
            .await
            .unwrap();
        store
            .record_job_claim(&signed_job_claim(
                "provider-b",
                &provider_b_key,
                job_id,
                "2026-07-01T00:00:02Z",
            ))
            .await
            .unwrap();

        let refreshed = service.match_provider(&job.job_id).await.unwrap();
        assert_eq!(refreshed.jobs[0].state, DesktopJobState::Running);
        assert_eq!(
            refreshed.jobs[0].provider_node_id,
            Some("provider-b".to_string())
        );
        let assignment = store
            .load_job_assignment(&job.job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(assignment.assigned_provider_node_id, "provider-b");
        verify_job_assignment_signature(
            &assignment,
            &service.inner.protocol_identity.verifying_key(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn refresh_protocol_jobs_reflects_assignment() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let job = service.create_job(valid_job_input()).await.unwrap();
        service.submit_job(&job.job_id).await.unwrap();
        service.publish_job(&job.job_id).await.unwrap();

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let mut assignment = osciris_core::JobAssignment {
            job_id: uuid::Uuid::parse_str(&job.job_id).unwrap(),
            assigned_provider_node_id: "provider-a".to_string(),
            assigner_node_id: "enterprise-1".to_string(),
            assigner_ed25519_public_key_base64: "test-key".to_string(),
            assignment_reason: "test".to_string(),
            assigned_at: Utc::now().to_rfc3339(),
            signature: "test-signature".to_string(),
        };
        store.record_job_assignment(&assignment).await.unwrap();
        assignment.signature = "different-signature".to_string();

        let refreshed = service.refresh_protocol_jobs().await.unwrap();
        assert_eq!(refreshed.jobs[0].state, DesktopJobState::Running);
        assert_eq!(
            refreshed.jobs[0].provider_node_id,
            Some("provider-a".to_string())
        );
        assert!(refreshed.jobs[0].progress_percent >= 50);
    }

    #[tokio::test]
    async fn ingest_evidence_records_receipt_and_refreshes_job() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let job = service.create_job(valid_job_input()).await.unwrap();
        service.submit_job(&job.job_id).await.unwrap();
        service.publish_job(&job.job_id).await.unwrap();

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let announcement = store
            .load_job_announcement(&job.job_id)
            .await
            .unwrap()
            .unwrap();
        let provider_key = ed25519_dalek::SigningKey::from_bytes(&[9_u8; 32]);
        let provider_public = verifying_key_to_base64(&provider_key.verifying_key());
        let evidence_dir = directory.path().join("provider-evidence");
        fs::create_dir_all(&evidence_dir).unwrap();
        let job_spec_path = evidence_dir.join("job_spec.json");
        let metrics_path = evidence_dir.join("metrics.json");
        let stdout_path = evidence_dir.join("stdout.log");
        let stderr_path = evidence_dir.join("stderr.log");
        let execution_receipt_path = evidence_dir.join("execution_receipt.json");
        let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
        let bundle_index_path = evidence_dir.join("bundle_index.json");
        fs::write(
            &job_spec_path,
            serde_json::to_vec_pretty(&announcement.job_spec).unwrap(),
        )
        .unwrap();
        fs::write(&metrics_path, br#"{"latency_ms": 42}"#).unwrap();
        fs::write(&stdout_path, b"ok\n").unwrap();
        fs::write(&stderr_path, b"").unwrap();

        let artifact_manifest = ArtifactManifest {
            name: "metrics.json".to_string(),
            algorithm: SHA256_ALGORITHM.to_string(),
            chunk_size: 8192,
            chunks: vec![sha256_file(&metrics_path).unwrap()],
            merkle_root: sha256_file(&metrics_path).unwrap(),
            byte_length: fs::metadata(&metrics_path).unwrap().len(),
            path: "metrics.json".to_string(),
        };
        let mut receipt = ExecutionReceipt {
            receipt_id: uuid::Uuid::now_v7(),
            job_id: announcement.job_id,
            provider_id: "provider-a".to_string(),
            job_type: announcement.job_spec.job_type.clone(),
            status: ExecutionStatus::Completed,
            command_exit_code: 0,
            started_at: Utc::now().to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            wall_clock_seconds: 0.1,
            stdout_sha256: sha256_file(&stdout_path).unwrap(),
            stderr_sha256: sha256_file(&stderr_path).unwrap(),
            artifact_root_sha256: canonical_json_sha256(&vec![artifact_manifest.clone()]).unwrap(),
            artifact_manifests: vec![artifact_manifest],
            metrics_path: "metrics.json".to_string(),
            gpu_metadata: GpuMetadata {
                gpu_model: "test-gpu".to_string(),
                driver: "test-driver".to_string(),
                cuda_available: false,
                vram_gb: Some(24.0),
            },
            signature: String::new(),
            signing_key_id: "provider-a-key".to_string(),
        };
        receipt.signature = sign_execution_receipt(&receipt, &provider_key).unwrap();
        fs::write(
            &execution_receipt_path,
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        fs::write(
            &bundle_index_path,
            br#"{"job_id":"00000000-0000-0000-0000-000000000000","artifacts":[],"execution_receipt_path":"execution_receipt.json","verification_receipt_paths":[]}"#,
        )
        .unwrap();
        let mut bundle = ReceiptBundle {
            bundle_id: uuid::Uuid::now_v7(),
            job_id: announcement.job_id,
            job_spec_sha256: sha256_file(&job_spec_path).unwrap(),
            execution_receipt_sha256: sha256_file(&execution_receipt_path).unwrap(),
            verification_receipt_sha256_list: vec![],
            bundle_sha256: String::new(),
            artifact_index_path: "bundle_index.json".to_string(),
            chain_submission_status: ChainSubmissionStatus::Pending,
        };
        bundle.bundle_sha256 = bundle_hash(&bundle).unwrap();
        fs::write(
            &receipt_bundle_path,
            serde_json::to_vec_pretty(&bundle).unwrap(),
        )
        .unwrap();

        let mut availability = ReceiptAvailability {
            job_id: announcement.job_id,
            provider_node_id: "provider-a".to_string(),
            provider_ed25519_public_key_base64: provider_public,
            execution_receipt_sha256: bundle.execution_receipt_sha256.clone(),
            bundle_sha256: bundle.bundle_sha256.clone(),
            bundle_uri: format!("file://{}", evidence_dir.display()),
            announced_at: Utc::now().to_rfc3339(),
            signature: String::new(),
        };
        availability.signature = sign_receipt_availability(&availability, &provider_key).unwrap();
        store
            .record_receipt_availability(&availability)
            .await
            .unwrap();

        let refreshed = service
            .ingest_evidence(EvidenceIngestionInput {
                job_id: job.job_id.clone(),
                evidence_dir: evidence_dir.display().to_string(),
            })
            .await
            .unwrap();
        let refreshed_job = &refreshed.jobs[0];
        assert_eq!(refreshed_job.state, DesktopJobState::Verifying);
        assert_eq!(
            refreshed_job.provider_node_id,
            Some("provider-a".to_string())
        );
        assert_eq!(
            refreshed_job.evidence.execution_receipt_sha256,
            Some(availability.execution_receipt_sha256.clone())
        );
        assert_eq!(
            refreshed_job.evidence.bundle_sha256,
            Some(availability.bundle_sha256.clone())
        );
        assert_eq!(
            store
                .load_receipt_bundle(&job.job_id)
                .await
                .unwrap()
                .unwrap()
                .bundle_sha256,
            availability.bundle_sha256
        );
    }

    #[tokio::test]
    async fn import_verification_receipt_completes_job_after_quorum() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let mut input = valid_job_input();
        input.required_verifier_count = 1;
        let job = service.create_job(input).await.unwrap();
        service.submit_job(&job.job_id).await.unwrap();
        service.publish_job(&job.job_id).await.unwrap();

        let store = ProtocolStore::open(&directory.path().join("protocol"))
            .await
            .unwrap();
        let job_id = uuid::Uuid::parse_str(&job.job_id).unwrap();
        let bundle = ReceiptBundle {
            bundle_id: uuid::Uuid::now_v7(),
            job_id,
            job_spec_sha256: "11".repeat(32),
            execution_receipt_sha256: "22".repeat(32),
            verification_receipt_sha256_list: vec![],
            bundle_sha256: "33".repeat(32),
            artifact_index_path: "bundle_index.json".to_string(),
            chain_submission_status: ChainSubmissionStatus::Pending,
        };
        store.record_receipt_bundle(&bundle).await.unwrap();
        let verifier_key = test_signing_key(8);
        let announcement = signed_verification_announcement(
            "verifier-a",
            &verifier_key,
            job_id,
            uuid::Uuid::now_v7(),
            &bundle.bundle_sha256,
        );
        let receipt_path = directory.path().join("verification_receipt.json");
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&announcement).unwrap(),
        )
        .unwrap();

        let refreshed = service
            .import_verification_receipt(VerificationReceiptImportInput {
                job_id: job.job_id.clone(),
                receipt_json_path: receipt_path.display().to_string(),
            })
            .await
            .unwrap();
        let refreshed_job = &refreshed.jobs[0];
        assert_eq!(refreshed_job.state, DesktopJobState::Completed);
        assert_eq!(refreshed_job.progress_percent, 100);
        assert_eq!(refreshed_job.evidence.verifier_count, 1);
        assert_eq!(
            refreshed_job.evidence.verification_status,
            Some("accepted".to_string())
        );
        assert_eq!(
            store
                .load_verification_receipts_by_job(&job.job_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn wallet_addresses_and_transfer_payloads_are_canonical() {
        let sender = "0x1111111111111111111111111111111111111111";
        let recipient = "0x2222222222222222222222222222222222222222";
        assert_eq!(normalize_address(sender).unwrap(), sender);
        assert!(normalize_address("not-an-address").is_err());
        assert!(normalize_nonzero_address("0x0000000000000000000000000000000000000000").is_err());
        assert_eq!(
            encode_balance_of(sender).unwrap(),
            "0x70a082310000000000000000000000001111111111111111111111111111111111111111"
        );
        assert_eq!(
            encode_transfer(recipient, U256::from(15_u64)).unwrap(),
            "0xa9059cbb0000000000000000000000002222222222222222222222222222222222222222000000000000000000000000000000000000000000000000000000000000000f"
        );
    }

    #[test]
    fn job_validation_rejects_zero_budget_and_invalid_quorum() {
        let mut input = valid_job_input();
        input.budget_usdc_micros = 0;
        assert!(validate_job_input(&input).is_err());
        input.budget_usdc_micros = 1;
        input.required_verifier_count = 0;
        assert!(validate_job_input(&input).is_err());
    }

    #[tokio::test]
    #[ignore = "requires the public Horizen testnet RPC"]
    async fn horizen_testnet_rpc_reports_expected_chain() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let config = WalletConfig {
            address: "0x000000000000000000000000000000000000dead".to_string(),
            settlement_token_address: None,
            settlement_token_symbol: "USDC_TEST".to_string(),
            settlement_token_decimals: 6,
        };
        let (native, token) = service.fetch_wallet_balances(&config).await.unwrap();
        assert!(native.chars().all(|character| character.is_ascii_digit()));
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn unsupported_api_version_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION + 1,
                request_id: 4,
                auth_token: service.inner.auth_token.clone(),
                command: DaemonCommand::Ping,
            })
            .await;
        assert_eq!(
            response.error.unwrap().code,
            "unsupported_api_version".to_string()
        );
    }

    #[tokio::test]
    async fn invalid_local_credential_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let service = DaemonService::new(directory.path().to_path_buf()).unwrap();
        let response = service
            .handle(DaemonRequest {
                api_version: API_VERSION,
                request_id: 5,
                auth_token: "not-the-daemon-token".to_string(),
                command: DaemonCommand::GetStatus,
            })
            .await;
        assert_eq!(
            response.error.unwrap().code,
            "authentication_failed".to_string()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_client_round_trips_status_and_control() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = LocalEndpoint::Unix(directory.path().join("daemon.sock"));
        let state_dir = directory.path().join("state");
        let service = DaemonService::new(state_dir.clone()).unwrap();
        let server = tokio::spawn(service.serve(endpoint.clone()));

        for _ in 0..50 {
            let LocalEndpoint::Unix(path) = &endpoint;
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let client = DaemonClient::new(endpoint, state_dir);
        let initial = client.status().await.unwrap();
        assert!(!initial.participation_enabled);
        let updated = client.set_participation(true).await.unwrap();
        assert!(updated.participation_enabled);
        server.abort();
    }
}
