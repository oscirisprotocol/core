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
    bundle_hash, load_signing_key_from_base64_seed, sha256_file, sign_job_announcement,
    sign_job_claim, sign_peer_presence, sign_provider_capability, sign_receipt_availability,
    verify_job_announcement_signature, verify_job_assignment_signature, verify_job_claim_signature,
    verify_peer_presence_signature, verify_provider_capability_signature,
    verify_receipt_availability_signature, verify_verification_receipt_signature,
    verifying_key_from_base64, ExecutionReceipt, JobAnnouncement, JobAssignment, JobClaim,
    NodeIdentity, NodeStatus, PeerPresence, ProviderCapability, ReceiptAvailability, ReceiptBundle,
    VerificationReceipt, VerificationReceiptAnnouncement,
};
use tar::{Archive, Builder};
use tracing::{info, warn};

use crate::store::ProtocolStore;
use crate::{run_job, ProviderConfig};

const PRESENCE_TOPIC: &str = "osciris/network/presence";
const CAPABILITY_TOPIC: &str = "osciris/network/capabilities";
const JOB_ANNOUNCEMENT_TOPIC: &str = "osciris/jobs/announcements";
const JOB_CLAIM_TOPIC: &str = "osciris/jobs/claims";
const JOB_ASSIGNMENT_TOPIC: &str = "osciris/jobs/assignments";
const RECEIPT_AVAILABILITY_TOPIC: &str = "osciris/jobs/receipts";
const VERIFICATION_RECEIPT_TOPIC: &str = "osciris/jobs/verifications";
const BUNDLE_TRANSFER_PROTOCOL: &str = "/osciris/bundle-transfer/0.1.0";

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
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub presence_interval: Duration,
    pub run_for: Duration,
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

impl Default for BundleTransferCodec {
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

fn invalid_data(error: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub fn peer_id_from_signing_seed(seed_base64: &str) -> Result<String> {
    let signing_key = load_signing_key_from_base64_seed(seed_base64)?;
    let keypair = identity::Keypair::ed25519_from_bytes(signing_key.to_bytes())
        .map_err(anyhow::Error::new)?;
    Ok(PeerId::from_public_key(&keypair.public()).to_string())
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
    let behaviour = OscirisBehaviour {
        gossipsub,
        bundle_transfer,
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
    job_id: uuid::Uuid,
) -> Result<Option<ReceiptAvailability>> {
    let assignment = store.load_job_assignment(&job_id.to_string()).await?;
    let Some(assignment) = assignment else {
        return Ok(None);
    };
    if assignment.assigned_provider_node_id != identity.node_id {
        return Ok(None);
    }
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
    use osciris_core::{JobType, PrivacyMode, PrivacyPolicy};

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
    fn job_matching_rejects_unsupported_job_type() {
        assert!(!job_matches_provider_capability(
            &announcement("gpu>=24gb"),
            &capability(24.0, vec![JobType::ProductionProof])
        ));
    }
}
