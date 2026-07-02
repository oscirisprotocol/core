use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, StreamExt};
use libp2p::gossipsub::{
    self, Behaviour as Gossipsub, Event as GossipsubEvent, IdentTopic, MessageAuthenticity,
    PublishError,
};
use libp2p::identify::{Behaviour as Identify, Config as IdentifyConfig, Event as IdentifyEvent};
use libp2p::identity;
use libp2p::multiaddr::Protocol;
use libp2p::ping::{Behaviour as Ping, Config as PingConfig, Event as PingEvent};
use libp2p::request_response::{
    Behaviour as RequestResponse, Codec as RequestResponseCodec, Config as RequestResponseConfig,
    Event as RequestResponseEvent, Message as RequestResponseMessage, OutboundRequestId,
    ProtocolSupport,
};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{Multiaddr, PeerId, StreamProtocol, SwarmBuilder};
use osciris_core::{
    bundle_hash, inference_request_commitment, inference_response_commitment,
    load_signing_key_from_base64_seed, sha256_file, sign_inference_request,
    sign_inference_response, sign_job_announcement, sign_job_claim, sign_peer_presence,
    sign_provider_capability, sign_receipt_availability, sign_execution_receipt,
    verify_inference_request_signature, verify_inference_response_signature, verify_job_announcement_signature,
    verify_job_assignment_signature, verify_job_claim_signature, verify_peer_presence_signature,
    verify_provider_capability_signature, verify_receipt_availability_signature,
    verify_verification_receipt_signature, verifying_key_from_base64, verifying_key_to_base64,
    BundleIndex, ChainSubmissionStatus, CommandMetadata, ExecutionReceipt, ExecutionStatus,
    InferenceRequest, InferenceResponse, JobAnnouncement, JobAssignment, JobClaim, JobSpec,
    JobType, NodeIdentity, NodeStatus, PeerPresence, PrivacyMode, PrivacyPolicy, ProviderCapability,
    ReceiptAvailability, ReceiptBundle, VerificationReceipt, VerificationReceiptAnnouncement,
};
use tar::{Archive, Builder};
use tokio::process::{Child, Command};
use tracing::{info, warn};

use crate::store::ProtocolStore;
use crate::{collect_artifacts, gpu_metadata_from_environment, relative_to, run_job, ProviderConfig};

const PRESENCE_TOPIC: &str = "osciris/network/presence";
const CAPABILITY_TOPIC: &str = "osciris/network/capabilities";
const JOB_ANNOUNCEMENT_TOPIC: &str = "osciris/jobs/announcements";
const JOB_CLAIM_TOPIC: &str = "osciris/jobs/claims";
const JOB_ASSIGNMENT_TOPIC: &str = "osciris/jobs/assignments";
const RECEIPT_AVAILABILITY_TOPIC: &str = "osciris/jobs/receipts";
const VERIFICATION_RECEIPT_TOPIC: &str = "osciris/jobs/verifications";
const BUNDLE_TRANSFER_PROTOCOL: &str = "/osciris/bundle-transfer/0.1.0";
const INFERENCE_PROTOCOL: &str = "/osciris/inference/0.1.0";
const PINNED_INFERENCE_PROFILE_ID: &str = "osciris-qwen3-4b-q4-v1";
const PINNED_INFERENCE_MODEL_REVISION: &str = "bc640142c66e1fdd12af0bd68f40445458f3869b";
const PINNED_INFERENCE_ARTIFACT_NAME: &str = "Qwen3-4B-Q4_K_M.gguf";
const PINNED_INFERENCE_ARTIFACT_SHA256: &str =
    "7485fe6f11af29433bc51cab58009521f205840f5b4ae3a32fa7f92e8534fdf5";
const PINNED_INFERENCE_LICENSE: &str = "Apache-2.0";

#[derive(Debug, Clone)]
pub struct NetworkServeConfig {
    pub protocol_root: std::path::PathBuf,
    pub signing_key_seed_base64: String,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub status: NodeStatus,
    pub current_load: f64,
    pub active_job_count: u32,
    pub presence_interval: Duration,
    pub run_for: Option<Duration>,
}

