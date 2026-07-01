use std::{
    env, fmt,
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
    sync::RwLock,
};
use tokio_util::codec::{Framed, LinesCodec};

use osciris_core::{
    load_signing_key_from_base64_seed, sign_job_announcement, verifying_key_to_base64,
    JobAnnouncement, JobSpec, JobType, PrivacyMode, PrivacyPolicy,
};
use osciris_node::store::ProtocolStore;

pub const API_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const HORIZEN_TESTNET_CHAIN_ID: u64 = 2_651_420;
pub const HORIZEN_TESTNET_RPC_URL: &str = "https://horizen-testnet.rpc.caldera.xyz/http";
pub const HORIZEN_TESTNET_EXPLORER_URL: &str = "https://horizen-testnet.explorer.caldera.xyz";

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

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
    SetParticipation { enabled: bool },
    CreateJob { input: CreateJobInput },
    SubmitJob { job_id: String },
    PublishJob { job_id: String },
    ConfigureWallet { input: WalletConfigInput },
    RefreshWallet,
    PrepareWithdrawal { input: WithdrawalInput },
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
    Job(DesktopJob),
    Wallet(WalletStatus),
    Withdrawal(UnsignedTokenTransfer),
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
    http: reqwest::Client,
    protocol_identity: ed25519_dalek::SigningKey,
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
        DaemonStatus {
            api_version: API_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.inner.started_at.elapsed().as_secs(),
            participation_enabled: state.participation_enabled,
            network_state: NetworkState::NotConfigured,
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
            readiness: None,
        }
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