#[derive(NetworkBehaviour)]
struct OscirisBehaviour {
    gossipsub: Gossipsub,
    bundle_transfer: RequestResponse<BundleTransferCodec>,
    inference: RequestResponse<InferenceCodec>,
    identify: Identify,
    ping: Ping,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkServeSummary {
    pub peer_id: String,
    pub listen_addr: String,
    pub local_node_id: String,
    pub bootstrap_peers: Vec<String>,
    pub heartbeat_count: u32,
    pub received_presence_count: u32,
    pub received_capability_count: u32,
    pub received_job_announcement_count: u32,
    pub received_job_claim_count: u32,
    pub received_job_assignment_count: u32,
    pub received_receipt_availability_count: u32,
    pub received_verification_receipt_count: u32,
    pub connected_peer_count: u32,
    pub active_peer_count: usize,
}

#[derive(Debug, Clone)]
pub struct BundleFetchConfig {
    pub protocol_root: PathBuf,
    pub signing_key_seed_base64: String,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub provider_peer_id: String,
    pub job_id: uuid::Uuid,
    pub provider_node_id: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct AutoVerifierConfig {
    pub protocol_root: PathBuf,
    pub signing_key_seed_base64: String,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub presence_interval: Duration,
    pub run_for: Duration,
}

#[derive(Debug, Clone)]
pub struct AutoProviderConfig {
    pub protocol_root: PathBuf,
    pub signing_key_seed_base64: String,
    pub signing_key_id: String,
    pub repo_root: PathBuf,
    pub work_root: PathBuf,
    pub trusted_assigner_public_keys_base64: Vec<String>,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub presence_interval: Duration,
    pub run_for: Duration,
}

#[derive(Debug, Clone)]
pub struct InferenceServeConfig {
    pub protocol_root: PathBuf,
    pub signing_key_seed_base64: String,
    pub signing_key_id: Option<String>,
    pub provider_id: String,
    pub profile_id: String,
    pub runtime: InferenceRuntimeConfig,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub run_for: Duration,
}

#[derive(Debug, Clone)]
pub enum InferenceRuntimeConfig {
    Deterministic,
    LlamaCppServer { endpoint: String },
    ManagedLlamaCpp {
        llama_server_path: PathBuf,
        model_path: PathBuf,
        host: String,
        port: u16,
        ctx_size: u32,
    },
}

#[derive(Debug, Clone)]
pub struct InferenceSubmitConfig {
    pub signing_key_seed_base64: String,
    pub requester_id: String,
    pub profile_id: String,
    pub prompt: String,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct InferenceWaitConfig {
    pub protocol_root: PathBuf,
    pub signing_key_seed_base64: String,
    pub request: InferenceRequest,
    pub provider_peer_id: String,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InferenceServeSummary {
    pub peer_id: String,
    pub provider_id: String,
    pub served_request_count: u32,
    pub request_ids: Vec<uuid::Uuid>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InferenceSubmitSummary {
    pub request: InferenceRequest,
    pub response: InferenceResponse,
    pub evidence_dir: PathBuf,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct InferenceProfileInstallSummary {
    pub profile_id: String,
    pub model_revision: String,
    pub artifact_name: String,
    pub artifact_sha256: String,
    pub license: String,
    pub installed_model_path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FetchedBundle {
    pub job_id: uuid::Uuid,
    pub provider_node_id: String,
    pub provider_ed25519_public_key_base64: String,
    pub evidence_dir: PathBuf,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AutoVerifierFetchSummary {
    pub peer_id: String,
    pub local_node_id: String,
    pub fetched_bundle_count: u32,
    pub received_presence_count: u32,
    pub received_capability_count: u32,
    pub received_job_announcement_count: u32,
    pub received_receipt_availability_count: u32,
    pub received_verification_receipt_count: u32,
    pub fetched_bundles: Vec<FetchedBundle>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AutoProviderRunSummary {
    pub peer_id: String,
    pub local_node_id: String,
    pub received_job_announcement_count: u32,
    pub received_job_assignment_count: u32,
    pub claimed_job_count: u32,
    pub executed_job_count: u32,
    pub announced_receipt_count: u32,
    pub claimed_job_ids: Vec<uuid::Uuid>,
    pub announced_receipts: Vec<ReceiptAvailability>,
}

struct LocalPresenceContext<'a> {
    identity: &'a NodeIdentity,
    signing_key: &'a ed25519_dalek::SigningKey,
    listen_addr: &'a str,
    status: NodeStatus,
    current_load: f64,
    active_job_count: u32,
}

struct ReplayTopics<'a> {
    capability: &'a IdentTopic,
    job_announcement: &'a IdentTopic,
    job_claim: &'a IdentTopic,
    job_assignment: &'a IdentTopic,
    receipt_availability: &'a IdentTopic,
    verification_receipt: &'a IdentTopic,
}

struct ProviderExecutionContext<'a> {
    config: &'a AutoProviderConfig,
    identity: &'a NodeIdentity,
    signing_key: &'a ed25519_dalek::SigningKey,
    capability: &'a ProviderCapability,
}

#[derive(Debug, Clone)]
struct BundleTransferCodec {
    request_size_maximum: u64,
    response_size_maximum: u64,
}

#[derive(Debug, Clone)]
struct InferenceCodec {
    request_size_maximum: u64,
    response_size_maximum: u64,
}

impl Default for BundleTransferCodec {
    fn default() -> Self {
        Self {
            request_size_maximum: 1024 * 1024,
            response_size_maximum: 128 * 1024 * 1024,
        }
    }
}

impl Default for InferenceCodec {
    fn default() -> Self {
        Self {
            request_size_maximum: 1024 * 1024,
            response_size_maximum: 128 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BundleTransferRequest {
    job_id: uuid::Uuid,
    provider_node_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BundleTransferResponse {
    job_id: uuid::Uuid,
    provider_node_id: String,
    execution_receipt_sha256: String,
    bundle_sha256: String,
    archive_tgz_base64: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InferenceResponseEnvelope {
    response: InferenceResponse,
    provider_capability: ProviderCapability,
    receipt_availability: ReceiptAvailability,
    archive_tgz_base64: String,
}

#[async_trait]
impl RequestResponseCodec for BundleTransferCodec {
    type Protocol = StreamProtocol;
    type Request = BundleTransferRequest;
    type Response = BundleTransferResponse;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(self.request_size_maximum)
            .read_to_end(&mut bytes)
            .await?;
        serde_json::from_slice(&bytes).map_err(invalid_data)
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(self.response_size_maximum)
            .read_to_end(&mut bytes)
            .await?;
        serde_json::from_slice(&bytes).map_err(invalid_data)
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&request).map_err(invalid_data)?;
        io.write_all(&bytes).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&response).map_err(invalid_data)?;
        io.write_all(&bytes).await
    }
}

#[async_trait]
impl RequestResponseCodec for InferenceCodec {
    type Protocol = StreamProtocol;
    type Request = InferenceRequest;
    type Response = InferenceResponseEnvelope;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(self.request_size_maximum)
            .read_to_end(&mut bytes)
            .await?;
        serde_json::from_slice(&bytes).map_err(invalid_data)
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut bytes = Vec::new();
        io.take(self.response_size_maximum)
            .read_to_end(&mut bytes)
            .await?;
        serde_json::from_slice(&bytes).map_err(invalid_data)
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        request: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&request).map_err(invalid_data)?;
        io.write_all(&bytes).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        response: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&response).map_err(invalid_data)?;
        io.write_all(&bytes).await
    }
}

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub fn peer_id_from_signing_seed(seed_base64: &str) -> Result<String> {
    let signing_key = load_signing_key_from_base64_seed(seed_base64)?;
    let keypair = identity::Keypair::ed25519_from_bytes(signing_key.to_bytes())
        .map_err(anyhow::Error::new)?;
    Ok(PeerId::from_public_key(&keypair.public()).to_string())
}

pub fn create_inference_request(config: &InferenceSubmitConfig) -> Result<InferenceRequest> {
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let request_sha256 =
        inference_request_commitment(&config.profile_id, &config.prompt, config.max_output_tokens);
    let mut request = InferenceRequest {
        request_id: uuid::Uuid::now_v7(),
        profile_id: config.profile_id.clone(),
        prompt: config.prompt.clone(),
        max_output_tokens: config.max_output_tokens,
        requester_node_id: config.requester_id.clone(),
        requester_ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
        created_at: chrono::Utc::now().to_rfc3339(),
        request_sha256,
        signature: String::new(),
    };
    request.signature = sign_inference_request(&request, &signing_key)?;
    Ok(request)
}

pub async fn serve_inference(config: &InferenceServeConfig) -> Result<InferenceServeSummary> {
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let store = ProtocolStore::open(&config.protocol_root).await?;
    let _managed_runtime = start_managed_inference_runtime_if_needed(config).await?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    swarm.listen_on(listen_addr)?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        dial_bootstrap(&mut swarm, addr);
    }
    let end_at = tokio::time::Instant::now() + config.run_for;
    let mut served_request_count = 0_u32;
    let mut request_ids = Vec::new();

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(end_at) => break,
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Inference(
                        RequestResponseEvent::Message {
                            peer,
                            message: RequestResponseMessage::Request { request, channel, .. },
                            ..
                        },
                    )) => {
                        let response = build_inference_response_envelope(
                            &store,
                            &request,
                            config,
                            &signing_key,
                        )
                        .await;
                        match response {
                            Ok(response) => {
                                request_ids.push(response.response.request_id);
                                served_request_count += 1;
                                if let Err(_response) = swarm.behaviour_mut().inference.send_response(channel, response) {
                                    warn!("failed to send inference response to {peer}");
                                }
                            }
                            Err(error) => {
                                warn!("failed to build inference response for {peer}: {error}");
                            }
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("inference serve outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(InferenceServeSummary {
        peer_id: swarm.local_peer_id().to_string(),
        provider_id: config.provider_id.clone(),
        served_request_count,
        request_ids,
    })
}

pub async fn wait_for_inference_response(
    config: &InferenceWaitConfig,
) -> Result<InferenceSubmitSummary> {
    let requester_key =
        verifying_key_from_base64(&config.request.requester_ed25519_public_key_base64)?;
    verify_inference_request_signature(&config.request, &requester_key)?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    swarm.listen_on(listen_addr)?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        dial_bootstrap(&mut swarm, addr);
    }
    let provider_peer: PeerId = config.provider_peer_id.parse()?;
    let timeout_at = tokio::time::Instant::now() + config.timeout;
    let mut pending_request_id = None;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(timeout_at) => {
                bail!("inference response timed out after {:?}", config.timeout);
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == provider_peer && pending_request_id.is_none() => {
                        pending_request_id = Some(
                            swarm
                                .behaviour_mut()
                                .inference
                                .send_request(&provider_peer, config.request.clone()),
                        );
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Inference(event)) => {
                        match event {
                            RequestResponseEvent::Message { message, .. } => {
                                if let RequestResponseMessage::Response { request_id, response } = message {
                                    if Some(request_id) == pending_request_id {
                                        let provider_key = verifying_key_from_base64(
                                            &response.response.provider_ed25519_public_key_base64
                                        )?;
                                        verify_inference_response_signature(&response.response, &provider_key)?;
                                        if response.response.request_id != config.request.request_id {
                                            bail!("inference response request_id did not match request");
                                        }
                                        if response.response.request_sha256 != config.request.request_sha256 {
                                            bail!("inference response request commitment did not match request");
                                        }
                                        verify_receipt_availability(&response.receipt_availability)?;
                                        if response.receipt_availability.job_id
                                            != response.response.request_id
                                        {
                                            bail!("receipt availability job_id did not match inference request_id");
                                        }
                                        if response.receipt_availability.provider_node_id
                                            != response.response.provider_node_id
                                        {
                                            bail!("receipt availability provider did not match inference response provider");
                                        }
                                        let fetched = store_inference_response_evidence(
                                            &config.protocol_root,
                                            &response.provider_capability,
                                            &response.receipt_availability,
                                            &response.archive_tgz_base64,
                                        )
                                        .await?;
                                        return Ok(InferenceSubmitSummary {
                                            request: config.request.clone(),
                                            response: response.response,
                                            evidence_dir: fetched.evidence_dir,
                                            execution_receipt_sha256: fetched.execution_receipt_sha256,
                                            bundle_sha256: fetched.bundle_sha256,
                                        });
                                    }
                                }
                            }
                            RequestResponseEvent::OutboundFailure { error, .. } => {
                                bail!("inference request failed: {error}");
                            }
                            RequestResponseEvent::InboundFailure { peer, error, .. } => {
                                warn!("inbound inference failure from {peer}: {error}");
                            }
                            RequestResponseEvent::ResponseSent { .. } => {}
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("inference wait outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    _ => {
                        if pending_request_id.is_none() {
                            for addr in &bootstrap_addrs {
                                dial_bootstrap(&mut swarm, addr);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn pinned_inference_profile_dir(protocol_root: &Path) -> PathBuf {
    protocol_root.join("profiles").join(PINNED_INFERENCE_PROFILE_ID)
}

pub fn pinned_inference_model_path(protocol_root: &Path) -> PathBuf {
    pinned_inference_profile_dir(protocol_root).join(PINNED_INFERENCE_ARTIFACT_NAME)
}

pub fn install_pinned_inference_profile(
    protocol_root: &Path,
    source_model_path: &Path,
) -> Result<InferenceProfileInstallSummary> {
    if !source_model_path.exists() {
        bail!(
            "pinned profile source model does not exist: {}",
            source_model_path.display()
        );
    }
    let actual_sha256 = sha256_file(source_model_path)?;
    if actual_sha256 != PINNED_INFERENCE_ARTIFACT_SHA256 {
        bail!(
            "pinned profile artifact SHA-256 mismatch: expected {}, got {}",
            PINNED_INFERENCE_ARTIFACT_SHA256,
            actual_sha256
        );
    }
    let profile_dir = pinned_inference_profile_dir(protocol_root);
    fs::create_dir_all(&profile_dir)
        .with_context(|| format!("create profile dir {}", profile_dir.display()))?;
    let installed_model_path = pinned_inference_model_path(protocol_root);
    fs::copy(source_model_path, &installed_model_path).with_context(|| {
        format!(
            "copy pinned profile model from {} to {}",
            source_model_path.display(),
            installed_model_path.display()
        )
    })?;
    Ok(InferenceProfileInstallSummary {
        profile_id: PINNED_INFERENCE_PROFILE_ID.to_string(),
        model_revision: PINNED_INFERENCE_MODEL_REVISION.to_string(),
        artifact_name: PINNED_INFERENCE_ARTIFACT_NAME.to_string(),
        artifact_sha256: PINNED_INFERENCE_ARTIFACT_SHA256.to_string(),
        license: PINNED_INFERENCE_LICENSE.to_string(),
        installed_model_path,
    })
}

async fn start_managed_inference_runtime_if_needed(
    config: &InferenceServeConfig,
) -> Result<Option<ManagedInferenceRuntime>> {
    match &config.runtime {
        InferenceRuntimeConfig::ManagedLlamaCpp {
            llama_server_path,
            model_path,
            host,
            port,
            ctx_size,
        } => {
            let child = Command::new(llama_server_path)
                .arg("--model")
                .arg(model_path)
                .arg("--host")
                .arg(host)
                .arg("--port")
                .arg(port.to_string())
                .arg("--ctx-size")
                .arg(ctx_size.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| {
                    format!(
                        "start managed llama-server {} with model {}",
                        llama_server_path.display(),
                        model_path.display()
                    )
                })?;
            let endpoint = format!("http://{}:{}", host, port);
            wait_for_llama_cpp_ready(&endpoint).await?;
            Ok(Some(ManagedInferenceRuntime { child }))
        }
        _ => Ok(None),
    }
}

async fn wait_for_llama_cpp_ready(endpoint: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let health_url = format!("{}/health", endpoint.trim_end_matches('/'));
    for _ in 0..50 {
        match client.get(&health_url).send().await {
            Ok(response) if response.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    bail!("managed llama.cpp runtime did not become ready at {endpoint}");
}

struct ManagedInferenceRuntime {
    child: Child,
}

impl Drop for ManagedInferenceRuntime {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn build_inference_response(
    request: &InferenceRequest,
    config: &InferenceServeConfig,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<InferenceResponse> {
    let started = std::time::Instant::now();
    let requester_key = verifying_key_from_base64(&request.requester_ed25519_public_key_base64)?;
    verify_inference_request_signature(request, &requester_key)?;
    if request.profile_id != config.profile_id {
        bail!(
            "provider profile {} cannot serve request profile {}",
            config.profile_id,
            request.profile_id
        );
    }
    let expected_request_sha256 = inference_request_commitment(
        &request.profile_id,
        &request.prompt,
        request.max_output_tokens,
    );
    if request.request_sha256 != expected_request_sha256 {
        bail!("inference request commitment mismatch");
    }
    let response_text = match &config.runtime {
        InferenceRuntimeConfig::Deterministic => deterministic_inference_response(request),
        InferenceRuntimeConfig::LlamaCppServer { endpoint } => {
            llama_cpp_completion(endpoint, request).await?
        }
        InferenceRuntimeConfig::ManagedLlamaCpp { host, port, .. } => {
            llama_cpp_completion(&format!("http://{}:{}", host, port), request).await?
        }
    };
    let mut response = InferenceResponse {
        request_id: request.request_id,
        profile_id: request.profile_id.clone(),
        provider_node_id: config.provider_id.clone(),
        provider_ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
        response_sha256: inference_response_commitment(&request.request_sha256, &response_text),
        response_text,
        request_sha256: request.request_sha256.clone(),
        prompt_tokens: rough_token_count(&request.prompt),
        output_tokens: 0,
        latency_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        created_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    response.output_tokens = rough_token_count(&response.response_text);
    response.signature = sign_inference_response(&response, signing_key)?;
    Ok(response)
}

async fn build_inference_response_envelope(
    store: &ProtocolStore,
    request: &InferenceRequest,
    config: &InferenceServeConfig,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<InferenceResponseEnvelope> {
    let response = build_inference_response(request, config, signing_key).await?;
    let evidence_dir = write_inference_evidence(store, request, &response, config, signing_key).await?;
    let provider_capability = if let Some(capability) =
        store.load_provider_capability(&config.provider_id).await?
    {
        capability
    } else {
        let mut capability = default_inference_provider_capability(config, &response);
        capability.signature = sign_provider_capability(&capability, signing_key)?;
        store.record_provider_capability(&capability).await?;
        capability
    };
    let identity = NodeIdentity {
        node_id: config.provider_id.clone(),
        role: osciris_core::NodeRole::Provider,
        ed25519_public_key_base64: response.provider_ed25519_public_key_base64.clone(),
        evm_address: None,
        display_name: config.provider_id.clone(),
        bootstrap_peers: config.bootstrap_peers.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let availability = create_receipt_availability_from_evidence(&evidence_dir, &identity, signing_key)?;
    store.record_receipt_bundle(
        &serde_json::from_slice::<ReceiptBundle>(
            &fs::read(evidence_dir.join("receipt_bundle.json"))
                .with_context(|| format!("failed to read {}", evidence_dir.join("receipt_bundle.json").display()))?,
        )?,
    ).await?;
    let archive_bytes = archive_evidence_dir(&evidence_dir)?;
    Ok(InferenceResponseEnvelope {
        response,
        provider_capability,
        receipt_availability: availability,
        archive_tgz_base64: BASE64.encode(archive_bytes),
    })
}

fn default_inference_provider_capability(
    config: &InferenceServeConfig,
    response: &InferenceResponse,
) -> ProviderCapability {
    let runtime = match &config.runtime {
        InferenceRuntimeConfig::Deterministic => "deterministic",
        InferenceRuntimeConfig::LlamaCppServer { .. } => "llama-cpp",
        InferenceRuntimeConfig::ManagedLlamaCpp { .. } => "llama-cpp",
    };
    let mut capability = ProviderCapability {
        node_id: config.provider_id.clone(),
        ed25519_public_key_base64: response.provider_ed25519_public_key_base64.clone(),
        host_class: "interactive-inference".to_string(),
        gpu_model: std::env::var("OSCIRIS_GPU_MODEL").unwrap_or_else(|_| "unknown".to_string()),
        gpu_count: std::env::var("OSCIRIS_GPU_COUNT")
            .ok()
            .and_then(|raw| raw.parse::<u32>().ok())
            .unwrap_or(0),
        vram_gb: std::env::var("OSCIRIS_GPU_VRAM_GB")
            .ok()
            .and_then(|raw| raw.parse::<f64>().ok())
            .unwrap_or(0.0),
        cuda_available: std::env::var("OSCIRIS_CUDA_AVAILABLE")
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false),
        mps_available: std::env::var("OSCIRIS_MPS_AVAILABLE")
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false),
        supported_job_types: vec![JobType::InferenceEconomics],
        supported_runtimes: vec![runtime.to_string()],
        pricing_hint: None,
        current_load: 0.0,
        active_job_count: 1,
        status: NodeStatus::OnlineBusy,
        updated_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    if capability.gpu_count == 0 {
        capability.gpu_model = "none".to_string();
    }
    capability
}

async fn write_inference_evidence(
    store: &ProtocolStore,
    request: &InferenceRequest,
    response: &InferenceResponse,
    config: &InferenceServeConfig,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<PathBuf> {
    let evidence_dir = config
        .protocol_root
        .join("evidence")
        .join(request.request_id.to_string());
    fs::create_dir_all(&evidence_dir)?;
    let metrics_dir = evidence_dir.join("python-output");
    fs::create_dir_all(&metrics_dir)?;

    let job_spec = JobSpec {
        job_id: request.request_id,
        job_type: JobType::InferenceEconomics,
        dataset: Some("interactive_inference_prompt".to_string()),
        model_id: Some(request.profile_id.clone()),
        command: match &config.runtime {
            InferenceRuntimeConfig::Deterministic => "osciris-node inference serve --runtime deterministic".to_string(),
            InferenceRuntimeConfig::LlamaCppServer { endpoint } => format!(
                "osciris-node inference serve --runtime llama-cpp --llama-cpp-endpoint {endpoint}"
            ),
            InferenceRuntimeConfig::ManagedLlamaCpp {
                llama_server_path,
                model_path,
                host,
                port,
                ctx_size,
            } => format!(
                "osciris-node inference serve --runtime llama-cpp-managed --llama-server-path {} --model-path {} --managed-llama-host {} --managed-llama-port {} --managed-llama-ctx-size {}",
                llama_server_path.display(),
                model_path.display(),
                host,
                port,
                ctx_size
            ),
        },
        args: vec![],
        privacy_policy: PrivacyPolicy {
            privacy_mode: PrivacyMode::RawBaseline,
            release_object: "interactive_inference_response".to_string(),
            formal_dp_claim: false,
            sensitive_field_policy: "prompt_and_response_private_to_peer_transport".to_string(),
            evidence_profile: "interactive_inference_transport".to_string(),
        },
        required_verifier_count: 2,
        challenge_window_seconds: 3600,
        payment_token: "USDC_TEST".to_string(),
        escrow_amount_atomic: "0".to_string(),
        created_at: request.created_at.clone(),
    };
    store
        .upsert_job_spec(&job_spec, "completed", Some(&evidence_dir), Some("python-output/inference_economics.json"))
        .await?;

    let job_spec_path = evidence_dir.join("job_spec.json");
    let stdout_path = evidence_dir.join("stdout.log");
    let stderr_path = evidence_dir.join("stderr.log");
    let command_path = evidence_dir.join("command.json");
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
    let bundle_index_path = evidence_dir.join("bundle_index.json");
    let request_path = evidence_dir.join("inference_request.json");
    let response_path = evidence_dir.join("inference_response.json");
    let metrics_path = metrics_dir.join("inference_economics.json");

    fs::write(&job_spec_path, serde_json::to_vec_pretty(&job_spec)?)?;
    fs::write(&request_path, serde_json::to_vec_pretty(request)?)?;
    fs::write(&response_path, serde_json::to_vec_pretty(response)?)?;
    fs::write(
        &stdout_path,
        format!("{}\n", response.response_text),
    )
    ?;
    fs::write(&stderr_path, b"")?;
    let command_metadata = CommandMetadata {
        command: job_spec.command.clone(),
        argv: vec![job_spec.command.clone()],
        working_directory: config.protocol_root.display().to_string(),
        started_at: request.created_at.clone(),
        finished_at: response.created_at.clone(),
        exit_code: 0,
    };
    fs::write(&command_path, serde_json::to_vec_pretty(&command_metadata)?)?;
    fs::write(
        &metrics_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "kind": "inference_economics_benchmark",
            "config": {
                "model": request.profile_id,
                "provider_id": response.provider_node_id,
                "mode": "interactive_peer_inference"
            },
            "aggregate": {
                "latency_ms": response.latency_ms,
                "prompt_tokens": response.prompt_tokens,
                "output_tokens": response.output_tokens
            },
            "runs": [{
                "request_id": request.request_id,
                "request_sha256": request.request_sha256,
                "response_sha256": response.response_sha256,
                "latency_ms": response.latency_ms,
                "prompt_tokens": response.prompt_tokens,
                "output_tokens": response.output_tokens
            }]
        }))?,
    )
    ?;

    let artifact_manifests = collect_artifacts(
        &evidence_dir,
        &[
            job_spec_path.clone(),
            command_path.clone(),
            stdout_path.clone(),
            stderr_path.clone(),
            request_path.clone(),
            response_path.clone(),
        ],
    )?;
    let artifact_root_sha256 = osciris_core::canonical_json_sha256(&artifact_manifests)?;
    let signing_key_id = config
        .signing_key_id
        .clone()
        .unwrap_or_else(|| format!("{}-inference-key", config.provider_id));
    let mut receipt = ExecutionReceipt {
        receipt_id: uuid::Uuid::now_v7(),
        job_id: request.request_id,
        provider_id: config.provider_id.clone(),
        job_type: JobType::InferenceEconomics,
        status: ExecutionStatus::Completed,
        command_exit_code: 0,
        started_at: request.created_at.clone(),
        finished_at: response.created_at.clone(),
        wall_clock_seconds: response.latency_ms as f64 / 1000.0,
        stdout_sha256: sha256_file(&stdout_path)?,
        stderr_sha256: sha256_file(&stderr_path)?,
        artifact_root_sha256,
        artifact_manifests,
        metrics_path: relative_to(&metrics_path, &evidence_dir)?,
        gpu_metadata: gpu_metadata_from_environment(),
        signature: String::new(),
        signing_key_id,
    };
    receipt.signature = sign_execution_receipt(&receipt, signing_key)?;
    fs::write(&execution_receipt_path, serde_json::to_vec_pretty(&receipt)?)?;
    store
        .record_execution_receipt(&receipt, &evidence_dir, &receipt.metrics_path)
        .await?;

    let bundle_index = BundleIndex {
        job_id: request.request_id,
        artifacts: receipt.artifact_manifests.clone(),
        execution_receipt_path: "execution_receipt.json".to_string(),
        verification_receipt_paths: vec![],
    };
    fs::write(&bundle_index_path, serde_json::to_vec_pretty(&bundle_index)?)?;
    let mut bundle = ReceiptBundle {
        bundle_id: uuid::Uuid::now_v7(),
        job_id: request.request_id,
        job_spec_sha256: sha256_file(&job_spec_path)?,
        execution_receipt_sha256: sha256_file(&execution_receipt_path)?,
        verification_receipt_sha256_list: vec![],
        bundle_sha256: String::new(),
        artifact_index_path: "bundle_index.json".to_string(),
        chain_submission_status: ChainSubmissionStatus::Pending,
    };
    bundle.bundle_sha256 = bundle_hash(&bundle)?;
    fs::write(&receipt_bundle_path, serde_json::to_vec_pretty(&bundle)?)?;
    store.record_receipt_bundle(&bundle).await?;
    Ok(evidence_dir)
}

async fn store_inference_response_evidence(
    protocol_root: &Path,
    provider_capability: &ProviderCapability,
    availability: &ReceiptAvailability,
    archive_tgz_base64: &str,
) -> Result<FetchedBundle> {
    let store = ProtocolStore::open(protocol_root).await?;
    verify_and_store_capability(&store, provider_capability.clone()).await?;
    let archive_bytes = BASE64.decode(archive_tgz_base64)?;
    let evidence_dir = protocol_root
        .join("evidence")
        .join(availability.job_id.to_string());
    unpack_evidence_archive(&archive_bytes, &evidence_dir)?;
    let bundle = validate_fetched_evidence(&evidence_dir, availability)?;
    store.record_receipt_bundle(&bundle).await?;
    let execution_receipt: ExecutionReceipt = serde_json::from_slice(
        &fs::read(evidence_dir.join("execution_receipt.json"))
            .with_context(|| format!("failed to read {}", evidence_dir.join("execution_receipt.json").display()))?,
    )?;
    let job_spec: JobSpec = serde_json::from_slice(
        &fs::read(evidence_dir.join("job_spec.json"))
            .with_context(|| format!("failed to read {}", evidence_dir.join("job_spec.json").display()))?,
    )?;
    store
        .upsert_job_spec(
            &job_spec,
            "completed",
            Some(&evidence_dir),
            Some(&execution_receipt.metrics_path),
        )
        .await?;
    store
        .record_execution_receipt(&execution_receipt, &evidence_dir, &execution_receipt.metrics_path)
        .await?;
    Ok(FetchedBundle {
        job_id: availability.job_id,
        provider_node_id: availability.provider_node_id.clone(),
        provider_ed25519_public_key_base64: availability.provider_ed25519_public_key_base64.clone(),
        evidence_dir,
        execution_receipt_sha256: availability.execution_receipt_sha256.clone(),
        bundle_sha256: availability.bundle_sha256.clone(),
    })
}

fn deterministic_inference_response(request: &InferenceRequest) -> String {
    let trimmed = request.prompt.trim();
    let bounded = if trimmed.len() > 240 {
        format!("{}…", &trimmed[..240])
    } else {
        trimmed.to_string()
    };
    format!(
        "[osciris-test-inference:{}] Received {} chars. Prompt: {}",
        request.profile_id,
        request.prompt.chars().count(),
        bounded
    )
}

async fn llama_cpp_completion(endpoint: &str, request: &InferenceRequest) -> Result<String> {
    let endpoint = endpoint.trim_end_matches('/');
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{endpoint}/completion"))
        .json(&serde_json::json!({
            "prompt": request.prompt,
            "n_predict": request.max_output_tokens,
            "stream": false
        }))
        .send()
        .await
        .with_context(|| format!("send inference request to llama.cpp endpoint {endpoint}"))?
        .error_for_status()
        .with_context(|| format!("llama.cpp endpoint {endpoint} returned an error"))?;
    let value: serde_json::Value = response
        .json()
        .await
        .context("decode llama.cpp completion response")?;
    value
        .get("content")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("choices")
                .and_then(serde_json::Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(|choice| choice.get("text"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("llama.cpp response did not contain completion content"))
}

fn rough_token_count(text: &str) -> u32 {
    text.split_whitespace().count().min(u32::MAX as usize) as u32
}

fn build_network_swarm(
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<libp2p::Swarm<OscirisBehaviour>> {
    let keypair = identity::Keypair::ed25519_from_bytes(signing_key.to_bytes())
        .map_err(anyhow::Error::new)?;
    let mut gossipsub_config = gossipsub::ConfigBuilder::default();
    gossipsub_config.heartbeat_interval(Duration::from_secs(2));
    gossipsub_config.flood_publish(true);
    let mut gossipsub = Gossipsub::new(
        MessageAuthenticity::Signed(keypair.clone()),
        gossipsub_config.build()?,
    )
    .map_err(anyhow::Error::msg)?;
    gossipsub.subscribe(&IdentTopic::new(PRESENCE_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(CAPABILITY_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(JOB_ANNOUNCEMENT_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(JOB_CLAIM_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(JOB_ASSIGNMENT_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(RECEIPT_AVAILABILITY_TOPIC))?;
    gossipsub.subscribe(&IdentTopic::new(VERIFICATION_RECEIPT_TOPIC))?;

    let bundle_transfer = RequestResponse::with_codec(
        BundleTransferCodec::default(),
        [(
            StreamProtocol::new(BUNDLE_TRANSFER_PROTOCOL),
            ProtocolSupport::Full,
        )],
        RequestResponseConfig::default().with_request_timeout(Duration::from_secs(30)),
    );
    let inference = RequestResponse::with_codec(
        InferenceCodec::default(),
        [(
            StreamProtocol::new(INFERENCE_PROTOCOL),
            ProtocolSupport::Full,
        )],
        RequestResponseConfig::default().with_request_timeout(Duration::from_secs(180)),
    );
    let behaviour = OscirisBehaviour {
        gossipsub,
        bundle_transfer,
        inference,
        identify: Identify::new(IdentifyConfig::new(
            "/osciris/0.1.0".to_string(),
            keypair.public(),
        )),
        ping: Ping::new(PingConfig::new()),
    };
    Ok(SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            libp2p::tcp::Config::default().nodelay(true),
            libp2p::noise::Config::new,
            libp2p::yamux::Config::default,
        )?
        .with_behaviour(|_| behaviour)?
        .build())
}

pub async fn serve_presence(config: &NetworkServeConfig) -> Result<NetworkServeSummary> {
    let store = ProtocolStore::open(&config.protocol_root).await?;
    let identity = store
        .load_node_identity()
        .await?
        .context("local node identity not found; run `osciris-node node join` first")?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let topic = IdentTopic::new(PRESENCE_TOPIC);
    let capability_topic = IdentTopic::new(CAPABILITY_TOPIC);
    let job_announcement_topic = IdentTopic::new(JOB_ANNOUNCEMENT_TOPIC);
    let job_claim_topic = IdentTopic::new(JOB_CLAIM_TOPIC);
    let job_assignment_topic = IdentTopic::new(JOB_ASSIGNMENT_TOPIC);
    let receipt_availability_topic = IdentTopic::new(RECEIPT_AVAILABILITY_TOPIC);
    let verification_receipt_topic = IdentTopic::new(VERIFICATION_RECEIPT_TOPIC);

    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    let listen_addr_string = listen_addr.to_string();
    swarm.listen_on(listen_addr.clone())?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        dial_bootstrap(&mut swarm, addr);
    }

    let mut interval = tokio::time::interval(config.presence_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let end_at = config
        .run_for
        .map(|duration| tokio::time::Instant::now() + duration);

    let mut heartbeat_count = 0u32;
    let mut received_presence_count = 0u32;
    let mut received_capability_count = 0u32;
    let mut received_job_announcement_count = 0u32;
    let mut received_job_claim_count = 0u32;
    let mut received_job_assignment_count = 0u32;
    let mut received_receipt_availability_count = 0u32;
    let mut received_verification_receipt_count = 0u32;
    let mut connected_peer_count = 0u32;
    let mut connected_peers = BTreeSet::new();
    let local_presence = LocalPresenceContext {
        identity: &identity,
        signing_key: &signing_key,
        listen_addr: &listen_addr_string,
        status: config.status.clone(),
        current_load: config.current_load,
        active_job_count: config.active_job_count,
    };

    publish_presence(
        &mut swarm.behaviour_mut().gossipsub,
        &topic,
        &local_presence,
    )?;
    heartbeat_count += 1;
    replay_local_protocol_state(
        &store,
        &mut swarm.behaviour_mut().gossipsub,
        &ReplayTopics {
            capability: &capability_topic,
            job_announcement: &job_announcement_topic,
            job_claim: &job_claim_topic,
            job_assignment: &job_assignment_topic,
            receipt_availability: &receipt_availability_topic,
            verification_receipt: &verification_receipt_topic,
        },
        &signing_key,
        &identity,
    )
    .await?;

    loop {
        tokio::select! {
            _ = async {
                if let Some(end_at) = end_at {
                    tokio::time::sleep_until(end_at).await;
                } else {
                    futures::future::pending::<()>().await;
                }
            } => {
                break;
            }
            _ = interval.tick() => {
                publish_presence(
                    &mut swarm.behaviour_mut().gossipsub,
                    &topic,
                    &local_presence,
                )?;
                heartbeat_count += 1;
                replay_local_protocol_state(
                    &store,
                    &mut swarm.behaviour_mut().gossipsub,
                    &ReplayTopics {
                        capability: &capability_topic,
                        job_announcement: &job_announcement_topic,
                        job_claim: &job_claim_topic,
                        job_assignment: &job_assignment_topic,
                        receipt_availability: &receipt_availability_topic,
                        verification_receipt: &verification_receipt_topic,
                    },
                    &signing_key,
                    &identity,
                )
                .await?;
                if connected_peers.is_empty() {
                    for addr in &bootstrap_addrs {
                        dial_bootstrap(&mut swarm, addr);
                    }
                }
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("listening on {address}");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        connected_peers.insert(peer_id);
                        connected_peer_count += 1;
                        info!("connection established with {peer_id}");
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        connected_peers.remove(&peer_id);
                        info!("connection closed with {peer_id}");
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Gossipsub(event)) => {
                        match event {
                            GossipsubEvent::Subscribed { peer_id, topic: topic_hash } => {
                                info!("peer {peer_id} subscribed to {topic_hash}");
                                publish_presence(
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &topic,
                                    &local_presence,
                                )?;
                                heartbeat_count += 1;
                                replay_local_protocol_state(
                                    &store,
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &ReplayTopics {
                                        capability: &capability_topic,
                                        job_announcement: &job_announcement_topic,
                                        job_claim: &job_claim_topic,
                                        job_assignment: &job_assignment_topic,
                                        receipt_availability: &receipt_availability_topic,
                                        verification_receipt: &verification_receipt_topic,
                                    },
                                    &signing_key,
                                    &identity,
                                )
                                .await?;
                            }
                            GossipsubEvent::Message { message, .. } => {
                                if let Ok(presence) = serde_json::from_slice::<PeerPresence>(&message.data) {
                                    if presence.node_id != identity.node_id {
                                        match verify_and_store_presence(&store, presence).await {
                                            Ok(()) => {
                                                received_presence_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store peer presence: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(capability) = serde_json::from_slice::<ProviderCapability>(&message.data) {
                                    if capability.node_id != identity.node_id {
                                        match verify_and_store_capability(&store, capability).await {
                                            Ok(()) => {
                                                received_capability_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store provider capability: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(announcement) =
                                    serde_json::from_slice::<JobAnnouncement>(&message.data)
                                {
                                    if announcement.submitter_node_id != identity.node_id {
                                        match verify_and_store_job_announcement(&store, announcement).await {
                                            Ok(()) => {
                                                received_job_announcement_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store job announcement: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(claim) =
                                    serde_json::from_slice::<JobClaim>(&message.data)
                                {
                                    if claim.provider_node_id != identity.node_id {
                                        match verify_and_store_job_claim(&store, claim).await {
                                            Ok(()) => {
                                                received_job_claim_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store job claim: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(assignment) =
                                    serde_json::from_slice::<JobAssignment>(&message.data)
                                {
                                    if assignment.assigner_node_id != identity.node_id {
                                        match verify_and_store_job_assignment(&store, assignment).await {
                                            Ok(()) => {
                                                received_job_assignment_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store job assignment: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(availability) =
                                    serde_json::from_slice::<ReceiptAvailability>(&message.data)
                                {
                                    if availability.provider_node_id != identity.node_id {
                                        match verify_and_store_receipt_availability(&store, availability).await {
                                            Ok(()) => {
                                                received_receipt_availability_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store receipt availability: {error}");
                                            }
                                        }
                                    }
                                } else if let Ok(announcement) =
                                    serde_json::from_slice::<VerificationReceiptAnnouncement>(&message.data)
                                {
                                    if announcement.verifier_node_id != identity.node_id {
                                        match verify_and_store_verification_receipt(&store, announcement).await {
                                            Ok(()) => {
                                                received_verification_receipt_count += 1;
                                            }
                                            Err(error) => {
                                                warn!("failed to verify/store verification receipt: {error}");
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::BundleTransfer(event)) => {
                        match event {
                            RequestResponseEvent::Message { peer, message, .. } => {
                                if let RequestResponseMessage::Request { request, channel, .. } = message {
                                    let response = match build_bundle_transfer_response(
                                        &store,
                                        &config.protocol_root,
                                        &identity,
                                        request,
                                    )
                                    .await
                                    {
                                        Ok(response) => response,
                                        Err(error) => BundleTransferResponse {
                                            job_id: uuid::Uuid::nil(),
                                            provider_node_id: identity.node_id.clone(),
                                            execution_receipt_sha256: String::new(),
                                            bundle_sha256: String::new(),
                                            archive_tgz_base64: None,
                                            error: Some(error.to_string()),
                                        },
                                    };
                                    if let Err(_response) = swarm
                                        .behaviour_mut()
                                        .bundle_transfer
                                        .send_response(channel, response)
                                    {
                                        warn!("failed to send bundle transfer response to {peer}");
                                    }
                                }
                            }
                            RequestResponseEvent::OutboundFailure { peer, error, .. } => {
                                warn!("outbound bundle transfer failure to {peer}: {error}");
                            }
                            RequestResponseEvent::InboundFailure { peer, error, .. } => {
                                warn!("inbound bundle transfer failure from {peer}: {error}");
                            }
                            RequestResponseEvent::ResponseSent { peer, .. } => {
                                info!("sent bundle transfer response to {peer}");
                            }
                        }
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Identify(IdentifyEvent::Received { peer_id, info, .. })) => {
                        info!("identified peer {peer_id} on {:?}", info.listen_addrs);
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Ping(PingEvent { peer, result, .. })) => {
                        if result.is_err() {
                            warn!("ping failed for {peer}: {result:?}");
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(NetworkServeSummary {
        peer_id: swarm.local_peer_id().to_string(),
        listen_addr: listen_addr.to_string(),
        local_node_id: identity.node_id,
        bootstrap_peers: config.bootstrap_peers.clone(),
        heartbeat_count,
        received_presence_count,
        received_capability_count,
        received_job_announcement_count,
        received_job_claim_count,
        received_job_assignment_count,
        received_receipt_availability_count,
        received_verification_receipt_count,
        connected_peer_count,
        active_peer_count: connected_peers.len(),
    })
}

pub async fn run_auto_provider(config: &AutoProviderConfig) -> Result<AutoProviderRunSummary> {
    let store = ProtocolStore::open(&config.protocol_root).await?;
    let identity = store
        .load_node_identity()
        .await?
        .context("local node identity not found; run `osciris-node node join` first")?;
    let capability = store
        .load_provider_capability(&identity.node_id)
        .await?
        .with_context(|| {
            format!(
                "provider capability not found for {}; import a signed capability first",
                identity.node_id
            )
        })?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let trusted_assigners = trusted_assigners(config)?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let topic = IdentTopic::new(PRESENCE_TOPIC);
    let capability_topic = IdentTopic::new(CAPABILITY_TOPIC);
    let job_claim_topic = IdentTopic::new(JOB_CLAIM_TOPIC);
    let receipt_availability_topic = IdentTopic::new(RECEIPT_AVAILABILITY_TOPIC);
    let verification_receipt_topic = IdentTopic::new(VERIFICATION_RECEIPT_TOPIC);

    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    let listen_addr_string = listen_addr.to_string();
    swarm.listen_on(listen_addr.clone())?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        dial_bootstrap(&mut swarm, addr);
    }

    let local_presence = LocalPresenceContext {
        identity: &identity,
        signing_key: &signing_key,
        listen_addr: &listen_addr_string,
        status: NodeStatus::OnlineIdle,
        current_load: capability.current_load,
        active_job_count: capability.active_job_count,
    };
    let provider_execution = ProviderExecutionContext {
        config,
        identity: &identity,
        signing_key: &signing_key,
        capability: &capability,
    };
    let mut received_job_announcement_count = 0u32;
    let mut received_job_assignment_count = 0u32;
    let mut claimed_job_count = 0u32;
    let mut executed_job_count = 0u32;
    let mut announced_receipt_count = 0u32;
    let mut claimed_job_ids = Vec::new();
    let mut announced_receipts = Vec::new();
    let mut connected_peers = BTreeSet::new();

    publish_presence(
        &mut swarm.behaviour_mut().gossipsub,
        &topic,
        &local_presence,
    )?;
    publish_capability(
        &mut swarm.behaviour_mut().gossipsub,
        &capability_topic,
        &capability,
        &signing_key,
        &identity,
    )?;
    replay_local_receipt_availability(
        &store,
        &mut swarm.behaviour_mut().gossipsub,
        &receipt_availability_topic,
        &identity,
        &signing_key,
    )
    .await?;
    let startup_availabilities = execute_and_publish_ready_assignments(
        &store,
        &mut swarm.behaviour_mut().gossipsub,
        &receipt_availability_topic,
        &provider_execution,
    )
    .await?;
    executed_job_count += startup_availabilities.len() as u32;
    announced_receipt_count += startup_availabilities.len() as u32;
    announced_receipts.extend(startup_availabilities);

    let mut interval = tokio::time::interval(config.presence_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let end_at = tokio::time::Instant::now() + config.run_for;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(end_at) => {
                break;
            }
            _ = interval.tick() => {
                publish_presence(
                    &mut swarm.behaviour_mut().gossipsub,
                    &topic,
                    &local_presence,
                )?;
                publish_capability(
                    &mut swarm.behaviour_mut().gossipsub,
                    &capability_topic,
                    &capability,
                    &signing_key,
                    &identity,
                )?;
                replay_local_receipt_availability(
                    &store,
                    &mut swarm.behaviour_mut().gossipsub,
                    &receipt_availability_topic,
                    &identity,
                    &signing_key,
                )
                .await?;
                let ready_availabilities = execute_and_publish_ready_assignments(
                    &store,
                    &mut swarm.behaviour_mut().gossipsub,
                    &receipt_availability_topic,
                    &provider_execution,
                )
                .await?;
                executed_job_count += ready_availabilities.len() as u32;
                announced_receipt_count += ready_availabilities.len() as u32;
                announced_receipts.extend(ready_availabilities);
                if connected_peers.is_empty() {
                    for addr in &bootstrap_addrs {
                        dial_bootstrap(&mut swarm, addr);
                    }
                }
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        info!("auto provider listening on {address}");
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        connected_peers.insert(peer_id);
                        info!("auto provider connected to {peer_id}");
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        connected_peers.remove(&peer_id);
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("auto provider outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Gossipsub(event)) => {
                        match event {
                            GossipsubEvent::Subscribed { .. } => {
                                publish_presence(
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &topic,
                                    &local_presence,
                                )?;
                                publish_capability(
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &capability_topic,
                                    &capability,
                                    &signing_key,
                                    &identity,
                                )?;
                                replay_local_receipt_availability(
                                    &store,
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &receipt_availability_topic,
                                    &identity,
                                    &signing_key,
                                )
                                .await?;
                                let ready_availabilities = execute_and_publish_ready_assignments(
                                    &store,
                                    &mut swarm.behaviour_mut().gossipsub,
                                    &receipt_availability_topic,
                                    &provider_execution,
                                )
                                .await?;
                                executed_job_count += ready_availabilities.len() as u32;
                                announced_receipt_count += ready_availabilities.len() as u32;
                                announced_receipts.extend(ready_availabilities);
                                for receipt in store
                                    .load_verification_receipts_by_verifier(&identity.node_id)
                                    .await?
                                {
                                    publish_verification_receipt(
                                        &mut swarm.behaviour_mut().gossipsub,
                                        &verification_receipt_topic,
                                        &receipt,
                                        &identity,
                                    )?;
                                }
                            }
                            GossipsubEvent::Message { message, .. } => {
                                if let Ok(presence) = serde_json::from_slice::<PeerPresence>(&message.data) {
                                    if presence.node_id != identity.node_id {
                                        if let Err(error) = verify_and_store_presence(&store, presence).await {
                                            warn!("failed to verify/store peer presence: {error}");
                                        }
                                    }
                                } else if let Ok(remote_capability) = serde_json::from_slice::<ProviderCapability>(&message.data) {
                                    if remote_capability.node_id != identity.node_id {
                                        if let Err(error) = verify_and_store_capability(&store, remote_capability).await {
                                            warn!("failed to verify/store provider capability: {error}");
                                        }
                                    }
                                } else if let Ok(announcement) = serde_json::from_slice::<JobAnnouncement>(&message.data) {
                                    if announcement.submitter_node_id == identity.node_id {
                                        continue;
                                    }
                                    match verify_and_store_job_announcement(&store, announcement.clone()).await {
                                        Ok(()) => {
                                            received_job_announcement_count += 1;
                                        }
                                        Err(error) => {
                                            warn!("failed to verify/store job announcement: {error}");
                                            continue;
                                        }
                                    }
                                    if !job_matches_provider_capability(&announcement, &capability) {
                                        continue;
                                    }
                                    if let Some(claim) = claim_job_if_needed(
                                        &store,
                                        &mut swarm.behaviour_mut().gossipsub,
                                        &job_claim_topic,
                                        &announcement,
                                        &signing_key,
                                        &identity,
                                    ).await? {
                                        claimed_job_count += 1;
                                        claimed_job_ids.push(claim.job_id);
                                    }

                                    match execute_assigned_job_if_ready(
                                        &store,
                                        config,
                                        &identity,
                                        &signing_key,
                                        &capability,
                                        &trusted_assigners,
                                        announcement.job_id,
                                    )
                                    .await
                                    {
                                        Ok(Some(availability)) => {
                                            executed_job_count += 1;
                                            publish_receipt_availability(
                                                &mut swarm.behaviour_mut().gossipsub,
                                                &receipt_availability_topic,
                                                &availability,
                                                &signing_key,
                                                &identity,
                                            )?;
                                            announced_receipt_count += 1;
                                            announced_receipts.push(availability);
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            warn!("auto provider failed job {}: {error}", announcement.job_id);
                                        }
                                    }
                                } else if let Ok(claim) = serde_json::from_slice::<JobClaim>(&message.data) {
                                    if claim.provider_node_id != identity.node_id {
                                        if let Err(error) = verify_and_store_job_claim(&store, claim).await {
                                            warn!("failed to verify/store job claim: {error}");
                                        }
                                    }
                                } else if let Ok(assignment) = serde_json::from_slice::<JobAssignment>(&message.data) {
                                    match verify_and_store_job_assignment(&store, assignment.clone()).await {
                                        Ok(()) => {
                                            received_job_assignment_count += 1;
                                        }
                                        Err(error) => {
                                            warn!("failed to verify/store job assignment: {error}");
                                            continue;
                                        }
                                    }
                                    if assignment.assigned_provider_node_id != identity.node_id {
                                        continue;
                                    }
                                    if let Some(announcement) = store
                                        .load_job_announcement(&assignment.job_id.to_string())
                                        .await?
                                    {
                                        if job_matches_provider_capability(&announcement, &capability) {
                                            if let Some(claim) = claim_job_if_needed(
                                                &store,
                                                &mut swarm.behaviour_mut().gossipsub,
                                                &job_claim_topic,
                                                &announcement,
                                                &signing_key,
                                                &identity,
                                            )
                                            .await?
                                            {
                                                claimed_job_count += 1;
                                                claimed_job_ids.push(claim.job_id);
                                            }
                                        }
                                    }
                                    match execute_assigned_job_if_ready(
                                        &store,
                                        config,
                                        &identity,
                                        &signing_key,
                                        &capability,
                                        &trusted_assigners,
                                        assignment.job_id,
                                    )
                                    .await
                                    {
                                        Ok(Some(availability)) => {
                                            executed_job_count += 1;
                                            publish_receipt_availability(
                                                &mut swarm.behaviour_mut().gossipsub,
                                                &receipt_availability_topic,
                                                &availability,
                                                &signing_key,
                                                &identity,
                                            )?;
                                            announced_receipt_count += 1;
                                            announced_receipts.push(availability);
                                        }
                                        Ok(None) => {}
                                        Err(error) => {
                                            warn!("auto provider failed assigned job {}: {error}", assignment.job_id);
                                        }
                                    }
                                } else if let Ok(availability) = serde_json::from_slice::<ReceiptAvailability>(&message.data) {
                                    if availability.provider_node_id != identity.node_id {
                                        if let Err(error) = verify_and_store_receipt_availability(&store, availability).await {
                                            warn!("failed to verify/store receipt availability: {error}");
                                        }
                                    }
                                } else if let Ok(announcement) = serde_json::from_slice::<VerificationReceiptAnnouncement>(&message.data) {
                                    if announcement.verifier_node_id != identity.node_id {
                                        if let Err(error) = verify_and_store_verification_receipt(&store, announcement).await {
                                            warn!("failed to verify/store verification receipt: {error}");
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::BundleTransfer(
                        RequestResponseEvent::Message {
                            peer,
                            message: RequestResponseMessage::Request { request, channel, .. },
                            ..
                        },
                    )) => {
                        let response = match build_bundle_transfer_response(
                            &store,
                            &config.protocol_root,
                            &identity,
                            request,
                        )
                        .await
                        {
                            Ok(response) => response,
                            Err(error) => BundleTransferResponse {
                                job_id: uuid::Uuid::nil(),
                                provider_node_id: identity.node_id.clone(),
                                execution_receipt_sha256: String::new(),
                                bundle_sha256: String::new(),
                                archive_tgz_base64: None,
                                error: Some(error.to_string()),
                            },
                        };
                        if let Err(_response) = swarm
                            .behaviour_mut()
                            .bundle_transfer
                            .send_response(channel, response)
                        {
                            warn!("failed to send bundle transfer response to {peer}");
                        }
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Identify(IdentifyEvent::Received { peer_id, info, .. })) => {
                        info!("auto provider identified peer {peer_id} on {:?}", info.listen_addrs);
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Ping(PingEvent { peer, result, .. })) => {
                        if result.is_err() {
                            warn!("auto provider ping failed for {peer}: {result:?}");
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(AutoProviderRunSummary {
        peer_id: swarm.local_peer_id().to_string(),
        local_node_id: identity.node_id,
        received_job_announcement_count,
        received_job_assignment_count,
        claimed_job_count,
        executed_job_count,
        announced_receipt_count,
        claimed_job_ids,
        announced_receipts,
    })
}

pub async fn fetch_receipt_bundle_p2p(config: &BundleFetchConfig) -> Result<FetchedBundle> {
    let store = ProtocolStore::open(&config.protocol_root).await?;
    let availability = store
        .load_receipt_availability(&config.job_id.to_string(), &config.provider_node_id)
        .await?
        .with_context(|| {
            format!(
                "no receipt availability found for job {} from provider {}",
                config.job_id, config.provider_node_id
            )
        })?;
    verify_receipt_availability(&availability)?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    swarm.listen_on(listen_addr)?;
    let target_peer: PeerId = config.provider_peer_id.parse()?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        if peer_id_from_multiaddr(addr) == Some(target_peer) {
            swarm.add_peer_address(target_peer, addr.clone());
        }
        dial_bootstrap(&mut swarm, addr);
    }

    let end_at = tokio::time::Instant::now() + config.timeout;
    let mut pending_request_id: Option<OutboundRequestId> = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(end_at) => {
                bail!("timed out waiting for bundle transfer response from {target_peer}");
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == target_peer && pending_request_id.is_none() => {
                        pending_request_id = Some(swarm.behaviour_mut().bundle_transfer.send_request(
                            &target_peer,
                            BundleTransferRequest {
                                job_id: availability.job_id,
                                provider_node_id: availability.provider_node_id.clone(),
                            },
                        ));
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::BundleTransfer(event)) => {
                        match event {
                            RequestResponseEvent::Message { message, .. } => {
                                if let RequestResponseMessage::Response { request_id, response } = message {
                                    if Some(request_id) == pending_request_id {
                                        return store_bundle_transfer_response(&store, &config.protocol_root, &availability, response).await;
                                    }
                                }
                            }
                            RequestResponseEvent::OutboundFailure { error, .. } => {
                                bail!("bundle transfer request failed: {error}");
                            }
                            RequestResponseEvent::InboundFailure { peer, error, .. } => {
                                warn!("inbound bundle transfer failure from {peer}: {error}");
                            }
                            RequestResponseEvent::ResponseSent { .. } => {}
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub async fn auto_fetch_receipts(config: &AutoVerifierConfig) -> Result<AutoVerifierFetchSummary> {
    let store = ProtocolStore::open(&config.protocol_root).await?;
    let identity = store
        .load_node_identity()
        .await?
        .context("local node identity not found; run `osciris-node node join` first")?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let mut swarm = build_network_swarm(&signing_key)?;
    let listen_addr: Multiaddr = config.listen_addr.parse()?;
    swarm.listen_on(listen_addr)?;
    let bootstrap_addrs = config
        .bootstrap_peers
        .iter()
        .map(|bootstrap| bootstrap.parse::<Multiaddr>())
        .collect::<Result<Vec<_>, _>>()?;
    for addr in &bootstrap_addrs {
        dial_bootstrap(&mut swarm, addr);
    }

    let end_at = tokio::time::Instant::now() + config.run_for;
    let mut interval = tokio::time::interval(config.presence_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pending: BTreeMap<OutboundRequestId, ReceiptAvailability> = BTreeMap::new();
    let mut requested: BTreeSet<(uuid::Uuid, String)> = BTreeSet::new();
    let mut fetched_bundles = Vec::new();
    let mut received_presence_count = 0u32;
    let mut received_capability_count = 0u32;
    let mut received_job_announcement_count = 0u32;
    let mut received_receipt_availability_count = 0u32;
    let mut received_verification_receipt_count = 0u32;
    queue_stored_receipt_fetches(
        &store,
        &mut swarm,
        &bootstrap_addrs,
        &identity,
        &mut requested,
        &mut pending,
    )
    .await?;

    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(end_at) => {
                break;
            }
            _ = interval.tick() => {
                queue_stored_receipt_fetches(
                    &store,
                    &mut swarm,
                    &bootstrap_addrs,
                    &identity,
                    &mut requested,
                    &mut pending,
                )
                .await?;
                if fetched_bundles.is_empty() {
                    for addr in &bootstrap_addrs {
                        dial_bootstrap(&mut swarm, addr);
                    }
                }
            }
            event = swarm.select_next_some() => {
                match event {
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::Gossipsub(
                        GossipsubEvent::Message { message, .. }
                    )) => {
                        if let Ok(presence) = serde_json::from_slice::<PeerPresence>(&message.data) {
                            if presence.node_id == identity.node_id {
                                continue;
                            }
                            match verify_and_store_presence(&store, presence).await {
                                Ok(()) => {
                                    received_presence_count += 1;
                                }
                                Err(error) => warn!("failed to verify/store peer presence: {error}"),
                            }
                        } else if let Ok(capability) = serde_json::from_slice::<ProviderCapability>(&message.data) {
                            if capability.node_id == identity.node_id {
                                continue;
                            }
                            match verify_and_store_capability(&store, capability).await {
                                Ok(()) => {
                                    received_capability_count += 1;
                                }
                                Err(error) => warn!("failed to verify/store provider capability: {error}"),
                            }
                        } else if let Ok(announcement) = serde_json::from_slice::<JobAnnouncement>(&message.data) {
                            if announcement.submitter_node_id == identity.node_id {
                                continue;
                            }
                            match verify_and_store_job_announcement(&store, announcement).await {
                                Ok(()) => {
                                    received_job_announcement_count += 1;
                                }
                                Err(error) => warn!("failed to verify/store job announcement: {error}"),
                            }
                        } else if let Ok(availability) = serde_json::from_slice::<ReceiptAvailability>(&message.data) {
                            if availability.provider_node_id == identity.node_id {
                                continue;
                            }
                            match verify_and_store_receipt_availability(&store, availability.clone()).await {
                                Ok(()) => {
                                    received_receipt_availability_count += 1;
                                    if let Err(error) = queue_bundle_fetch(
                                        &store,
                                        &mut swarm,
                                        &bootstrap_addrs,
                                        &identity,
                                        availability,
                                        &mut requested,
                                        &mut pending,
                                    )
                                    .await
                                    {
                                        warn!("failed to queue bundle fetch: {error}");
                                    }
                                }
                                Err(error) => {
                                    warn!("failed to verify/store receipt availability: {error}");
                                }
                            }
                        } else if let Ok(announcement) = serde_json::from_slice::<VerificationReceiptAnnouncement>(&message.data) {
                            if announcement.verifier_node_id == identity.node_id {
                                continue;
                            }
                            match verify_and_store_verification_receipt(&store, announcement).await {
                                Ok(()) => {
                                    received_verification_receipt_count += 1;
                                }
                                Err(error) => warn!("failed to verify/store verification receipt: {error}"),
                            }
                        }
                    }
                    SwarmEvent::Behaviour(OscirisBehaviourEvent::BundleTransfer(event)) => {
                        match event {
                            RequestResponseEvent::Message { message, .. } => {
                                if let RequestResponseMessage::Response { request_id, response } = message {
                                    if let Some(availability) = pending.remove(&request_id) {
                                        match store_bundle_transfer_response(&store, &config.protocol_root, &availability, response).await {
                                            Ok(fetched) => fetched_bundles.push(fetched),
                                            Err(error) => warn!("failed to store fetched bundle: {error}"),
                                        }
                                    }
                                }
                            }
                            RequestResponseEvent::OutboundFailure { request_id, error, .. } => {
                                pending.remove(&request_id);
                                warn!("bundle transfer request failed: {error}");
                            }
                            RequestResponseEvent::InboundFailure { peer, error, .. } => {
                                warn!("inbound bundle transfer failure from {peer}: {error}");
                            }
                            RequestResponseEvent::ResponseSent { .. } => {}
                        }
                    }
                    SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                        info!("outgoing connection retry to {:?} did not complete: {error}", peer_id);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(AutoVerifierFetchSummary {
        peer_id: swarm.local_peer_id().to_string(),
        local_node_id: identity.node_id,
        fetched_bundle_count: fetched_bundles.len() as u32,
        received_presence_count,
        received_capability_count,
        received_job_announcement_count,
        received_receipt_availability_count,
        received_verification_receipt_count,
        fetched_bundles,
    })
}

fn dial_bootstrap(swarm: &mut libp2p::Swarm<OscirisBehaviour>, addr: &Multiaddr) {
    if let Err(error) = swarm.dial(addr.clone()) {
        warn!("failed to dial bootstrap {addr}: {error}");
    }
}

fn peer_id_from_multiaddr(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|protocol| {
        if let Protocol::P2p(peer_id) = protocol {
            Some(peer_id)
        } else {
            None
        }
    })
}

fn peer_id_from_ed25519_public_key_base64(public_key_base64: &str) -> Result<PeerId> {
    let public_key_bytes = BASE64.decode(public_key_base64)?;
    let public_key = identity::ed25519::PublicKey::try_from_bytes(&public_key_bytes)
        .map(identity::PublicKey::from)
        .map_err(anyhow::Error::new)?;
    Ok(PeerId::from_public_key(&public_key))
}

async fn replay_local_receipt_availability(
    store: &ProtocolStore,
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    identity: &NodeIdentity,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<u32> {
    let availabilities = store
        .load_receipt_availability_by_provider(&identity.node_id)
        .await?;
    let mut published_count = 0u32;
    for availability in availabilities {
        publish_receipt_availability(gossipsub, topic, &availability, signing_key, identity)?;
        published_count += 1;
    }
    Ok(published_count)
}

async fn replay_local_protocol_state(
    store: &ProtocolStore,
    gossipsub: &mut Gossipsub,
    topics: &ReplayTopics<'_>,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<()> {
    if let Some(capability) = store.load_provider_capability(&identity.node_id).await? {
        publish_capability(
            gossipsub,
            topics.capability,
            &capability,
            signing_key,
            identity,
        )?;
    }

    for announcement in store
        .load_job_announcements_by_submitter(&identity.node_id)
        .await?
    {
        publish_job_announcement(
            gossipsub,
            topics.job_announcement,
            &announcement,
            signing_key,
            identity,
        )?;
    }

    for claim in store.load_job_claims_by_provider(&identity.node_id).await? {
        publish_job_claim(gossipsub, topics.job_claim, &claim, signing_key, identity)?;
    }

    for assignment in store.list_job_assignment_objects().await? {
        publish_job_assignment(gossipsub, topics.job_assignment, &assignment)?;
    }

    for availability in store
        .load_receipt_availability_by_provider(&identity.node_id)
        .await?
    {
        publish_receipt_availability(
            gossipsub,
            topics.receipt_availability,
            &availability,
            signing_key,
            identity,
        )?;
    }

    for receipt in store
        .load_verification_receipts_by_verifier(&identity.node_id)
        .await?
    {
        publish_verification_receipt(gossipsub, topics.verification_receipt, &receipt, identity)?;
    }

    Ok(())
}

async fn queue_stored_receipt_fetches(
    store: &ProtocolStore,
    swarm: &mut libp2p::Swarm<OscirisBehaviour>,
    bootstrap_addrs: &[Multiaddr],
    identity: &NodeIdentity,
    requested: &mut BTreeSet<(uuid::Uuid, String)>,
    pending: &mut BTreeMap<OutboundRequestId, ReceiptAvailability>,
) -> Result<u32> {
    let mut queued_count = 0u32;
    for availability in store.list_receipt_availability_objects().await? {
        match queue_bundle_fetch(
            store,
            swarm,
            bootstrap_addrs,
            identity,
            availability,
            requested,
            pending,
        )
        .await
        {
            Ok(true) => queued_count += 1,
            Ok(false) => {}
            Err(error) => warn!("failed to queue stored receipt availability: {error}"),
        }
    }
    Ok(queued_count)
}

async fn queue_bundle_fetch(
    store: &ProtocolStore,
    swarm: &mut libp2p::Swarm<OscirisBehaviour>,
    bootstrap_addrs: &[Multiaddr],
    identity: &NodeIdentity,
    availability: ReceiptAvailability,
    requested: &mut BTreeSet<(uuid::Uuid, String)>,
    pending: &mut BTreeMap<OutboundRequestId, ReceiptAvailability>,
) -> Result<bool> {
    if availability.provider_node_id == identity.node_id {
        return Ok(false);
    }
    verify_receipt_availability(&availability)?;
    let key = (availability.job_id, availability.provider_node_id.clone());
    if !requested.insert(key) {
        return Ok(false);
    }

    let provider_peer =
        peer_id_from_ed25519_public_key_base64(&availability.provider_ed25519_public_key_base64)?;
    for addr in bootstrap_addrs {
        if peer_id_from_multiaddr(addr) == Some(provider_peer) {
            swarm.add_peer_address(provider_peer, addr.clone());
        }
    }
    if let Some(presence) = store
        .load_peer_presence(&availability.provider_node_id)
        .await?
    {
        for listen_addr in presence.listen_addrs {
            match listen_addr.parse::<Multiaddr>() {
                Ok(addr) => {
                    if peer_id_from_multiaddr(&addr)
                        .map(|peer_id| peer_id == provider_peer)
                        .unwrap_or(true)
                    {
                        swarm.add_peer_address(provider_peer, addr);
                    }
                }
                Err(error) => warn!(
                    "ignoring invalid listen address for provider {}: {error}",
                    availability.provider_node_id
                ),
            }
        }
    }
    let request_id = swarm.behaviour_mut().bundle_transfer.send_request(
        &provider_peer,
        BundleTransferRequest {
            job_id: availability.job_id,
            provider_node_id: availability.provider_node_id.clone(),
        },
    );
    pending.insert(request_id, availability);
    Ok(true)
}

fn verify_receipt_availability(availability: &ReceiptAvailability) -> Result<()> {
    let verifying_key =
        verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
    verify_receipt_availability_signature(availability, &verifying_key)?;
    Ok(())
}

async fn build_bundle_transfer_response(
    store: &ProtocolStore,
    protocol_root: &Path,
    identity: &NodeIdentity,
    request: BundleTransferRequest,
) -> Result<BundleTransferResponse> {
    if request.provider_node_id != identity.node_id {
        bail!(
            "bundle request provider {} does not match local provider {}",
            request.provider_node_id,
            identity.node_id
        );
    }
    let availability = store
        .load_receipt_availability(&request.job_id.to_string(), &request.provider_node_id)
        .await?
        .with_context(|| {
            format!(
                "no local receipt availability for job {} from provider {}",
                request.job_id, request.provider_node_id
            )
        })?;
    verify_receipt_availability(&availability)?;
    let evidence_dir = evidence_dir_from_availability(protocol_root, &availability)?;
    let archive_bytes = archive_evidence_dir(&evidence_dir)?;
    Ok(BundleTransferResponse {
        job_id: availability.job_id,
        provider_node_id: availability.provider_node_id,
        execution_receipt_sha256: availability.execution_receipt_sha256,
        bundle_sha256: availability.bundle_sha256,
        archive_tgz_base64: Some(BASE64.encode(archive_bytes)),
        error: None,
    })
}

async fn store_bundle_transfer_response(
    store: &ProtocolStore,
    protocol_root: &Path,
    availability: &ReceiptAvailability,
    response: BundleTransferResponse,
) -> Result<FetchedBundle> {
    if let Some(error) = response.error {
        bail!("provider returned bundle transfer error: {error}");
    }
    if response.job_id != availability.job_id
        || response.provider_node_id != availability.provider_node_id
        || response.execution_receipt_sha256 != availability.execution_receipt_sha256
        || response.bundle_sha256 != availability.bundle_sha256
    {
        bail!("bundle transfer response metadata does not match signed availability");
    }
    let archive_tgz_base64 = response
        .archive_tgz_base64
        .context("bundle transfer response did not include an archive")?;
    let archive_bytes = BASE64.decode(archive_tgz_base64)?;
    let evidence_dir = protocol_root
        .join("evidence")
        .join(availability.job_id.to_string());
    unpack_evidence_archive(&archive_bytes, &evidence_dir)?;
    let bundle = validate_fetched_evidence(&evidence_dir, availability)?;
    store.record_receipt_bundle(&bundle).await?;
    Ok(FetchedBundle {
        job_id: availability.job_id,
        provider_node_id: availability.provider_node_id.clone(),
        provider_ed25519_public_key_base64: availability.provider_ed25519_public_key_base64.clone(),
        evidence_dir,
        execution_receipt_sha256: availability.execution_receipt_sha256.clone(),
        bundle_sha256: availability.bundle_sha256.clone(),
    })
}

fn evidence_dir_from_availability(
    protocol_root: &Path,
    availability: &ReceiptAvailability,
) -> Result<PathBuf> {
    if let Some(path) = local_path_from_bundle_uri(&availability.bundle_uri) {
        if path.is_dir() {
            return Ok(path);
        }
    }
    let fallback = protocol_root
        .join("evidence")
        .join(availability.job_id.to_string());
    if fallback.is_dir() {
        return Ok(fallback);
    }
    bail!(
        "no local evidence directory found for job {} at advertised URI {} or fallback {}",
        availability.job_id,
        availability.bundle_uri,
        fallback.display()
    );
}

fn local_path_from_bundle_uri(uri: &str) -> Option<PathBuf> {
    if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("s3://") {
        return None;
    }
    let path = uri
        .strip_prefix("file://localhost")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    Some(PathBuf::from(path))
}

fn archive_evidence_dir(evidence_dir: &Path) -> Result<Vec<u8>> {
    if !evidence_dir.is_dir() {
        bail!(
            "evidence directory does not exist: {}",
            evidence_dir.display()
        );
    }
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = Builder::new(encoder);
    archive.append_dir_all(".", evidence_dir)?;
    let encoder = archive.into_inner()?;
    Ok(encoder.finish()?)
}

fn unpack_evidence_archive(archive_bytes: &[u8], destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::create_dir_all(destination)?;
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);
    archive.unpack(destination)?;
    Ok(())
}

fn validate_fetched_evidence(
    evidence_dir: &Path,
    availability: &ReceiptAvailability,
) -> Result<ReceiptBundle> {
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
    let execution_receipt: ExecutionReceipt = serde_json::from_slice(
        &fs::read(&execution_receipt_path)
            .with_context(|| format!("failed to read {}", execution_receipt_path.display()))?,
    )?;
    if execution_receipt.job_id != availability.job_id {
        bail!(
            "fetched execution receipt job_id {} does not match availability job_id {}",
            execution_receipt.job_id,
            availability.job_id
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
    Ok(bundle)
}

pub fn job_matches_provider_capability(
    announcement: &JobAnnouncement,
    capability: &ProviderCapability,
) -> bool {
    if !capability
        .supported_job_types
        .iter()
        .any(|job_type| job_type == &announcement.job_type)
    {
        return false;
    }
    if !capability
        .supported_job_types
        .iter()
        .any(|job_type| job_type == &announcement.job_spec.job_type)
    {
        return false;
    }

    let required = announcement.required_capability.trim().to_ascii_lowercase();
    if required.is_empty() || required == "any" {
        return true;
    }
    if let Some(required_vram) = parse_required_gpu_vram_gb(&required) {
        return capability.gpu_count > 0 && capability.vram_gb >= required_vram;
    }
    false
}

fn parse_required_gpu_vram_gb(required: &str) -> Option<f64> {
    required
        .strip_prefix("gpu>=")
        .and_then(|value| value.strip_suffix("gb"))
        .and_then(|value| value.parse::<f64>().ok())
}

fn create_receipt_availability_from_evidence(
    evidence_dir: &Path,
    identity: &NodeIdentity,
    signing_key: &ed25519_dalek::SigningKey,
) -> Result<ReceiptAvailability> {
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let bundle_path = evidence_dir.join("receipt_bundle.json");
    let execution_receipt: ExecutionReceipt = serde_json::from_slice(
        &fs::read(&execution_receipt_path)
            .with_context(|| format!("failed to read {}", execution_receipt_path.display()))?,
    )?;
    if execution_receipt.provider_id != identity.node_id {
        bail!(
            "execution receipt provider_id {} does not match local provider {}",
            execution_receipt.provider_id,
            identity.node_id
        );
    }
    let bundle: ReceiptBundle = serde_json::from_slice(
        &fs::read(&bundle_path)
            .with_context(|| format!("failed to read {}", bundle_path.display()))?,
    )?;
    if bundle.job_id != execution_receipt.job_id {
        bail!(
            "receipt bundle job_id {} does not match execution receipt job_id {}",
            bundle.job_id,
            execution_receipt.job_id
        );
    }
    let default_bundle_uri = evidence_dir
        .canonicalize()
        .map(|path| format!("file://{}", path.display()))
        .unwrap_or_else(|_| evidence_dir.display().to_string());
    let mut availability = ReceiptAvailability {
        job_id: execution_receipt.job_id,
        provider_node_id: identity.node_id.clone(),
        provider_ed25519_public_key_base64: identity.ed25519_public_key_base64.clone(),
        execution_receipt_sha256: sha256_file(&execution_receipt_path)?,
        bundle_sha256: bundle.bundle_sha256,
        bundle_uri: default_bundle_uri,
        announced_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    availability.signature = sign_receipt_availability(&availability, signing_key)?;
    Ok(availability)
}

async fn claim_job_if_needed(
    store: &ProtocolStore,
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    announcement: &JobAnnouncement,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<Option<JobClaim>> {
    if store
        .load_job_claim(&announcement.job_id.to_string(), &identity.node_id)
        .await?
        .is_some()
    {
        return Ok(None);
    }

    let mut claim = JobClaim {
        job_id: announcement.job_id,
        provider_node_id: identity.node_id.clone(),
        provider_ed25519_public_key_base64: identity.ed25519_public_key_base64.clone(),
        claimed_at: chrono::Utc::now().to_rfc3339(),
        claim_note: Some(format!("matched {}", announcement.required_capability)),
        signature: String::new(),
    };
    claim.signature = sign_job_claim(&claim, signing_key)?;
    store.record_job_claim(&claim).await?;
    publish_job_claim(gossipsub, topic, &claim, signing_key, identity)?;
    Ok(Some(claim))
}

async fn execute_and_publish_ready_assignments(
    store: &ProtocolStore,
    gossipsub: &mut Gossipsub,
    receipt_availability_topic: &IdentTopic,
    context: &ProviderExecutionContext<'_>,
) -> Result<Vec<ReceiptAvailability>> {
    let mut announced = Vec::new();
    for assignment in store
        .load_job_assignments_by_provider(&context.identity.node_id)
        .await?
    {
        match execute_assigned_job_if_ready(
            store,
            context.config,
            context.identity,
            context.signing_key,
            context.capability,
            &trusted_assigners(context.config)?,
            assignment.job_id,
        )
        .await
        {
            Ok(Some(availability)) => {
                publish_receipt_availability(
                    gossipsub,
                    receipt_availability_topic,
                    &availability,
                    context.signing_key,
                    context.identity,
                )?;
                announced.push(availability);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    "auto provider failed stored assigned job {}: {error}",
                    assignment.job_id
                );
            }
        }
    }
    Ok(announced)
}

async fn execute_assigned_job_if_ready(
    store: &ProtocolStore,
    config: &AutoProviderConfig,
    identity: &NodeIdentity,
    signing_key: &ed25519_dalek::SigningKey,
    capability: &ProviderCapability,
    trusted_assigners: &BTreeSet<String>,
    job_id: uuid::Uuid,
) -> Result<Option<ReceiptAvailability>> {
    let assignment = store.load_job_assignment(&job_id.to_string()).await?;
    let Some(assignment) = assignment else {
        return Ok(None);
    };
    if assignment.assigned_provider_node_id != identity.node_id {
        return Ok(None);
    }
    ensure_assignment_trusted(&assignment, trusted_assigners)?;
    if store
        .load_receipt_availability(&job_id.to_string(), &identity.node_id)
        .await?
        .is_some()
    {
        return Ok(None);
    }
    let announcement = store.load_job_announcement(&job_id.to_string()).await?;
    let Some(announcement) = announcement else {
        return Ok(None);
    };
    if !job_matches_provider_capability(&announcement, capability) {
        return Ok(None);
    }

    let provider = ProviderConfig {
        provider_id: identity.node_id.clone(),
        signing_key_id: config.signing_key_id.clone(),
        signing_key_seed_base64: config.signing_key_seed_base64.clone(),
        repo_root: config.repo_root.clone(),
        work_root: config.work_root.clone(),
    };
    let output = run_job(&announcement.job_spec, &provider).await?;
    let availability =
        create_receipt_availability_from_evidence(&output.evidence_dir, identity, signing_key)?;
    store.record_receipt_availability(&availability).await?;
    Ok(Some(availability))
}

fn trusted_assigners(config: &AutoProviderConfig) -> Result<BTreeSet<String>> {
    if config.trusted_assigner_public_keys_base64.is_empty() {
        bail!("network run-provider requires at least one --trusted-assigner-public-key-base64");
    }

    let mut trusted = BTreeSet::new();
    for key in &config.trusted_assigner_public_keys_base64 {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        verifying_key_from_base64(key).with_context(|| "invalid trusted assigner public key")?;
        trusted.insert(key.to_string());
    }

    if trusted.is_empty() {
        bail!("network run-provider requires at least one non-empty trusted assigner public key");
    }

    Ok(trusted)
}

fn ensure_assignment_trusted(
    assignment: &JobAssignment,
    trusted_assigners: &BTreeSet<String>,
) -> Result<()> {
    if trusted_assigners.contains(&assignment.assigner_ed25519_public_key_base64) {
        return Ok(());
    }

    bail!(
        "job assignment {} was signed by untrusted assigner {}",
        assignment.job_id,
        assignment.assigner_node_id
    )
}

fn publish_presence(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    local: &LocalPresenceContext<'_>,
) -> Result<()> {
    let mut presence = PeerPresence {
        node_id: local.identity.node_id.clone(),
        role: local.identity.role.clone(),
        ed25519_public_key_base64: local.identity.ed25519_public_key_base64.clone(),
        evm_address: local.identity.evm_address.clone(),
        listen_addrs: vec![local.listen_addr.to_string()],
        relay_capable: false,
        protocol_version: "0.1.0".to_string(),
        client_version: "0.1.0".to_string(),
        status: local.status.clone(),
        current_load: local.current_load,
        active_job_count: local.active_job_count,
        last_seen_at: chrono::Utc::now().to_rfc3339(),
        capability_version: None,
        signature: String::new(),
    };
    presence.signature = sign_peer_presence(&presence, local.signing_key)?;
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&presence)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_presence(store: &ProtocolStore, presence: PeerPresence) -> Result<()> {
    let verifying_key = verifying_key_from_base64(&presence.ed25519_public_key_base64)?;
    verify_peer_presence_signature(&presence, &verifying_key)?;
    store.record_peer_presence(&presence).await
}

fn publish_capability(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    capability: &ProviderCapability,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<()> {
    let mut capability = capability.clone();
    capability.node_id = identity.node_id.clone();
    capability.ed25519_public_key_base64 = identity.ed25519_public_key_base64.clone();
    capability.signature = sign_provider_capability(&capability, signing_key)?;
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&capability)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_capability(
    store: &ProtocolStore,
    capability: ProviderCapability,
) -> Result<()> {
    let verifying_key = verifying_key_from_base64(&capability.ed25519_public_key_base64)?;
    verify_provider_capability_signature(&capability, &verifying_key)?;
    store.record_provider_capability(&capability).await
}

fn publish_job_announcement(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    announcement: &JobAnnouncement,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<()> {
    let mut announcement = announcement.clone();
    announcement.submitter_node_id = identity.node_id.clone();
    announcement.submitter_ed25519_public_key_base64 = identity.ed25519_public_key_base64.clone();
    announcement.signature = sign_job_announcement(&announcement, signing_key)?;
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&announcement)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_job_announcement(
    store: &ProtocolStore,
    announcement: JobAnnouncement,
) -> Result<()> {
    let verifying_key =
        verifying_key_from_base64(&announcement.submitter_ed25519_public_key_base64)?;
    verify_job_announcement_signature(&announcement, &verifying_key)?;
    store.record_job_announcement(&announcement).await
}

fn publish_job_claim(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    claim: &JobClaim,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<()> {
    let mut claim = claim.clone();
    claim.provider_node_id = identity.node_id.clone();
    claim.provider_ed25519_public_key_base64 = identity.ed25519_public_key_base64.clone();
    claim.signature = sign_job_claim(&claim, signing_key)?;
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&claim)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_job_claim(store: &ProtocolStore, claim: JobClaim) -> Result<()> {
    let verifying_key = verifying_key_from_base64(&claim.provider_ed25519_public_key_base64)?;
    verify_job_claim_signature(&claim, &verifying_key)?;
    store.record_job_claim(&claim).await
}

fn publish_job_assignment(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    assignment: &JobAssignment,
) -> Result<()> {
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&assignment)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_job_assignment(
    store: &ProtocolStore,
    assignment: JobAssignment,
) -> Result<()> {
    let verifying_key = verifying_key_from_base64(&assignment.assigner_ed25519_public_key_base64)?;
    verify_job_assignment_signature(&assignment, &verifying_key)?;
    store.record_job_assignment(&assignment).await
}

fn publish_receipt_availability(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    availability: &ReceiptAvailability,
    signing_key: &ed25519_dalek::SigningKey,
    identity: &NodeIdentity,
) -> Result<()> {
    let mut availability = availability.clone();
    availability.provider_node_id = identity.node_id.clone();
    availability.provider_ed25519_public_key_base64 = identity.ed25519_public_key_base64.clone();
    availability.signature = sign_receipt_availability(&availability, signing_key)?;
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&availability)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_receipt_availability(
    store: &ProtocolStore,
    availability: ReceiptAvailability,
) -> Result<()> {
    let verifying_key =
        verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
    verify_receipt_availability_signature(&availability, &verifying_key)?;
    store.record_receipt_availability(&availability).await
}

fn publish_verification_receipt(
    gossipsub: &mut Gossipsub,
    topic: &IdentTopic,
    receipt: &VerificationReceipt,
    identity: &NodeIdentity,
) -> Result<()> {
    if receipt.verifier_id != identity.node_id {
        return Ok(());
    }
    let announcement = VerificationReceiptAnnouncement {
        verifier_node_id: identity.node_id.clone(),
        verifier_ed25519_public_key_base64: identity.ed25519_public_key_base64.clone(),
        verification_receipt: receipt.clone(),
    };
    if let Err(error) = gossipsub.publish(topic.clone(), serde_json::to_vec(&announcement)?) {
        if !matches!(error, PublishError::InsufficientPeers) {
            return Err(anyhow::Error::new(error));
        }
    }
    Ok(())
}

async fn verify_and_store_verification_receipt(
    store: &ProtocolStore,
    announcement: VerificationReceiptAnnouncement,
) -> Result<()> {
    if announcement.verification_receipt.verifier_id != announcement.verifier_node_id {
        bail!(
            "verification receipt verifier_id {} does not match announcement verifier {}",
            announcement.verification_receipt.verifier_id,
            announcement.verifier_node_id
        );
    }
    let verifying_key =
        verifying_key_from_base64(&announcement.verifier_ed25519_public_key_base64)?;
    verify_verification_receipt_signature(&announcement.verification_receipt, &verifying_key)?;
    store
        .record_verification_receipt(&announcement.verification_receipt)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        load_signing_key_from_base64_seed, verifying_key_to_base64, JobType, PrivacyMode,
        PrivacyPolicy,
    };
    use osciris_verifier::{verify_bundle, VerifierConfig};

    fn announcement(required_capability: &str) -> JobAnnouncement {
        let job_spec = osciris_core::JobSpec {
            job_id: uuid::Uuid::now_v7(),
            job_type: JobType::LlmLoraEconomics,
            dataset: Some("enterprise_synthetic".to_string()),
            model_id: Some("mock-7b".to_string()),
            command: "mock_llm_lora_economics.py".to_string(),
            args: vec![],
            privacy_policy: PrivacyPolicy {
                privacy_mode: PrivacyMode::DspPrepared,
                release_object: "model".to_string(),
                formal_dp_claim: false,
                sensitive_field_policy: "configured_guard".to_string(),
                evidence_profile: "network_workflow_mock".to_string(),
            },
            required_verifier_count: 1,
            challenge_window_seconds: 3600,
            payment_token: "USDC_TEST".to_string(),
            escrow_amount_atomic: "1000000".to_string(),
            created_at: "2026-06-04T00:00:00Z".to_string(),
        };
        JobAnnouncement {
            job_id: job_spec.job_id,
            job_spec,
            submitter_node_id: "enterprise-1".to_string(),
            submitter_ed25519_public_key_base64: "enterprise-public".to_string(),
            job_type: JobType::LlmLoraEconomics,
            privacy_mode: PrivacyMode::DspPrepared,
            required_capability: required_capability.to_string(),
            estimated_runtime_class: "short".to_string(),
            payment_token: "USDC_TEST".to_string(),
            escrow_amount_atomic: "1000000".to_string(),
            required_verifier_count: 1,
            announced_at: "2026-06-04T00:00:00Z".to_string(),
            signature: "signature".to_string(),
        }
    }

    fn capability(vram_gb: f64, supported_job_types: Vec<JobType>) -> ProviderCapability {
        ProviderCapability {
            node_id: "provider-1".to_string(),
            ed25519_public_key_base64: "provider-public".to_string(),
            host_class: "aws-g5".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb,
            cuda_available: true,
            mps_available: false,
            supported_job_types,
            supported_runtimes: vec!["python".to_string()],
            pricing_hint: None,
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: "2026-06-04T00:00:00Z".to_string(),
            signature: "signature".to_string(),
        }
    }

    fn assignment(assigner_public_key: String) -> JobAssignment {
        JobAssignment {
            job_id: uuid::Uuid::now_v7(),
            assigned_provider_node_id: "provider-1".to_string(),
            assigner_node_id: "enterprise-1".to_string(),
            assigner_ed25519_public_key_base64: assigner_public_key,
            assignment_reason: "test".to_string(),
            assigned_at: "2026-06-04T00:00:00Z".to_string(),
            signature: "signature".to_string(),
        }
    }

    fn trusted_key(byte: u8) -> String {
        let seed = BASE64.encode([byte; 32]);
        let signing_key = load_signing_key_from_base64_seed(&seed).unwrap();
        verifying_key_to_base64(&signing_key.verifying_key())
    }

    fn start_llama_cpp_test_server(response_body: String) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn inference_request_response_signatures_verify() {
        let requester_seed = BASE64.encode([21_u8; 32]);
        let provider_seed = BASE64.encode([22_u8; 32]);
        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: requester_seed,
            requester_id: "developer-1".to_string(),
            profile_id: "osciris-test-profile".to_string(),
            prompt: "Explain a public function.".to_string(),
            max_output_tokens: 64,
        })
        .unwrap();
        let provider_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let response = build_inference_response(
            &request,
            &InferenceServeConfig {
                protocol_root: PathBuf::from("/tmp/unused"),
                signing_key_seed_base64: provider_seed,
                signing_key_id: None,
                provider_id: "provider-1".to_string(),
                profile_id: "osciris-test-profile".to_string(),
                runtime: InferenceRuntimeConfig::Deterministic,
                listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
                bootstrap_peers: vec![],
                run_for: Duration::from_secs(1),
            },
            &provider_key,
        )
        .await
        .unwrap();
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(response.request_sha256, request.request_sha256);
        verify_inference_response_signature(&response, &provider_key.verifying_key()).unwrap();
    }

    #[tokio::test]
    async fn inference_llama_cpp_runtime_signs_endpoint_response() {
        let requester_seed = BASE64.encode([23_u8; 32]);
        let provider_seed = BASE64.encode([24_u8; 32]);
        let endpoint = start_llama_cpp_test_server(
            serde_json::json!({
                "content": "llama-cpp-smoke: empty input raises IndexError."
            })
            .to_string(),
        );
        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: requester_seed,
            requester_id: "developer-llama".to_string(),
            profile_id: "osciris-test-profile".to_string(),
            prompt: "Explain a public function.".to_string(),
            max_output_tokens: 64,
        })
        .unwrap();
        let provider_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let response = build_inference_response(
            &request,
            &InferenceServeConfig {
                protocol_root: PathBuf::from("/tmp/unused"),
                signing_key_seed_base64: provider_seed,
                signing_key_id: None,
                provider_id: "provider-llama".to_string(),
                profile_id: "osciris-test-profile".to_string(),
                runtime: InferenceRuntimeConfig::LlamaCppServer { endpoint },
                listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
                bootstrap_peers: vec![],
                run_for: Duration::from_secs(1),
            },
            &provider_key,
        )
        .await
        .unwrap();
        assert_eq!(
            response.response_text,
            "llama-cpp-smoke: empty input raises IndexError."
        );
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(response.request_sha256, request.request_sha256);
        verify_inference_response_signature(&response, &provider_key.verifying_key()).unwrap();
    }

    #[test]
    fn pinned_profile_install_rejects_hash_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("wrong.gguf");
        std::fs::write(&source, b"not-the-pinned-model").unwrap();
        let error = install_pinned_inference_profile(temp.path(), &source).unwrap_err();
        assert!(error
            .to_string()
            .contains("pinned profile artifact SHA-256 mismatch"));
    }

    #[tokio::test]
    async fn managed_llama_runtime_uses_local_endpoint() {
        let endpoint = start_llama_cpp_test_server(
            serde_json::json!({
                "content": "managed-llama-smoke: verified local runtime launch path."
            })
            .to_string(),
        );
        let url = reqwest::Url::parse(&endpoint).unwrap();
        let requester_seed = BASE64.encode([27_u8; 32]);
        let provider_seed = BASE64.encode([28_u8; 32]);
        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: requester_seed,
            requester_id: "developer-managed".to_string(),
            profile_id: "osciris-test-profile".to_string(),
            prompt: "Explain a public function.".to_string(),
            max_output_tokens: 64,
        })
        .unwrap();
        let provider_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let response = build_inference_response(
            &request,
            &InferenceServeConfig {
                protocol_root: PathBuf::from("/tmp/unused"),
                signing_key_seed_base64: provider_seed,
                signing_key_id: None,
                provider_id: "provider-managed".to_string(),
                profile_id: "osciris-test-profile".to_string(),
                runtime: InferenceRuntimeConfig::ManagedLlamaCpp {
                    llama_server_path: PathBuf::from("/usr/bin/llama-server"),
                    model_path: PathBuf::from("/models/Qwen3-4B-Q4_K_M.gguf"),
                    host: url.host_str().unwrap().to_string(),
                    port: url.port().unwrap(),
                    ctx_size: 8192,
                },
                listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
                bootstrap_peers: vec![],
                run_for: Duration::from_secs(1),
            },
            &provider_key,
        )
        .await
        .unwrap();
        assert_eq!(
            response.response_text,
            "managed-llama-smoke: verified local runtime launch path."
        );
    }

    #[tokio::test]
    async fn inference_submit_round_trip_stores_verifier_ready_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let provider_seed = BASE64.encode([25_u8; 32]);
        let requester_seed = BASE64.encode([26_u8; 32]);
        let provider_peer_id = peer_id_from_signing_seed(&provider_seed).unwrap();
        let provider_root = temp.path().join("provider");
        let requester_root = temp.path().join("requester");
        std::fs::create_dir_all(provider_root.join(".osciris")).unwrap();
        std::fs::create_dir_all(requester_root.join(".osciris")).unwrap();

        let serve = tokio::spawn({
            let provider_root = provider_root.clone();
            let provider_seed = provider_seed.clone();
            async move {
                serve_inference(&InferenceServeConfig {
                    protocol_root: provider_root.join(".osciris"),
                    signing_key_seed_base64: provider_seed,
                    signing_key_id: Some("provider-inference-key".to_string()),
                    provider_id: "provider-roundtrip".to_string(),
                    profile_id: "osciris-test-profile".to_string(),
                    runtime: InferenceRuntimeConfig::Deterministic,
                    listen_addr: "/ip4/127.0.0.1/tcp/48201".to_string(),
                    bootstrap_peers: vec![],
                    run_for: Duration::from_secs(10),
                })
                .await
            }
        });

        tokio::time::sleep(Duration::from_millis(300)).await;

        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: requester_seed.clone(),
            requester_id: "requester-roundtrip".to_string(),
            profile_id: "osciris-test-profile".to_string(),
            prompt: "Explain this public function.".to_string(),
            max_output_tokens: 32,
        })
        .unwrap();
        let summary = wait_for_inference_response(&InferenceWaitConfig {
            protocol_root: requester_root.join(".osciris"),
            signing_key_seed_base64: requester_seed,
            request,
            provider_peer_id: provider_peer_id.clone(),
            listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
            bootstrap_peers: vec![format!("/ip4/127.0.0.1/tcp/48201/p2p/{provider_peer_id}")],
            timeout: Duration::from_secs(10),
        })
        .await
        .unwrap();

        let serve_summary = serve.await.unwrap().unwrap();
        assert_eq!(serve_summary.served_request_count, 1);
        assert!(summary.evidence_dir.join("execution_receipt.json").exists());
        assert!(summary.evidence_dir.join("receipt_bundle.json").exists());
        assert!(summary.evidence_dir.join("bundle_index.json").exists());
        assert!(summary.evidence_dir.join("python-output/inference_economics.json").exists());
        assert_eq!(summary.response.provider_node_id, "provider-roundtrip");
        assert!(!summary.execution_receipt_sha256.is_empty());
        assert!(!summary.bundle_sha256.is_empty());
    }

    #[tokio::test]
    async fn interactive_inference_evidence_verifies_locally() {
        let temp = tempfile::tempdir().unwrap();
        let provider_seed = BASE64.encode([27_u8; 32]);
        let requester_seed = BASE64.encode([28_u8; 32]);
        let verifier_seed = BASE64.encode([29_u8; 32]);
        let provider_peer_id = peer_id_from_signing_seed(&provider_seed).unwrap();
        let provider_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let provider_public_key = verifying_key_to_base64(&provider_key.verifying_key());
        let provider_root = temp.path().join("provider");
        let requester_root = temp.path().join("requester");
        std::fs::create_dir_all(provider_root.join(".osciris")).unwrap();
        std::fs::create_dir_all(requester_root.join(".osciris")).unwrap();

        let serve = tokio::spawn({
            let provider_root = provider_root.clone();
            let provider_seed = provider_seed.clone();
            async move {
                serve_inference(&InferenceServeConfig {
                    protocol_root: provider_root.join(".osciris"),
                    signing_key_seed_base64: provider_seed,
                    signing_key_id: Some("provider-inference-key".to_string()),
                    provider_id: "provider-verify".to_string(),
                    profile_id: "osciris-test-profile".to_string(),
                    runtime: InferenceRuntimeConfig::Deterministic,
                    listen_addr: "/ip4/127.0.0.1/tcp/48202".to_string(),
                    bootstrap_peers: vec![],
                    run_for: Duration::from_secs(10),
                })
                .await
            }
        });

        tokio::time::sleep(Duration::from_millis(300)).await;

        let request = create_inference_request(&InferenceSubmitConfig {
            signing_key_seed_base64: requester_seed.clone(),
            requester_id: "requester-verify".to_string(),
            profile_id: "osciris-test-profile".to_string(),
            prompt: "Explain this public function.".to_string(),
            max_output_tokens: 32,
        })
        .unwrap();
        let summary = wait_for_inference_response(&InferenceWaitConfig {
            protocol_root: requester_root.join(".osciris"),
            signing_key_seed_base64: requester_seed,
            request,
            provider_peer_id: provider_peer_id.clone(),
            listen_addr: "/ip4/127.0.0.1/tcp/0".to_string(),
            bootstrap_peers: vec![format!("/ip4/127.0.0.1/tcp/48202/p2p/{provider_peer_id}")],
            timeout: Duration::from_secs(10),
        })
        .await
        .unwrap();

        let serve_summary = serve.await.unwrap().unwrap();
        assert_eq!(serve_summary.served_request_count, 1);

        let verifier = VerifierConfig {
            verifier_id: "verifier-a".to_string(),
            signing_key_id: "verifier-a-key".to_string(),
            signing_key_seed_base64: verifier_seed,
        };
        let verify_output = verify_bundle(&summary.evidence_dir, &provider_public_key, &verifier)
            .await
            .unwrap();
        assert!(verify_output.verification_receipt_path.exists());
    }

    #[test]
    fn job_matching_accepts_sufficient_gpu_vram() {
        assert!(job_matches_provider_capability(
            &announcement("gpu>=24gb"),
            &capability(24.0, vec![JobType::LlmLoraEconomics])
        ));
    }

    #[test]
    fn job_matching_rejects_insufficient_gpu_vram() {
        assert!(!job_matches_provider_capability(
            &announcement("gpu>=24gb"),
            &capability(16.0, vec![JobType::LlmLoraEconomics])
        ));
    }

    #[test]
    fn assignment_trust_accepts_configured_assigner_key() {
        let assigner = trusted_key(7);
        let trusted = BTreeSet::from([assigner.clone()]);
        ensure_assignment_trusted(&assignment(assigner), &trusted).unwrap();
    }

    #[test]
    fn assignment_trust_rejects_unconfigured_assigner_key() {
        let trusted = BTreeSet::from([trusted_key(7)]);
        let err = ensure_assignment_trusted(&assignment(trusted_key(8)), &trusted).unwrap_err();
        assert!(err.to_string().contains("untrusted assigner"));
    }

    #[test]
    fn job_matching_rejects_unsupported_job_type() {
        assert!(!job_matches_provider_capability(
            &announcement("gpu>=24gb"),
            &capability(24.0, vec![JobType::ProductionProof])
        ));
    }
}
