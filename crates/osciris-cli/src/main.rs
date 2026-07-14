use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use clap::{ArgAction, Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::Compression;
use osciris_chain::{
    private_key_from_env, provider_address_from_id, verifier_address_from_id, ChainConfig,
    OscirisChain, RegisterIdentityRequest, SubmitBundleRequest,
};
use osciris_core::{
    bundle_hash, load_signing_key_from_base64_seed, sha256_file, sign_challenge_record,
    sign_job_announcement, sign_job_assignment, sign_job_claim, sign_milestone_record,
    sign_provider_capability, sign_receipt_availability, verify_challenge_record_signature,
    verify_job_claim_signature, verify_milestone_record_signature,
    verify_receipt_availability_signature, verify_verification_receipt_signature,
    verifying_key_from_base64, verifying_key_to_base64, ChainSubmissionStatus, ChallengeReasonCode,
    ChallengeRecord, ChallengeStatus, ExecutionReceipt, JobAnnouncement, JobAssignment, JobClaim,
    JobSpec, JobType, MilestoneRecord, NodeIdentity, NodeRole, NodeStatus, PeerPresence,
    PrivacyMode, PrivacyPolicy, ProviderCapability, ReceiptAvailability, ReceiptBundle,
    VerificationReceipt, VerificationReceiptAnnouncement,
};
use osciris_node::network::{
    auto_fetch_receipts, fetch_receipt_bundle_p2p, peer_id_from_signing_seed, run_auto_provider,
    serve_presence, AutoProviderConfig, AutoVerifierConfig, BundleFetchConfig, NetworkServeConfig,
};
use osciris_node::status::{
    build_provider_network_status, calculate_quorum_status, calculate_settlement_status,
    QuorumStatusReport, SettlementStatusReport,
};
use osciris_node::store::ProtocolStore;
use osciris_node::{run_job, ProviderConfig};
use osciris_verifier::{verify_bundle, verify_bundle_with_chain, VerifierConfig};
use rand::rngs::OsRng;
use rand::RngCore;
use semver::Version;
use serde::{Deserialize, Serialize};
use tar::Builder;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct SubmitJobArgOptions {
    samples: u32,
    eval_samples: u32,
    seed: u32,
    seeds: Option<String>,
    max_steps: u32,
    timeout: u32,
    backend: String,
}

#[derive(Debug, Parser)]
#[command(name = "osciris-node", version, about = "OSCIRIS protocol node CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor {
        #[arg(long)]
        repo_root: Option<PathBuf>,
        #[arg(long)]
        work_root: Option<PathBuf>,
    },
    Demo {
        #[command(subcommand)]
        command: DemoCommands,
    },
    Identity {
        #[command(subcommand)]
        command: IdentityCommands,
    },
    SubmitJob {
        #[arg(long, default_value = "llm_lora_economics")]
        job_type: String,
        #[arg(long, default_value = "enterprise_synthetic")]
        dataset: String,
        #[arg(long, default_value = "Qwen/Qwen2.5-7B-Instruct")]
        model_id: String,
        #[arg(long, default_value = "uv run osciris llm-lora-economics")]
        command: String,
        #[arg(long, default_value = "transformers_causal_lm")]
        backend: String,
        #[arg(long, default_value_t = 24)]
        samples: u32,
        #[arg(long)]
        seeds: Option<String>,
        #[arg(long, default_value_t = 8)]
        eval_samples: u32,
        #[arg(long, default_value_t = 11)]
        seed: u32,
        #[arg(long, default_value_t = 20)]
        max_steps: u32,
        #[arg(long, default_value_t = 300)]
        timeout: u32,
        #[arg(long, default_value_t = 1)]
        required_verifier_count: u8,
        #[arg(long, default_value_t = 3600)]
        challenge_window_seconds: u64,
        #[arg(long, default_value = "USDC_TEST")]
        payment_token: String,
        #[arg(long, default_value = "1000000")]
        escrow_amount_atomic: String,
        #[arg(long)]
        output: PathBuf,
    },
    RunProvider {
        #[arg(long)]
        job_spec: PathBuf,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        work_root: PathBuf,
    },
    RunClaimedJob {
        #[arg(long)]
        job_spec: PathBuf,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        work_root: PathBuf,
    },
    VerifyReceipt {
        #[arg(long)]
        evidence_dir: PathBuf,
        #[arg(long)]
        provider_public_key_base64: Option<String>,
        #[arg(long)]
        chain_config: Option<PathBuf>,
        #[arg(long)]
        verifier_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
    },
    RegisterProvider {
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long, default_value = "HORIZEN_TESTNET_PRIVATE_KEY")]
        private_key_env: String,
        #[arg(long)]
        provider_public_key_base64: String,
        #[arg(long)]
        metadata_uri: String,
        #[arg(long)]
        stake_amount_atomic: String,
    },
    RegisterVerifier {
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long, default_value = "HORIZEN_TESTNET_PRIVATE_KEY")]
        private_key_env: String,
        #[arg(long)]
        verifier_public_key_base64: String,
        #[arg(long)]
        metadata_uri: String,
    },
    CreateEscrow {
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long)]
        job_spec: PathBuf,
        #[arg(long, default_value = "HORIZEN_TESTNET_PRIVATE_KEY")]
        private_key_env: String,
    },
    FinalizeSettlement {
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long, default_value = "HORIZEN_TESTNET_PRIVATE_KEY")]
        private_key_env: String,
    },
    SubmitReceipt {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long)]
        provider_address: Option<String>,
        #[arg(long = "verifier-address")]
        verifier_addresses: Vec<String>,
        #[arg(long, default_value = "HORIZEN_TESTNET_PRIVATE_KEY")]
        private_key_env: String,
    },
    WatchChain {
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        chain_config: PathBuf,
        #[arg(long, default_value_t = false)]
        follow: bool,
        #[arg(long, default_value_t = 15)]
        poll_seconds: u64,
    },
    ExportEvidence {
        #[arg(long)]
        evidence_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ListJobs {
        #[arg(long)]
        work_root: PathBuf,
    },
    Node {
        #[command(subcommand)]
        command: NodeCommands,
    },
    Network {
        #[command(subcommand)]
        command: NetworkCommands,
    },
}

#[derive(Debug, Subcommand)]
enum DemoCommands {
    LocalSettlement {
        #[arg(long)]
        work_root: Option<PathBuf>,
        #[arg(long)]
        repo_root: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        keep_artifacts: bool,
    },
    ContributorFlow {
        #[arg(long)]
        work_root: Option<PathBuf>,
        #[arg(long)]
        repo_root: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        keep_artifacts: bool,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum IdentityCommands {
    Generate {
        #[arg(long)]
        node_id: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        work_root: Option<PathBuf>,
        /// Reuse an existing signing seed instead of generating a new one. Deterministically
        /// restores the same identity/peer id, e.g. after a work-root was wiped but the seed
        /// was kept. Mutually exclusive with --signing-key-seed-file.
        #[arg(long, conflicts_with = "signing_key_seed_file")]
        signing_key_seed_base64: Option<String>,
        /// Reuse an existing signing seed read from a file (see --signing-key-seed-base64).
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        evm_private_key_hex: Option<String>,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum NodeCommands {
    Join {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        node_id: String,
        #[arg(long)]
        role: String,
        #[arg(long)]
        ed25519_public_key_base64: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        evm_address: Option<String>,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
    },
    Status {
        #[arg(long)]
        work_root: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum NetworkCommands {
    PeerId {
        #[arg(long)]
        signing_key_seed_base64: String,
    },
    Serve {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen_addr: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
        /// Act as a circuit relay for NAT'd peers. Enable this only on a publicly reachable
        /// node (VPS or port-forwarded host); NAT'd peers reserve a circuit here and then
        /// hole-punch to each other via DCUtR.
        #[arg(long, default_value_t = false)]
        relay_server: bool,
        /// Multiaddr to advertise as ours, e.g. /ip4/203.0.113.7/tcp/4101. Needed when the
        /// public address is not visible on a local interface (AWS elastic IP and similar).
        /// A relay server must know its external address or its reservations are unusable.
        #[arg(long = "external-address")]
        external_addresses: Vec<String>,
        #[arg(long, default_value_t = 5)]
        presence_interval_seconds: u64,
        #[arg(long)]
        run_seconds: Option<u64>,
    },
    ImportPeer {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        presence_json: PathBuf,
    },
    Peers {
        #[arg(long)]
        work_root: PathBuf,
    },
    ImportCapability {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        capability_json: PathBuf,
    },
    CreateProviderCapability {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        node_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long, default_value = "aws_g5_xlarge")]
        host_class: String,
        #[arg(long, default_value = "NVIDIA A10G")]
        gpu_model: String,
        #[arg(long, default_value_t = 1)]
        gpu_count: u32,
        #[arg(long, default_value_t = 24.0)]
        vram_gb: f64,
        #[arg(long, default_value_t = true, action = ArgAction::Set)]
        cuda_available: bool,
        #[arg(long, default_value_t = false, action = ArgAction::Set)]
        mps_available: bool,
        #[arg(long = "supported-job-type")]
        supported_job_types: Vec<String>,
        #[arg(long = "supported-runtime")]
        supported_runtimes: Vec<String>,
        #[arg(long)]
        pricing_hint: Option<String>,
        #[arg(long, default_value_t = 0.0)]
        current_load: f64,
        #[arg(long, default_value_t = 0)]
        active_job_count: u32,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Providers {
        #[arg(long)]
        work_root: PathBuf,
    },
    ProviderStatus {
        #[arg(long)]
        work_root: PathBuf,
    },
    ImportJobAnnouncement {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        announcement_json: PathBuf,
    },
    CreateJobAnnouncement {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_spec: PathBuf,
        #[arg(long)]
        submitter_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long, default_value = "gpu>=24gb")]
        required_capability: String,
        #[arg(long, default_value = "short")]
        estimated_runtime_class: String,
    },
    Jobs {
        #[arg(long)]
        work_root: PathBuf,
    },
    ImportJobClaim {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        claim_json: PathBuf,
    },
    CreateJobClaim {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        claim_note: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Claims {
        #[arg(long)]
        work_root: PathBuf,
    },
    AssignJob {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        assigner_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long, default_value = "manual_assignment")]
        assignment_reason: String,
    },
    Assignments {
        #[arg(long)]
        work_root: PathBuf,
    },
    CreateReceiptAvailability {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        evidence_dir: PathBuf,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
        #[arg(long)]
        bundle_uri: Option<String>,
    },
    PublishMilestone {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        title: String,
        #[arg(long)]
        summary: String,
        #[arg(long = "contributor-node-id")]
        contributor_node_ids: Vec<String>,
        #[arg(long)]
        quality_metric_name: String,
        #[arg(long)]
        quality_metric_value: f64,
        #[arg(long)]
        publisher_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        evidence_dir: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    ImportReceiptAvailability {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        availability_json: PathBuf,
    },
    Receipts {
        #[arg(long)]
        work_root: PathBuf,
    },
    Milestones {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Option<Uuid>,
    },
    Verifications {
        #[arg(long)]
        work_root: PathBuf,
    },
    ImportVerificationReceipt {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        announcement_json: PathBuf,
    },
    OpenChallenge {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        opened_by: String,
        #[arg(long)]
        signing_key_seed_base64: String,
        #[arg(long, default_value = "forbidden_job_transition")]
        reason_code: String,
        #[arg(long)]
        reason_detail: String,
        #[arg(long)]
        bundle_sha256: Option<String>,
    },
    Challenges {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Option<Uuid>,
    },
    ResolveChallenge {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        challenge_id: Uuid,
        #[arg(long)]
        resolver_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
        #[arg(long, default_value = "rejected")]
        resolution: String,
        #[arg(long)]
        resolution_note: Option<String>,
    },
    QuorumStatus {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
    },
    SettlementStatus {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
    },
    JobStatus {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
    },
    #[command(about = "Participant-facing workflow snapshot with milestones")]
    ParticipantStatus {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    FetchReceiptBundle {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        provider_id: String,
    },
    FetchReceiptBundleP2p {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        signing_key_seed_base64: String,
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen_addr: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
        #[arg(long)]
        provider_peer_id: String,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        provider_id: String,
        #[arg(long, default_value_t = 30)]
        timeout_seconds: u64,
    },
    #[command(
        alias = "sync-published",
        about = "Synchronize published bundles for beta collaboration clients"
    )]
    SyncPublishedUpdates {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long, default_value = "https://oscirislabs.com")]
        base_url: String,
        #[arg(long, default_value_t = 300)]
        poll_seconds: u64,
        #[arg(long, default_value_t = false)]
        watch: bool,
    },
    #[command(about = "Check the public beta release manifest for newer OSCIRIS binaries")]
    CheckUpdates {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long, default_value = "https://oscirislabs.com")]
        base_url: String,
        #[arg(long, default_value_t = 300)]
        poll_seconds: u64,
        #[arg(long, default_value_t = false)]
        watch: bool,
    },
    VerifyDiscoveredReceipt {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        job_id: Uuid,
        #[arg(long)]
        provider_id: String,
        #[arg(long)]
        verifier_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
    },
    RunVerifier {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        verifier_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen_addr: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
        #[arg(long, default_value_t = 5)]
        presence_interval_seconds: u64,
        #[arg(long, default_value_t = 30)]
        run_seconds: u64,
        #[arg(long, default_value_t = 8)]
        announce_seconds: u64,
    },
    RunProvider {
        #[arg(long)]
        work_root: PathBuf,
        #[arg(long)]
        signing_key_seed_base64: Option<String>,
        #[arg(long)]
        signing_key_seed_file: Option<PathBuf>,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long = "trusted-assigner-public-key-base64", required = true)]
        trusted_assigner_public_keys_base64: Vec<String>,
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen_addr: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
        #[arg(long, default_value_t = 5)]
        presence_interval_seconds: u64,
        #[arg(long, default_value_t = 60)]
        run_seconds: u64,
    },
}

#[derive(Debug, Serialize)]
struct CommandAvailability {
    available: bool,
    path: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct DspDoctorStatus {
    invoked: bool,
    ok: bool,
    exit_code: Option<i32>,
    output_json: Option<serde_json::Value>,
    stderr: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    ready: bool,
    cli_version: &'static str,
    platform: String,
    architecture: String,
    work_root: String,
    work_root_writable: bool,
    protocol_store_ready: bool,
    python3: CommandAvailability,
    uv: CommandAvailability,
    forge: CommandAvailability,
    dsp_repo_root: Option<String>,
    dsp_repo_valid: Option<bool>,
    dsp_doctor: Option<DspDoctorStatus>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct LocalSettlementDemoSummary {
    ready: bool,
    work_root: String,
    repo_root: String,
    kept_artifacts: bool,
    workflow: String,
    identity_files: serde_json::Value,
    job_id: Uuid,
    provider_a_executed: bool,
    provider_b_executed: bool,
    quorum_status: String,
    settlement_ready: bool,
    lifecycle_state: String,
    files: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GeneratedIdentity {
    node_id: String,
    role: NodeRole,
    display_name: String,
    signing_key_seed_base64: String,
    ed25519_public_key_base64: String,
    peer_id: String,
    evm_address: Option<String>,
    evm_private_key_hex: Option<String>,
    bootstrap_peers: Vec<String>,
    node_identity: NodeIdentity,
    suggested_commands: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ContributorGpuPeerDemoSummary {
    ready: bool,
    work_root: String,
    repo_root: String,
    contributor_manifest: serde_json::Value,
    settlement: LocalSettlementDemoSummary,
}

#[derive(Debug, Serialize)]
struct PublishedUpdateSyncSummary {
    ready: bool,
    work_root: String,
    base_url: String,
    poll_seconds: u64,
    watch: bool,
    sync_count: u64,
    output_dir: String,
    participant_status: String,
    proof_feed: String,
    contributor_manifest: String,
    beta_release_manifest: String,
    last_synced_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BetaReleaseAsset {
    platform: String,
    filename: String,
    url: String,
    sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BetaReleaseManifest {
    channel: String,
    latest_version: String,
    published_at: String,
    release_page_url: String,
    release_notes: String,
    assets: Vec<BetaReleaseAsset>,
}

#[derive(Debug, Serialize)]
struct BetaReleaseCheckSummary {
    ready: bool,
    work_root: String,
    base_url: String,
    poll_seconds: u64,
    watch: bool,
    current_version: String,
    latest_version: String,
    update_available: bool,
    comparison: String,
    channel: String,
    release_page_url: String,
    recommended_platform: Option<String>,
    recommended_download_url: Option<String>,
    release_notes: String,
    beta_release_manifest: String,
    checked_at: String,
}

fn main() -> Result<()> {
    // Long-running commands (`network serve`, `run-provider`, `run-verifier`) print nothing
    // until they exit, so without a default log level an operator sees a silent, seemingly
    // hung process. Default to info-level for our own crate (plus mDNS discovery lines and
    // connection warnings) so `serve` shows its listen addresses and peer activity out of the
    // box; RUST_LOG still overrides this entirely when set.
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(value) if !value.trim().is_empty() => EnvFilter::new(value),
        _ => EnvFilter::new("warn,osciris_node=info,osciris_cli=info,libp2p_mdns=info"),
    };
    // Logs go to stderr so they never corrupt the JSON that data commands print to stdout.
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    match cli.command {
        Commands::Doctor {
            repo_root,
            work_root,
        } => {
            let report = runtime.block_on(run_doctor(repo_root, work_root))?;
            print_json(&report)?;
            if !report.ready {
                std::process::exit(1);
            }
        }
        Commands::Demo { command } => match command {
            DemoCommands::LocalSettlement {
                work_root,
                repo_root,
                keep_artifacts,
            } => {
                let summary = runtime.block_on(run_local_settlement_demo(
                    work_root,
                    repo_root,
                    keep_artifacts,
                ))?;
                print_json(&summary)?;
                if !summary.ready {
                    std::process::exit(1);
                }
            }
            DemoCommands::ContributorFlow {
                work_root,
                repo_root,
                keep_artifacts,
                output,
            } => {
                let summary = runtime.block_on(run_contributor_gpu_peer_demo(
                    work_root,
                    repo_root,
                    keep_artifacts,
                    output,
                ))?;
                print_json(&summary)?;
                if !summary.ready {
                    std::process::exit(1);
                }
            }
        },
        Commands::Identity { command } => match command {
            IdentityCommands::Generate {
                node_id,
                role,
                display_name,
                work_root,
                signing_key_seed_base64,
                signing_key_seed_file,
                evm_private_key_hex,
                bootstrap_peers,
                output,
            } => {
                let existing_seed = if signing_key_seed_base64.is_some()
                    || signing_key_seed_file.is_some()
                {
                    Some(load_signing_seed_from_source(
                        signing_key_seed_base64,
                        signing_key_seed_file,
                    )?)
                } else {
                    None
                };
                let generated = runtime.block_on(generate_identity(
                    node_id,
                    role,
                    display_name,
                    work_root,
                    existing_seed,
                    evm_private_key_hex,
                    bootstrap_peers,
                ))?;
                if let Some(output) = output {
                    fs::write(&output, serde_json::to_vec_pretty(&generated)?)?;
                }
                print_json(&generated)?;
            }
        },
        Commands::SubmitJob {
            job_type,
            dataset,
            model_id,
            command,
            backend,
            samples,
            seeds,
            eval_samples,
            seed,
            max_steps,
            timeout,
            required_verifier_count,
            challenge_window_seconds,
            payment_token,
            escrow_amount_atomic,
            output,
        } => {
            let parsed_job_type = parse_job_type(&job_type)?;
            let command = default_command_for_job_type(&parsed_job_type, &command);
            let args = structured_submit_job_args(
                &parsed_job_type,
                SubmitJobArgOptions {
                    samples,
                    eval_samples,
                    seed,
                    seeds,
                    max_steps,
                    timeout,
                    backend,
                },
            );
            let job = JobSpec {
                job_id: Uuid::now_v7(),
                job_type: parsed_job_type.clone(),
                dataset: Some(dataset),
                model_id: Some(model_id),
                command,
                args,
                privacy_policy: PrivacyPolicy {
                    privacy_mode: PrivacyMode::DspPrepared,
                    release_object: release_object_for_job_type(&parsed_job_type).to_string(),
                    formal_dp_claim: false,
                    sensitive_field_policy: "configured_guard".to_string(),
                    evidence_profile: evidence_profile_for_job_type(&parsed_job_type).to_string(),
                },
                required_verifier_count,
                challenge_window_seconds,
                payment_token,
                escrow_amount_atomic,
                created_at: Utc::now().to_rfc3339(),
            };
            std::fs::write(output, serde_json::to_vec_pretty(&job)?)?;
            println!("{}", job.job_id);
        }
        Commands::RunProvider {
            job_spec,
            provider_id,
            signing_key_id,
            signing_key_seed_base64,
            signing_key_seed_file,
            repo_root,
            work_root,
        } => {
            let job: JobSpec = serde_json::from_slice(&std::fs::read(job_spec)?)?;
            let provider = ProviderConfig {
                provider_id,
                signing_key_id,
                signing_key_seed_base64: load_signing_seed_from_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?,
                repo_root,
                work_root,
            };
            let output = runtime.block_on(run_job(&job, &provider))?;
            println!("{}", output.evidence_dir.display());
        }
        Commands::RunClaimedJob {
            job_spec,
            provider_id,
            signing_key_id,
            signing_key_seed_base64,
            repo_root,
            work_root,
        } => {
            let job: JobSpec = serde_json::from_slice(&std::fs::read(&job_spec)?)?;
            let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
            let claim =
                runtime.block_on(store.load_job_claim(&job.job_id.to_string(), &provider_id))?;
            let claim = claim.ok_or_else(|| {
                anyhow!(
                    "provider {provider_id} has no persisted claim for job {}; import or receive a signed claim first",
                    job.job_id
                )
            })?;
            let provider_key = verifying_key_from_base64(&claim.provider_ed25519_public_key_base64)
                .context("failed to decode provider claim public key")?;
            verify_job_claim_signature(&claim, &provider_key)
                .context("provider claim signature is invalid")?;
            let provider = ProviderConfig {
                provider_id,
                signing_key_id,
                signing_key_seed_base64,
                repo_root,
                work_root,
            };
            let output = runtime.block_on(run_job(&job, &provider))?;
            println!("{}", output.evidence_dir.display());
        }
        Commands::VerifyReceipt {
            evidence_dir,
            provider_public_key_base64,
            chain_config,
            verifier_id,
            signing_key_id,
            signing_key_seed_base64,
        } => {
            let verifier = VerifierConfig {
                verifier_id,
                signing_key_id,
                signing_key_seed_base64,
            };
            let output = if let Some(chain_config) = chain_config {
                let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
                runtime.block_on(verify_bundle_with_chain(&evidence_dir, &chain, &verifier))?
            } else {
                let provider_public_key_base64 =
                    provider_public_key_base64.as_deref().ok_or_else(|| {
                        anyhow!("provider_public_key_base64 is required without --chain-config")
                    })?;
                runtime.block_on(verify_bundle(
                    &evidence_dir,
                    provider_public_key_base64,
                    &verifier,
                ))?
            };
            println!("{}", output.verification_receipt_path.display());
        }
        Commands::RegisterProvider {
            chain_config,
            private_key_env,
            provider_public_key_base64,
            metadata_uri,
            stake_amount_atomic,
        } => {
            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            let tx_hash = runtime.block_on(chain.register_provider(
                &private_key_from_env(&private_key_env)?,
                RegisterIdentityRequest {
                    metadata_uri,
                    ed25519_public_key_base64: provider_public_key_base64,
                    stake_token: None,
                    stake_amount: Some(stake_amount_atomic.parse()?),
                },
            ))?;
            println!("{tx_hash}");
        }
        Commands::RegisterVerifier {
            chain_config,
            private_key_env,
            verifier_public_key_base64,
            metadata_uri,
        } => {
            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            let tx_hash = runtime.block_on(chain.register_verifier(
                &private_key_from_env(&private_key_env)?,
                RegisterIdentityRequest {
                    metadata_uri,
                    ed25519_public_key_base64: verifier_public_key_base64,
                    stake_token: None,
                    stake_amount: None,
                },
            ))?;
            println!("{tx_hash}");
        }
        Commands::CreateEscrow {
            chain_config,
            job_spec,
            private_key_env,
        } => {
            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            let job: JobSpec = serde_json::from_slice(&std::fs::read(job_spec)?)?;
            let tx_hash = runtime.block_on(
                chain.create_job_escrow(&private_key_from_env(&private_key_env)?, &job),
            )?;
            println!("{tx_hash}");
        }
        Commands::FinalizeSettlement {
            chain_config,
            job_id,
            private_key_env,
        } => {
            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            let tx_hash = runtime.block_on(
                chain.finalize_settlement(&private_key_from_env(&private_key_env)?, job_id),
            )?;
            println!("{tx_hash}");
        }
        Commands::SubmitReceipt {
            work_root,
            job_id,
            chain_config,
            provider_address,
            verifier_addresses,
            private_key_env,
        } => {
            let evidence_dir = work_root
                .join(".osciris")
                .join("evidence")
                .join(job_id.to_string());
            let bundle_path = evidence_dir.join("receipt_bundle.json");
            let execution_receipt_path = evidence_dir.join("execution_receipt.json");
            let mut bundle: ReceiptBundle = serde_json::from_slice(
                &std::fs::read(&bundle_path)
                    .with_context(|| format!("failed to read {}", bundle_path.display()))?,
            )?;
            let execution_receipt: osciris_core::ExecutionReceipt =
                serde_json::from_slice(&std::fs::read(&execution_receipt_path)?)?;
            let verification_receipts = load_verification_receipts(&evidence_dir)?;
            if verification_receipts.is_empty() {
                bail!(
                    "no verification receipts found under {}",
                    evidence_dir.display()
                );
            }

            let verifier_addresses =
                resolve_verifier_addresses(&verification_receipts, &verifier_addresses)?;
            let verifier_set = verifier_addresses.iter().copied().collect::<BTreeSet<_>>();
            if verifier_set.len() != verifier_addresses.len() {
                bail!("duplicate verifier addresses found in verification receipts");
            }
            let provider_address =
                resolve_provider_address(&execution_receipt, provider_address.as_deref())?;

            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            let submission = runtime.block_on(chain.submit_receipt_bundle(
                &private_key_from_env(&private_key_env)?,
                SubmitBundleRequest {
                    job_id,
                    provider_address,
                    execution_receipt_sha256: bundle.execution_receipt_sha256.clone(),
                    bundle_sha256: bundle.bundle_sha256.clone(),
                    verifier_receipt_sha256_list: bundle.verification_receipt_sha256_list.clone(),
                    verifier_addresses,
                },
            ))?;
            let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
            runtime.block_on(store.record_chain_submission(
                &job_id.to_string(),
                &submission.receipt_registry_tx_hash,
                &submission.escrow_tx_hash,
            ))?;
            bundle.chain_submission_status = ChainSubmissionStatus::Submitted;
            std::fs::write(&bundle_path, serde_json::to_vec_pretty(&bundle)?)?;
            runtime.block_on(store.record_receipt_bundle(&bundle))?;
            print_json(&submission)?;
        }
        Commands::WatchChain {
            job_id,
            chain_config,
            follow,
            poll_seconds,
        } => {
            let chain = OscirisChain::new(ChainConfig::from_path(&chain_config)?)?;
            if follow {
                loop {
                    let snapshot = runtime.block_on(chain.watch_job(job_id))?;
                    print_json(&snapshot)?;
                    thread::sleep(Duration::from_secs(poll_seconds));
                }
            } else {
                let snapshot = runtime.block_on(chain.watch_job(job_id))?;
                print_json(&snapshot)?;
            }
        }
        Commands::ExportEvidence {
            evidence_dir,
            output,
        } => {
            let file = File::create(output)?;
            let encoder = GzEncoder::new(file, Compression::default());
            let mut archive = Builder::new(encoder);
            archive.append_dir_all(".", evidence_dir)?;
            archive.finish()?;
        }
        Commands::ListJobs { work_root } => {
            let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
            for job in runtime.block_on(store.list_jobs())? {
                println!(
                    "{}\t{}\t{}\t{}",
                    job.job_id,
                    job.status,
                    job.evidence_dir.unwrap_or_default(),
                    job.metrics_path.unwrap_or_default()
                );
            }
        }
        Commands::Node { command } => match command {
            NodeCommands::Join {
                work_root,
                node_id,
                role,
                ed25519_public_key_base64,
                display_name,
                evm_address,
                bootstrap_peers,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let identity = NodeIdentity {
                    node_id,
                    role: parse_node_role(&role)?,
                    ed25519_public_key_base64,
                    evm_address,
                    display_name,
                    bootstrap_peers,
                    created_at: Utc::now().to_rfc3339(),
                };
                runtime.block_on(store.record_node_identity(&identity))?;
                print_json(&identity)?;
            }
            NodeCommands::Status { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let identity = runtime.block_on(store.load_node_identity())?;
                print_json(&identity)?;
            }
        },
        Commands::Network { command } => match command {
            NetworkCommands::PeerId {
                signing_key_seed_base64,
            } => {
                println!("{}", peer_id_from_signing_seed(&signing_key_seed_base64)?);
            }
            NetworkCommands::Serve {
                work_root,
                signing_key_seed_base64,
                signing_key_seed_file,
                listen_addr,
                bootstrap_peers,
                relay_server,
                external_addresses,
                presence_interval_seconds,
                run_seconds,
            } => {
                let signing_key_seed_base64 =
                    load_signing_seed_from_source(signing_key_seed_base64, signing_key_seed_file)?;
                let summary = runtime.block_on(serve_presence(&NetworkServeConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64,
                    listen_addr,
                    bootstrap_peers,
                    relay_server,
                    external_addresses,
                    status: osciris_core::NodeStatus::OnlineIdle,
                    current_load: 0.0,
                    active_job_count: 0,
                    presence_interval: Duration::from_secs(presence_interval_seconds),
                    run_for: run_seconds.map(Duration::from_secs),
                }))?;
                print_json(&summary)?;
            }
            NetworkCommands::ImportPeer {
                work_root,
                presence_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let presence: PeerPresence = serde_json::from_slice(
                    &std::fs::read(&presence_json)
                        .with_context(|| format!("failed to read {}", presence_json.display()))?,
                )?;
                runtime.block_on(store.record_peer_presence(&presence))?;
                print_json(&presence)?;
            }
            NetworkCommands::Peers { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let peers = runtime.block_on(store.list_peer_presences())?;
                print_json(&peers)?;
            }
            NetworkCommands::ImportCapability {
                work_root,
                capability_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let capability: ProviderCapability = serde_json::from_slice(
                    &std::fs::read(&capability_json)
                        .with_context(|| format!("failed to read {}", capability_json.display()))?,
                )?;
                runtime.block_on(store.record_provider_capability(&capability))?;
                print_json(&capability)?;
            }
            NetworkCommands::CreateProviderCapability {
                work_root,
                node_id,
                signing_key_seed_base64,
                signing_key_seed_file,
                host_class,
                gpu_model,
                gpu_count,
                vram_gb,
                cuda_available,
                mps_available,
                supported_job_types,
                supported_runtimes,
                pricing_hint,
                current_load,
                active_job_count,
                output,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let signing_key = load_signing_key_from_seed_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?;
                let capability = create_signed_provider_capability(
                    &node_id,
                    &signing_key,
                    &host_class,
                    &gpu_model,
                    gpu_count,
                    vram_gb,
                    cuda_available,
                    mps_available,
                    &supported_job_types,
                    &supported_runtimes,
                    pricing_hint,
                    current_load,
                    active_job_count,
                )?;
                runtime.block_on(store.record_provider_capability(&capability))?;
                if let Some(output) = output {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    fs::write(&output, serde_json::to_vec_pretty(&capability)?)
                        .with_context(|| format!("failed to write {}", output.display()))?;
                }
                print_json(&capability)?;
            }
            NetworkCommands::Providers { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let providers = runtime.block_on(store.list_provider_capabilities())?;
                print_json(&providers)?;
            }
            NetworkCommands::ProviderStatus { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let capabilities = runtime.block_on(store.list_provider_capabilities())?;
                let claims = runtime.block_on(store.list_job_claims())?;
                let assignments = runtime.block_on(store.list_job_assignments())?;
                let receipt_availability = runtime.block_on(store.list_receipt_availability())?;
                let report = build_provider_network_status(
                    &capabilities,
                    &claims,
                    &assignments,
                    &receipt_availability,
                );
                print_json(&report)?;
            }
            NetworkCommands::ImportJobAnnouncement {
                work_root,
                announcement_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement: JobAnnouncement =
                    serde_json::from_slice(&std::fs::read(&announcement_json).with_context(
                        || format!("failed to read {}", announcement_json.display()),
                    )?)?;
                runtime.block_on(store.record_job_announcement(&announcement))?;
                print_json(&announcement)?;
            }
            NetworkCommands::CreateJobAnnouncement {
                work_root,
                job_spec,
                submitter_id,
                signing_key_seed_base64,
                signing_key_seed_file,
                required_capability,
                estimated_runtime_class,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let job: JobSpec = serde_json::from_slice(
                    &std::fs::read(&job_spec)
                        .with_context(|| format!("failed to read {}", job_spec.display()))?,
                )?;
                let signing_key = load_signing_key_from_seed_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?;
                let mut announcement = JobAnnouncement {
                    job_id: job.job_id,
                    job_spec: job.clone(),
                    submitter_node_id: submitter_id,
                    submitter_ed25519_public_key_base64: verifying_key_to_base64(
                        &signing_key.verifying_key(),
                    ),
                    job_type: job.job_type.clone(),
                    privacy_mode: job.privacy_policy.privacy_mode.clone(),
                    required_capability,
                    estimated_runtime_class,
                    payment_token: job.payment_token.clone(),
                    escrow_amount_atomic: job.escrow_amount_atomic.clone(),
                    required_verifier_count: job.required_verifier_count,
                    announced_at: Utc::now().to_rfc3339(),
                    signature: String::new(),
                };
                announcement.signature = sign_job_announcement(&announcement, &signing_key)?;
                runtime.block_on(store.record_job_announcement(&announcement))?;
                print_json(&announcement)?;
            }
            NetworkCommands::Jobs { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcements = runtime.block_on(store.list_job_announcements())?;
                print_json(&announcements)?;
            }
            NetworkCommands::ImportJobClaim {
                work_root,
                claim_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let claim: JobClaim = serde_json::from_slice(
                    &std::fs::read(&claim_json)
                        .with_context(|| format!("failed to read {}", claim_json.display()))?,
                )?;
                runtime.block_on(store.record_job_claim(&claim))?;
                print_json(&claim)?;
            }
            NetworkCommands::CreateJobClaim {
                work_root,
                job_id,
                provider_id,
                signing_key_seed_base64,
                signing_key_seed_file,
                claim_note,
                output,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement = runtime
                    .block_on(store.load_job_announcement(&job_id.to_string()))?
                    .with_context(|| format!("cannot create a claim for unknown job {job_id}"))?;
                let capability = runtime
                    .block_on(store.load_provider_capability(&provider_id))?
                    .with_context(|| {
                        format!("cannot create a claim for provider {provider_id}; publish provider capability first")
                    })?;
                let signing_key = load_signing_key_from_seed_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?;
                let provider_public_key = verifying_key_to_base64(&signing_key.verifying_key());
                if capability.ed25519_public_key_base64 != provider_public_key {
                    bail!(
                        "provider capability public key does not match the supplied signing key for {provider_id}"
                    );
                }

                let claim = signed_job_claim_with_note(
                    &provider_id,
                    &provider_public_key,
                    &signing_key,
                    job_id,
                    claim_note
                        .or_else(|| Some(format!("matched {}", announcement.required_capability))),
                )?;
                runtime.block_on(store.record_job_claim(&claim))?;
                if let Some(output) = output {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    fs::write(&output, serde_json::to_vec_pretty(&claim)?)
                        .with_context(|| format!("failed to write {}", output.display()))?;
                }
                print_json(&claim)?;
            }
            NetworkCommands::Claims { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let claims = runtime.block_on(store.list_job_claims())?;
                print_json(&claims)?;
            }
            NetworkCommands::AssignJob {
                work_root,
                job_id,
                provider_id,
                assigner_id,
                signing_key_seed_base64,
                signing_key_seed_file,
                assignment_reason,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement =
                    runtime.block_on(store.load_job_announcement(&job_id.to_string()))?;
                if announcement.is_none() {
                    bail!("cannot assign unknown job {job_id}; import or create the job announcement first");
                }
                let claim =
                    runtime.block_on(store.load_job_claim(&job_id.to_string(), &provider_id))?;
                if claim.is_none() {
                    bail!("cannot assign job {job_id} to provider {provider_id}; provider has no stored signed claim");
                }
                let signing_key = load_signing_key_from_seed_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?;
                let mut assignment = JobAssignment {
                    job_id,
                    assigned_provider_node_id: provider_id,
                    assigner_node_id: assigner_id,
                    assigner_ed25519_public_key_base64: verifying_key_to_base64(
                        &signing_key.verifying_key(),
                    ),
                    assignment_reason,
                    assigned_at: Utc::now().to_rfc3339(),
                    signature: String::new(),
                };
                assignment.signature = sign_job_assignment(&assignment, &signing_key)?;
                runtime.block_on(store.record_job_assignment(&assignment))?;
                print_json(&assignment)?;
            }
            NetworkCommands::Assignments { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let assignments = runtime.block_on(store.list_job_assignments())?;
                print_json(&assignments)?;
            }
            NetworkCommands::CreateReceiptAvailability {
                work_root,
                evidence_dir,
                provider_id,
                signing_key_seed_base64,
                bundle_uri,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let execution_receipt_path = evidence_dir.join("execution_receipt.json");
                let bundle_path = evidence_dir.join("receipt_bundle.json");
                let execution_receipt: ExecutionReceipt =
                    serde_json::from_slice(&std::fs::read(&execution_receipt_path).with_context(
                        || format!("failed to read {}", execution_receipt_path.display()),
                    )?)?;
                if execution_receipt.provider_id != provider_id {
                    bail!(
                        "execution receipt provider_id {} does not match requested provider_id {}",
                        execution_receipt.provider_id,
                        provider_id
                    );
                }
                let bundle: ReceiptBundle = serde_json::from_slice(
                    &std::fs::read(&bundle_path)
                        .with_context(|| format!("failed to read {}", bundle_path.display()))?,
                )?;
                if bundle.job_id != execution_receipt.job_id {
                    bail!(
                        "receipt bundle job_id {} does not match execution receipt job_id {}",
                        bundle.job_id,
                        execution_receipt.job_id
                    );
                }

                let signing_key = load_signing_key_from_base64_seed(&signing_key_seed_base64)?;
                let provider_public_key_base64 =
                    verifying_key_to_base64(&signing_key.verifying_key());
                let default_bundle_uri = evidence_dir
                    .canonicalize()
                    .map(|path| format!("file://{}", path.display()))
                    .unwrap_or_else(|_| evidence_dir.display().to_string());
                let mut availability = ReceiptAvailability {
                    job_id: execution_receipt.job_id,
                    provider_node_id: provider_id,
                    provider_ed25519_public_key_base64: provider_public_key_base64,
                    execution_receipt_sha256: sha256_file(&execution_receipt_path)?,
                    bundle_sha256: bundle.bundle_sha256.clone(),
                    bundle_uri: bundle_uri.unwrap_or(default_bundle_uri),
                    announced_at: Utc::now().to_rfc3339(),
                    signature: String::new(),
                };
                availability.signature = sign_receipt_availability(&availability, &signing_key)?;
                runtime.block_on(store.record_receipt_availability(&availability))?;
                print_json(&availability)?;
            }
            NetworkCommands::PublishMilestone {
                work_root,
                job_id,
                title,
                summary,
                contributor_node_ids,
                quality_metric_name,
                quality_metric_value,
                publisher_id,
                signing_key_id,
                signing_key_seed_base64,
                signing_key_seed_file,
                evidence_dir,
                output,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let evidence_dir = evidence_dir.unwrap_or_else(|| {
                    work_root
                        .join(".osciris")
                        .join("evidence")
                        .join(job_id.to_string())
                });
                let execution_receipt_path = evidence_dir.join("execution_receipt.json");
                let bundle_path = evidence_dir.join("receipt_bundle.json");
                let execution_receipt: ExecutionReceipt =
                    serde_json::from_slice(&std::fs::read(&execution_receipt_path).with_context(
                        || format!("failed to read {}", execution_receipt_path.display()),
                    )?)?;
                if execution_receipt.job_id != job_id {
                    bail!(
                        "execution receipt job_id {} does not match requested job_id {}",
                        execution_receipt.job_id,
                        job_id
                    );
                }
                let bundle: ReceiptBundle = serde_json::from_slice(
                    &std::fs::read(&bundle_path)
                        .with_context(|| format!("failed to read {}", bundle_path.display()))?,
                )?;
                if bundle.job_id != job_id {
                    bail!(
                        "receipt bundle job_id {} does not match requested job_id {}",
                        bundle.job_id,
                        job_id
                    );
                }
                let verification_receipts = load_verification_receipts(&evidence_dir)?;
                if verification_receipts.is_empty() {
                    bail!(
                        "no verification receipts found under {}",
                        evidence_dir.display()
                    );
                }
                let signing_key = load_signing_key_from_seed_source(
                    signing_key_seed_base64,
                    signing_key_seed_file,
                )?;
                let mut contributor_set = BTreeSet::new();
                contributor_set.insert(publisher_id.clone());
                contributor_set.insert(execution_receipt.provider_id.clone());
                contributor_set.extend(contributor_node_ids);
                let mut milestone = MilestoneRecord {
                    milestone_id: Uuid::now_v7(),
                    job_id,
                    job_type: execution_receipt.job_type.clone(),
                    title,
                    summary,
                    contributing_node_ids: contributor_set.into_iter().collect(),
                    quality_metric_name,
                    quality_metric_value,
                    evidence_bundle_sha256: bundle.bundle_sha256.clone(),
                    verification_receipt_sha256_list: bundle
                        .verification_receipt_sha256_list
                        .clone(),
                    published_by: publisher_id,
                    published_at: Utc::now().to_rfc3339(),
                    signing_key_id,
                    signature: String::new(),
                };
                milestone.signature = sign_milestone_record(&milestone, &signing_key)?;
                let verifying_key = signing_key.verifying_key();
                verify_milestone_record_signature(&milestone, &verifying_key)?;
                runtime.block_on(store.record_milestone(&milestone))?;
                if let Some(output) = output {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    fs::write(&output, serde_json::to_vec_pretty(&milestone)?)
                        .with_context(|| format!("failed to write {}", output.display()))?;
                }
                print_json(&milestone)?;
            }
            NetworkCommands::ImportReceiptAvailability {
                work_root,
                availability_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let availability: ReceiptAvailability =
                    serde_json::from_slice(&std::fs::read(&availability_json).with_context(
                        || format!("failed to read {}", availability_json.display()),
                    )?)?;
                let verifying_key =
                    verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
                verify_receipt_availability_signature(&availability, &verifying_key)?;
                runtime.block_on(store.record_receipt_availability(&availability))?;
                print_json(&availability)?;
            }
            NetworkCommands::Receipts { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let availability = runtime.block_on(store.list_receipt_availability())?;
                print_json(&availability)?;
            }
            NetworkCommands::Milestones { work_root, job_id } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                if let Some(job_id) = job_id {
                    let milestones =
                        runtime.block_on(store.load_milestones_by_job(&job_id.to_string()))?;
                    print_json(&milestones)?;
                } else {
                    let milestones = runtime.block_on(store.list_milestones())?;
                    print_json(&milestones)?;
                }
            }
            NetworkCommands::Verifications { work_root } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let receipts = runtime.block_on(store.list_verification_receipts())?;
                print_json(&receipts)?;
            }
            NetworkCommands::ImportVerificationReceipt {
                work_root,
                announcement_json,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement: VerificationReceiptAnnouncement =
                    serde_json::from_slice(&std::fs::read(&announcement_json).with_context(
                        || format!("failed to read {}", announcement_json.display()),
                    )?)?;
                let receipt = runtime.block_on(
                    record_verified_verification_receipt_announcement(&store, &announcement),
                )?;
                print_json(&receipt)?;
            }
            NetworkCommands::OpenChallenge {
                work_root,
                job_id,
                opened_by,
                signing_key_seed_base64,
                reason_code,
                reason_detail,
                bundle_sha256,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement =
                    runtime.block_on(store.load_job_announcement(&job_id.to_string()))?;
                if announcement.is_none() {
                    bail!("cannot challenge unknown job {job_id}; import or create the job announcement first");
                }
                let reason_code = parse_challenge_reason_code(&reason_code)?;
                let bundle_sha256 = if let Some(bundle_sha256) = bundle_sha256 {
                    bundle_sha256
                } else {
                    let quorum = runtime.block_on(build_quorum_report(&store, job_id))?;
                    if let Some(bundle_sha256) = quorum.bundle_sha256 {
                        bundle_sha256
                    } else if let Some(availability) = runtime
                        .block_on(store.load_receipt_availability_by_job(&job_id.to_string()))?
                        .first()
                    {
                        availability.bundle_sha256.clone()
                    } else {
                        bail!("cannot open challenge for job {job_id}; no bundle hash is available")
                    }
                };
                let signing_key = load_signing_key_from_base64_seed(&signing_key_seed_base64)?;
                let mut challenge = ChallengeRecord {
                    challenge_id: Uuid::now_v7(),
                    job_id,
                    bundle_sha256,
                    opened_by,
                    opened_by_ed25519_public_key_base64: verifying_key_to_base64(
                        &signing_key.verifying_key(),
                    ),
                    reason_code,
                    reason_detail,
                    opened_at: Utc::now().to_rfc3339(),
                    status: ChallengeStatus::Open,
                    resolved_by: None,
                    resolved_by_ed25519_public_key_base64: None,
                    resolved_at: None,
                    resolution_note: None,
                    signature: String::new(),
                };
                challenge.signature = sign_challenge_record(&challenge, &signing_key)?;
                let verifying_key =
                    verifying_key_from_base64(&challenge.opened_by_ed25519_public_key_base64)?;
                verify_challenge_record_signature(&challenge, &verifying_key)?;
                runtime.block_on(store.record_challenge_record(&challenge))?;
                print_json(&challenge)?;
            }
            NetworkCommands::Challenges { work_root, job_id } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                if let Some(job_id) = job_id {
                    let challenges = runtime
                        .block_on(store.load_challenge_records_by_job(&job_id.to_string()))?;
                    print_json(&challenges)?;
                } else {
                    let challenges = runtime.block_on(store.list_challenge_records())?;
                    print_json(&challenges)?;
                }
            }
            NetworkCommands::ResolveChallenge {
                work_root,
                challenge_id,
                resolver_id,
                signing_key_seed_base64,
                resolution,
                resolution_note,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let mut challenge = runtime
                    .block_on(store.load_challenge_record(&challenge_id.to_string()))?
                    .with_context(|| format!("challenge {challenge_id} not found"))?;
                if challenge.status != ChallengeStatus::Open {
                    bail!(
                        "challenge {} is already resolved with status {:?}",
                        challenge.challenge_id,
                        challenge.status
                    );
                }
                let signing_key = load_signing_key_from_base64_seed(&signing_key_seed_base64)?;
                let resolver_public_key_base64 =
                    verifying_key_to_base64(&signing_key.verifying_key());
                challenge.status = parse_challenge_resolution(&resolution)?;
                challenge.resolved_by = Some(resolver_id);
                challenge.resolved_by_ed25519_public_key_base64 =
                    Some(resolver_public_key_base64.clone());
                challenge.resolved_at = Some(Utc::now().to_rfc3339());
                challenge.resolution_note = resolution_note;
                challenge.signature.clear();
                challenge.signature = sign_challenge_record(&challenge, &signing_key)?;
                let verifying_key = verifying_key_from_base64(&resolver_public_key_base64)?;
                verify_challenge_record_signature(&challenge, &verifying_key)?;
                runtime.block_on(store.record_challenge_record(&challenge))?;
                print_json(&challenge)?;
            }
            NetworkCommands::QuorumStatus { work_root, job_id } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let report = runtime.block_on(build_quorum_report(&store, job_id))?;
                print_json(&report)?;
            }
            NetworkCommands::SettlementStatus { work_root, job_id } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let report = runtime.block_on(build_settlement_report(&store, job_id))?;
                print_json(&report)?;
            }
            NetworkCommands::JobStatus { work_root, job_id } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let announcement =
                    runtime.block_on(store.load_job_announcement(&job_id.to_string()))?;
                let job_spec = runtime.block_on(store.load_job_spec(&job_id.to_string()))?;
                let claims = runtime.block_on(store.load_job_claims_by_job(&job_id.to_string()))?;
                let assignment =
                    runtime.block_on(store.load_job_assignment(&job_id.to_string()))?;
                let receipt_availability = runtime
                    .block_on(store.load_receipt_availability_by_job(&job_id.to_string()))?;
                let verification_receipts = runtime
                    .block_on(store.load_verification_receipts_by_job(&job_id.to_string()))?;
                let quorum = runtime.block_on(build_quorum_report(&store, job_id))?;
                let challenges =
                    runtime.block_on(store.load_challenge_records_by_job(&job_id.to_string()))?;
                let settlement = runtime.block_on(build_settlement_report(&store, job_id))?;
                print_json(&serde_json::json!({
                    "job_id": job_id,
                    "job_spec": job_spec,
                    "job_announcement": announcement,
                    "claims": claims,
                    "assignment": assignment,
                    "receipt_availability": receipt_availability,
                    "verification_receipts": verification_receipts,
                    "quorum": quorum,
                    "challenges": challenges,
                    "settlement": settlement
                }))?;
            }
            NetworkCommands::ParticipantStatus {
                work_root,
                job_id,
                output,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let snapshot =
                    runtime.block_on(build_participant_status_snapshot(&store, job_id))?;
                if let Some(output) = output {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    fs::write(&output, serde_json::to_vec_pretty(&snapshot)?)
                        .with_context(|| format!("failed to write {}", output.display()))?;
                }
                print_json(&snapshot)?;
            }
            NetworkCommands::FetchReceiptBundle {
                work_root,
                job_id,
                provider_id,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let availability = runtime
                    .block_on(store.load_receipt_availability(&job_id.to_string(), &provider_id))?
                    .ok_or_else(|| {
                        anyhow!(
                            "no receipt availability found for job {job_id} from provider {provider_id}"
                        )
                    })?;
                verify_availability_signature(&availability)?;
                let source_dir = local_path_from_bundle_uri(&availability.bundle_uri)?;
                let evidence_dir = work_root
                    .join(".osciris")
                    .join("evidence")
                    .join(job_id.to_string());
                copy_dir_recursive_replace(&source_dir, &evidence_dir)?;
                let bundle = validate_fetched_evidence(&evidence_dir, &availability)?;
                runtime.block_on(store.record_receipt_bundle(&bundle))?;
                print_json(&serde_json::json!({
                    "job_id": job_id,
                    "provider_id": provider_id,
                    "source_dir": source_dir,
                    "evidence_dir": evidence_dir,
                    "execution_receipt_sha256": availability.execution_receipt_sha256,
                    "bundle_sha256": availability.bundle_sha256
                }))?;
            }
            NetworkCommands::FetchReceiptBundleP2p {
                work_root,
                signing_key_seed_base64,
                listen_addr,
                bootstrap_peers,
                provider_peer_id,
                job_id,
                provider_id,
                timeout_seconds,
            } => {
                let fetched = runtime.block_on(fetch_receipt_bundle_p2p(&BundleFetchConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64,
                    listen_addr,
                    bootstrap_peers,
                    provider_peer_id,
                    job_id,
                    provider_node_id: provider_id,
                    timeout: Duration::from_secs(timeout_seconds),
                }))?;
                print_json(&fetched)?;
            }
            NetworkCommands::SyncPublishedUpdates {
                work_root,
                base_url,
                poll_seconds,
                watch,
            } => {
                let summary = runtime.block_on(sync_published_updates(
                    work_root,
                    base_url,
                    poll_seconds,
                    watch,
                ))?;
                print_json(&summary)?;
            }
            NetworkCommands::CheckUpdates {
                work_root,
                base_url,
                poll_seconds,
                watch,
            } => {
                let summary = runtime.block_on(check_beta_release_updates(
                    work_root,
                    base_url,
                    poll_seconds,
                    watch,
                ))?;
                print_json(&summary)?;
            }
            NetworkCommands::VerifyDiscoveredReceipt {
                work_root,
                job_id,
                provider_id,
                verifier_id,
                signing_key_id,
                signing_key_seed_base64,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let availability = runtime
                    .block_on(store.load_receipt_availability(&job_id.to_string(), &provider_id))?
                    .ok_or_else(|| {
                        anyhow!(
                            "no receipt availability found for job {job_id} from provider {provider_id}"
                        )
                    })?;
                verify_availability_signature(&availability)?;
                let evidence_dir = work_root
                    .join(".osciris")
                    .join("evidence")
                    .join(job_id.to_string());
                if !evidence_dir.exists() {
                    let source_dir = local_path_from_bundle_uri(&availability.bundle_uri)?;
                    copy_dir_recursive_replace(&source_dir, &evidence_dir)?;
                    let bundle = validate_fetched_evidence(&evidence_dir, &availability)?;
                    runtime.block_on(store.record_receipt_bundle(&bundle))?;
                }
                let verifier = VerifierConfig {
                    verifier_id,
                    signing_key_id,
                    signing_key_seed_base64,
                };
                let output = runtime.block_on(verify_bundle(
                    &evidence_dir,
                    &availability.provider_ed25519_public_key_base64,
                    &verifier,
                ))?;
                print_json(&serde_json::json!({
                    "job_id": job_id,
                    "provider_id": provider_id,
                    "evidence_dir": evidence_dir,
                    "verification_receipt_path": output.verification_receipt_path,
                    "receipt_bundle_path": output.receipt_bundle_path
                }))?;
            }
            NetworkCommands::RunVerifier {
                work_root,
                signing_key_seed_base64,
                signing_key_seed_file,
                verifier_id,
                signing_key_id,
                listen_addr,
                bootstrap_peers,
                presence_interval_seconds,
                run_seconds,
                announce_seconds,
            } => {
                let signing_key_seed_base64 =
                    load_signing_seed_from_source(signing_key_seed_base64, signing_key_seed_file)?;
                let summary = runtime.block_on(auto_fetch_receipts(&AutoVerifierConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64: signing_key_seed_base64.clone(),
                    listen_addr: listen_addr.clone(),
                    bootstrap_peers: bootstrap_peers.clone(),
                    presence_interval: Duration::from_secs(presence_interval_seconds),
                    run_for: Duration::from_secs(run_seconds),
                }))?;
                let mut verification_results = Vec::new();
                for fetched in &summary.fetched_bundles {
                    let verifier = VerifierConfig {
                        verifier_id: verifier_id.clone(),
                        signing_key_id: signing_key_id.clone(),
                        signing_key_seed_base64: signing_key_seed_base64.clone(),
                    };
                    let output = runtime.block_on(verify_bundle(
                        &fetched.evidence_dir,
                        &fetched.provider_ed25519_public_key_base64,
                        &verifier,
                    ))?;
                    verification_results.push(serde_json::json!({
                        "job_id": fetched.job_id,
                        "provider_id": fetched.provider_node_id,
                        "evidence_dir": fetched.evidence_dir,
                        "verification_receipt_path": output.verification_receipt_path,
                        "receipt_bundle_path": output.receipt_bundle_path
                    }));
                }
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let local_verification_receipt_count = runtime
                    .block_on(store.load_verification_receipts_by_verifier(&verifier_id))?
                    .len();
                let announce_summary = if local_verification_receipt_count == 0 {
                    None
                } else {
                    Some(runtime.block_on(serve_presence(&NetworkServeConfig {
                        protocol_root: work_root.join(".osciris"),
                        signing_key_seed_base64,
                        listen_addr,
                        bootstrap_peers,
                        relay_server: false,
                        external_addresses: Vec::new(),
                        status: osciris_core::NodeStatus::OnlineIdle,
                        current_load: 0.0,
                        active_job_count: 0,
                        presence_interval: Duration::from_secs(presence_interval_seconds),
                        run_for: Some(Duration::from_secs(announce_seconds)),
                    }))?)
                };
                print_json(&serde_json::json!({
                    "fetch_summary": summary,
                    "verification_results": verification_results,
                    "local_verification_receipt_count": local_verification_receipt_count,
                    "announce_summary": announce_summary
                }))?;
            }
            NetworkCommands::RunProvider {
                work_root,
                signing_key_seed_base64,
                signing_key_seed_file,
                signing_key_id,
                repo_root,
                trusted_assigner_public_keys_base64,
                listen_addr,
                bootstrap_peers,
                presence_interval_seconds,
                run_seconds,
            } => {
                let signing_key_seed_base64 =
                    load_signing_seed_from_source(signing_key_seed_base64, signing_key_seed_file)?;
                let summary = runtime.block_on(run_auto_provider(&AutoProviderConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64,
                    signing_key_id,
                    repo_root,
                    work_root,
                    trusted_assigner_public_keys_base64,
                    listen_addr,
                    bootstrap_peers,
                    presence_interval: Duration::from_secs(presence_interval_seconds),
                    run_for: Duration::from_secs(run_seconds),
                }))?;
                print_json(&summary)?;
            }
        },
    }
    Ok(())
}

async fn build_quorum_report(store: &ProtocolStore, job_id: Uuid) -> Result<QuorumStatusReport> {
    let announcement = store.load_job_announcement(&job_id.to_string()).await?;
    let required_verifier_count = announcement
        .as_ref()
        .map(|announcement| announcement.required_verifier_count);
    let required_verifier_count = if let Some(required) = required_verifier_count {
        required
    } else if let Some(job_spec) = store.load_job_spec(&job_id.to_string()).await? {
        job_spec.required_verifier_count
    } else {
        bail!("no job announcement or job spec found for job {job_id}");
    };
    let receipts = store
        .load_verification_receipts_by_job(&job_id.to_string())
        .await?;
    Ok(calculate_quorum_status(
        job_id,
        required_verifier_count,
        &receipts,
    ))
}

async fn build_settlement_report(
    store: &ProtocolStore,
    job_id: Uuid,
) -> Result<SettlementStatusReport> {
    let announcement = store.load_job_announcement(&job_id.to_string()).await?;
    let job_spec = store.load_job_spec(&job_id.to_string()).await?;
    let challenge_window_seconds = announcement
        .as_ref()
        .map(|announcement| announcement.job_spec.challenge_window_seconds)
        .or_else(|| {
            job_spec
                .as_ref()
                .map(|job_spec| job_spec.challenge_window_seconds)
        })
        .unwrap_or(0);
    let claims = store.load_job_claims_by_job(&job_id.to_string()).await?;
    let stored_claims = claims
        .iter()
        .map(|claim| osciris_node::store::StoredJobClaim {
            job_id: claim.job_id.to_string(),
            provider_node_id: claim.provider_node_id.clone(),
            claimed_at: claim.claimed_at.clone(),
            claim_note: claim.claim_note.clone(),
        })
        .collect::<Vec<_>>();
    let assignment_object = store.load_job_assignment(&job_id.to_string()).await?;
    let assignment =
        assignment_object
            .as_ref()
            .map(|assignment| osciris_node::store::StoredJobAssignment {
                job_id: assignment.job_id.to_string(),
                assigned_provider_node_id: assignment.assigned_provider_node_id.clone(),
                assigner_node_id: assignment.assigner_node_id.clone(),
                assignment_reason: assignment.assignment_reason.clone(),
                assigned_at: assignment.assigned_at.clone(),
            });
    let receipt_availability = store
        .load_receipt_availability_by_job(&job_id.to_string())
        .await?;
    let stored_receipt_availability = receipt_availability
        .iter()
        .map(
            |availability| osciris_node::store::StoredReceiptAvailability {
                job_id: availability.job_id.to_string(),
                provider_node_id: availability.provider_node_id.clone(),
                execution_receipt_sha256: availability.execution_receipt_sha256.clone(),
                bundle_sha256: availability.bundle_sha256.clone(),
                bundle_uri: availability.bundle_uri.clone(),
                announced_at: availability.announced_at.clone(),
            },
        )
        .collect::<Vec<_>>();
    let verification_receipts = store
        .load_verification_receipts_by_job(&job_id.to_string())
        .await?;
    let quorum = build_quorum_report(store, job_id).await?;
    let challenges = store
        .load_challenge_records_by_job(&job_id.to_string())
        .await?;
    let receipt_bundle = store.load_receipt_bundle(&job_id.to_string()).await?;
    let chain_submitted = receipt_bundle
        .as_ref()
        .is_some_and(|bundle| bundle.chain_submission_status == ChainSubmissionStatus::Submitted);

    Ok(calculate_settlement_status(
        job_id,
        challenge_window_seconds,
        announcement.is_some() || job_spec.is_some(),
        &stored_claims,
        assignment.as_ref(),
        &stored_receipt_availability,
        &verification_receipts,
        &quorum,
        &challenges,
        receipt_bundle.as_ref(),
        chain_submitted,
        Utc::now(),
    ))
}

async fn build_participant_status_snapshot(
    store: &ProtocolStore,
    job_id: Uuid,
) -> Result<serde_json::Value> {
    let announcement = store.load_job_announcement(&job_id.to_string()).await?;
    let job_spec = store.load_job_spec(&job_id.to_string()).await?;
    let claims = store.load_job_claims_by_job(&job_id.to_string()).await?;
    let assignment = store.load_job_assignment(&job_id.to_string()).await?;
    let receipt_availability = store
        .load_receipt_availability_by_job(&job_id.to_string())
        .await?;
    let verification_receipts = store
        .load_verification_receipts_by_job(&job_id.to_string())
        .await?;
    let milestones = store.load_milestones_by_job(&job_id.to_string()).await?;
    let quorum = build_quorum_report(store, job_id).await?;
    let challenges = store
        .load_challenge_records_by_job(&job_id.to_string())
        .await?;
    let settlement = build_settlement_report(store, job_id).await?;

    Ok(serde_json::json!({
        "job_id": job_id,
        "job_spec": job_spec,
        "job_announcement": announcement,
        "claims": claims,
        "assignment": assignment,
        "receipt_availability": receipt_availability,
        "verification_receipts": verification_receipts,
        "milestones": milestones,
        "participant_summary": {
            "job_id": job_id,
            "claim_count": claims.len(),
            "assignment_present": assignment.is_some(),
            "receipt_availability_count": receipt_availability.len(),
            "verification_receipt_count": verification_receipts.len(),
            "milestone_count": milestones.len(),
            "challenge_count": challenges.len(),
            "quorum_status": quorum.status.clone(),
            "settlement_lifecycle_state": settlement.lifecycle_state.clone()
        },
        "quorum": quorum,
        "challenges": challenges,
        "settlement": settlement
    }))
}

async fn sync_published_updates(
    work_root: PathBuf,
    base_url: String,
    poll_seconds: u64,
    watch: bool,
) -> Result<PublishedUpdateSyncSummary> {
    let normalized_base_url = base_url.trim_end_matches('/').to_string();
    let output_dir = work_root.join(".osciris").join("published");
    fs::create_dir_all(&output_dir)?;

    let client = reqwest::Client::builder().no_proxy().build()?;
    let mut sync_count = 0_u64;
    let participant_status_path = output_dir.join("participant-status-summary.json");
    let proof_feed_path = output_dir.join("proof-feed.json");
    let contributor_manifest_path = output_dir.join("contributor-manifest.json");
    let beta_release_manifest_path = output_dir.join("beta-release-manifest.json");

    loop {
        sync_json_bundle(
            &client,
            &format!("{normalized_base_url}/participant-status-summary.json"),
            &participant_status_path,
        )
        .await?;
        sync_json_bundle(
            &client,
            &format!("{normalized_base_url}/proof-feed.json"),
            &proof_feed_path,
        )
        .await?;
        sync_json_bundle(
            &client,
            &format!("{normalized_base_url}/contributor-manifest.json"),
            &contributor_manifest_path,
        )
        .await?;
        sync_json_bundle(
            &client,
            &format!("{normalized_base_url}/beta-release-manifest.json"),
            &beta_release_manifest_path,
        )
        .await?;

        sync_count += 1;
        let last_sync = Utc::now().to_rfc3339();

        let summary = PublishedUpdateSyncSummary {
            ready: true,
            work_root: work_root.display().to_string(),
            base_url: normalized_base_url.clone(),
            poll_seconds,
            watch,
            sync_count,
            output_dir: output_dir.display().to_string(),
            participant_status: participant_status_path.display().to_string(),
            proof_feed: proof_feed_path.display().to_string(),
            contributor_manifest: contributor_manifest_path.display().to_string(),
            beta_release_manifest: beta_release_manifest_path.display().to_string(),
            last_synced_at: last_sync.clone(),
        };

        if !watch {
            return Ok(summary);
        }

        print_json(&summary)?;
        tokio::time::sleep(Duration::from_secs(poll_seconds.max(1))).await;
    }
}

async fn check_beta_release_updates(
    work_root: PathBuf,
    base_url: String,
    poll_seconds: u64,
    watch: bool,
) -> Result<BetaReleaseCheckSummary> {
    let normalized_base_url = base_url.trim_end_matches('/').to_string();
    let output_dir = work_root.join(".osciris").join("published");
    fs::create_dir_all(&output_dir)?;

    let client = reqwest::Client::builder().no_proxy().build()?;
    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let beta_release_manifest_path = output_dir.join("beta-release-manifest.json");

    loop {
        let manifest = fetch_beta_release_manifest(
            &client,
            &format!("{normalized_base_url}/beta-release-manifest.json"),
        )
        .await?;
        fs::write(
            &beta_release_manifest_path,
            serde_json::to_vec_pretty(&manifest)?,
        )?;

        let latest_version = Version::parse(&manifest.latest_version).with_context(|| {
            format!("invalid beta release version: {}", manifest.latest_version)
        })?;
        let recommended_asset = select_beta_release_asset(&manifest);
        let recommended_download_url = recommended_asset
            .as_ref()
            .map(|asset| asset.url.clone())
            .or_else(|| Some(manifest.release_page_url.clone()));

        let comparison = match latest_version.cmp(&current_version) {
            std::cmp::Ordering::Greater => "update_available",
            std::cmp::Ordering::Equal => "current",
            std::cmp::Ordering::Less => "ahead_of_manifest",
        }
        .to_string();

        let summary = BetaReleaseCheckSummary {
            ready: true,
            work_root: work_root.display().to_string(),
            base_url: normalized_base_url.clone(),
            poll_seconds,
            watch,
            current_version: current_version.to_string(),
            latest_version: latest_version.to_string(),
            update_available: latest_version > current_version,
            comparison,
            channel: manifest.channel.clone(),
            release_page_url: manifest.release_page_url.clone(),
            recommended_platform: recommended_asset
                .as_ref()
                .map(|asset| asset.platform.clone()),
            recommended_download_url,
            release_notes: manifest.release_notes.clone(),
            beta_release_manifest: beta_release_manifest_path.display().to_string(),
            checked_at: Utc::now().to_rfc3339(),
        };

        if !watch {
            return Ok(summary);
        }

        print_json(&summary)?;
        tokio::time::sleep(Duration::from_secs(poll_seconds.max(1))).await;
    }
}

async fn fetch_beta_release_manifest(
    client: &reqwest::Client,
    url: &str,
) -> Result<BetaReleaseManifest> {
    let response = client.get(url).send().await?.error_for_status()?;
    Ok(response.json().await?)
}

fn select_beta_release_asset(manifest: &BetaReleaseManifest) -> Option<&BetaReleaseAsset> {
    let target_platform = current_beta_platform_key();
    manifest
        .assets
        .iter()
        .find(|asset| asset.platform == target_platform)
}

fn current_beta_platform_key() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-aarch64".to_string(),
        ("macos", "x86_64") => "macos-x86_64".to_string(),
        ("linux", "x86_64") => "linux-x86_64".to_string(),
        ("windows", "x86_64") => "windows-x86_64".to_string(),
        (os, arch) => format!("{os}-{arch}"),
    }
}

async fn sync_json_bundle(client: &reqwest::Client, url: &str, output_path: &Path) -> Result<()> {
    let response = client.get(url).send().await?.error_for_status()?;
    let value: serde_json::Value = response.json().await?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

async fn run_doctor(
    repo_root: Option<PathBuf>,
    work_root: Option<PathBuf>,
) -> Result<DoctorReport> {
    let work_root = work_root
        .unwrap_or_else(|| env::temp_dir().join(format!("osciris-doctor-{}", Uuid::now_v7())));
    fs::create_dir_all(&work_root)?;

    let temp_probe_path = work_root.join(".doctor-write-test");
    let work_root_writable = fs::write(&temp_probe_path, b"ok").is_ok();
    if work_root_writable {
        let _ = fs::remove_file(&temp_probe_path);
    }

    let protocol_store_ready = if work_root_writable {
        ProtocolStore::open(&work_root.join(".osciris"))
            .await
            .is_ok()
    } else {
        false
    };

    let python3 = inspect_command("python3", &["--version"]);
    let uv = inspect_command("uv", &["--version"]);
    let forge = inspect_command("forge", &["--version"]);

    let mut warnings = Vec::new();
    if !python3.available {
        warnings.push(
            "python3 is not available; Python-backed provider execution and demos will not run"
                .to_string(),
        );
    }
    if !uv.available {
        warnings.push(
            "uv is not available; DSP repo commands cannot be invoked from this CLI".to_string(),
        );
    }
    if !forge.available {
        warnings.push(
            "forge is not available; smart contract tests and deployments cannot be run locally"
                .to_string(),
        );
    }

    let dsp_repo_valid = repo_root.as_ref().map(|root| {
        root.join("pyproject.toml").exists() && root.join("src/osciris/cli.py").exists()
    });
    if repo_root.is_some() && dsp_repo_valid == Some(false) {
        warnings.push(
            "repo_root does not look like the OSCIRIS DSP repository; skipping DSP health check"
                .to_string(),
        );
    }

    let dsp_doctor = if let (Some(root), Some(true)) = (repo_root.as_ref(), dsp_repo_valid) {
        if uv.available {
            Some(run_dsp_doctor(root)?)
        } else {
            Some(DspDoctorStatus {
                invoked: false,
                ok: false,
                exit_code: None,
                output_json: None,
                stderr: Some("uv is unavailable".to_string()),
            })
        }
    } else {
        None
    };

    let ready = work_root_writable && protocol_store_ready;
    Ok(DoctorReport {
        ready,
        cli_version: env!("CARGO_PKG_VERSION"),
        platform: env::consts::OS.to_string(),
        architecture: env::consts::ARCH.to_string(),
        work_root: work_root.display().to_string(),
        work_root_writable,
        protocol_store_ready,
        python3,
        uv,
        forge,
        dsp_repo_root: repo_root.as_ref().map(|path| path.display().to_string()),
        dsp_repo_valid,
        dsp_doctor,
        warnings,
    })
}

async fn run_local_settlement_demo(
    work_root: Option<PathBuf>,
    repo_root: Option<PathBuf>,
    keep_artifacts: bool,
) -> Result<LocalSettlementDemoSummary> {
    let work_root = work_root
        .unwrap_or_else(|| env::temp_dir().join(format!("osciris-demo-{}", Uuid::now_v7())));
    fs::create_dir_all(&work_root)?;
    let repo_root = repo_root.unwrap_or_else(|| work_root.clone());
    let protocol_root = work_root.join(".osciris");
    let demo_root = work_root.join("demo");
    fs::create_dir_all(&demo_root)?;
    let store = ProtocolStore::open(&protocol_root).await?;

    let enterprise_seed = seed_base64(1);
    let provider_a_seed = seed_base64(2);
    let provider_b_seed = seed_base64(3);
    let verifier_seed = seed_base64(4);

    let enterprise_signing_key = load_signing_key_from_base64_seed(&enterprise_seed)?;
    let provider_a_signing_key = load_signing_key_from_base64_seed(&provider_a_seed)?;
    let provider_b_signing_key = load_signing_key_from_base64_seed(&provider_b_seed)?;
    let verifier_signing_key = load_signing_key_from_base64_seed(&verifier_seed)?;

    let enterprise_public_key = verifying_key_to_base64(&enterprise_signing_key.verifying_key());
    let provider_a_public_key = verifying_key_to_base64(&provider_a_signing_key.verifying_key());
    let provider_b_public_key = verifying_key_to_base64(&provider_b_signing_key.verifying_key());

    let provider_a_capability = signed_provider_capability(
        "provider-a",
        &provider_a_public_key,
        &provider_a_signing_key,
        "aws_g5_xlarge",
    )?;
    let provider_b_capability = signed_provider_capability(
        "provider-b",
        &provider_b_public_key,
        &provider_b_signing_key,
        "aws_g5_xlarge",
    )?;
    store
        .record_provider_capability(&provider_a_capability)
        .await?;
    store
        .record_provider_capability(&provider_b_capability)
        .await?;
    fs::write(
        demo_root.join("provider_a_capability.json"),
        serde_json::to_vec_pretty(&provider_a_capability)?,
    )?;
    fs::write(
        demo_root.join("provider_b_capability.json"),
        serde_json::to_vec_pretty(&provider_b_capability)?,
    )?;

    let job_id = Uuid::now_v7();
    let job = mock_demo_job(job_id);
    let job_spec_path = demo_root.join("job_spec.json");
    fs::write(&job_spec_path, serde_json::to_vec_pretty(&job)?)?;

    let mut announcement = JobAnnouncement {
        job_id,
        job_spec: job.clone(),
        submitter_node_id: "enterprise-1".to_string(),
        submitter_ed25519_public_key_base64: enterprise_public_key.clone(),
        job_type: job.job_type.clone(),
        privacy_mode: job.privacy_policy.privacy_mode.clone(),
        required_capability: "gpu>=24gb".to_string(),
        estimated_runtime_class: "short".to_string(),
        payment_token: job.payment_token.clone(),
        escrow_amount_atomic: job.escrow_amount_atomic.clone(),
        required_verifier_count: job.required_verifier_count,
        announced_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    announcement.signature = sign_job_announcement(&announcement, &enterprise_signing_key)?;
    store.record_job_announcement(&announcement).await?;
    fs::write(
        demo_root.join("job_announcement.json"),
        serde_json::to_vec_pretty(&announcement)?,
    )?;

    let claim_a = signed_job_claim(
        "provider-a",
        &provider_a_public_key,
        &provider_a_signing_key,
        job_id,
    )?;
    let claim_b = signed_job_claim(
        "provider-b",
        &provider_b_public_key,
        &provider_b_signing_key,
        job_id,
    )?;
    store.record_job_claim(&claim_a).await?;
    store.record_job_claim(&claim_b).await?;
    fs::write(
        demo_root.join("job_claim_provider_a.json"),
        serde_json::to_vec_pretty(&claim_a)?,
    )?;
    fs::write(
        demo_root.join("job_claim_provider_b.json"),
        serde_json::to_vec_pretty(&claim_b)?,
    )?;

    let mut assignment = JobAssignment {
        job_id,
        assigned_provider_node_id: "provider-a".to_string(),
        assigner_node_id: "enterprise-1".to_string(),
        assigner_ed25519_public_key_base64: enterprise_public_key,
        assignment_reason: "demo_preferred_provider".to_string(),
        assigned_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    assignment.signature = sign_job_assignment(&assignment, &enterprise_signing_key)?;
    store.record_job_assignment(&assignment).await?;
    fs::write(
        demo_root.join("job_assignment.json"),
        serde_json::to_vec_pretty(&assignment)?,
    )?;

    let gpu_env = ScopedGpuEnvironment::set("NVIDIA A10G", "550.54.15", true, Some(24.0));
    let provider_a = ProviderConfig {
        provider_id: "provider-a".to_string(),
        signing_key_id: "provider-a-key".to_string(),
        signing_key_seed_base64: provider_a_seed,
        repo_root: repo_root.clone(),
        work_root: work_root.clone(),
    };
    let output = run_job(&job, &provider_a).await?;
    drop(gpu_env);

    let mut availability = ReceiptAvailability {
        job_id,
        provider_node_id: "provider-a".to_string(),
        provider_ed25519_public_key_base64: provider_a_public_key.clone(),
        execution_receipt_sha256: sha256_file(&output.execution_receipt_path)?,
        bundle_sha256: {
            let bundle: ReceiptBundle =
                serde_json::from_slice(&fs::read(&output.receipt_bundle_path)?)?;
            bundle.bundle_sha256
        },
        bundle_uri: format!("file://{}", output.evidence_dir.display()),
        announced_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    availability.signature = sign_receipt_availability(&availability, &provider_a_signing_key)?;
    store.record_receipt_availability(&availability).await?;
    fs::write(
        demo_root.join("receipt_availability.json"),
        serde_json::to_vec_pretty(&availability)?,
    )?;

    let verifier = VerifierConfig {
        verifier_id: "verifier-1".to_string(),
        signing_key_id: "verifier-1-key".to_string(),
        signing_key_seed_base64: verifier_seed,
    };
    let verification_output =
        verify_bundle(&output.evidence_dir, &provider_a_public_key, &verifier).await?;
    fs::copy(
        &verification_output.verification_receipt_path,
        demo_root.join("verification_receipt.json"),
    )?;

    let milestone_bundle: ReceiptBundle =
        serde_json::from_slice(&fs::read(&output.receipt_bundle_path)?)?;
    let mut milestone = MilestoneRecord {
        milestone_id: Uuid::now_v7(),
        job_id,
        job_type: job.job_type.clone(),
        title: "Shared settlement milestone".to_string(),
        summary: "Provider and verifier peers completed the shared private AI checkpoint."
            .to_string(),
        contributing_node_ids: vec![
            "enterprise-1".to_string(),
            "provider-a".to_string(),
            "verifier-1".to_string(),
        ],
        quality_metric_name: "quality_retention".to_string(),
        quality_metric_value: 0.96,
        evidence_bundle_sha256: milestone_bundle.bundle_sha256.clone(),
        verification_receipt_sha256_list: milestone_bundle.verification_receipt_sha256_list.clone(),
        published_by: "enterprise-1".to_string(),
        published_at: Utc::now().to_rfc3339(),
        signing_key_id: "enterprise-key".to_string(),
        signature: String::new(),
    };
    milestone.signature = sign_milestone_record(&milestone, &enterprise_signing_key)?;
    verify_milestone_record_signature(&milestone, &enterprise_signing_key.verifying_key())?;
    store.record_milestone(&milestone).await?;
    fs::write(
        demo_root.join("milestone_record.json"),
        serde_json::to_vec_pretty(&milestone)?,
    )?;

    let quorum_before_challenge = build_quorum_report(&store, job_id).await?;
    fs::write(
        demo_root.join("quorum_status.json"),
        serde_json::to_vec_pretty(&quorum_before_challenge)?,
    )?;

    let mut challenge = ChallengeRecord {
        challenge_id: Uuid::now_v7(),
        job_id,
        bundle_sha256: availability.bundle_sha256.clone(),
        opened_by: "enterprise-1".to_string(),
        opened_by_ed25519_public_key_base64: verifying_key_to_base64(
            &enterprise_signing_key.verifying_key(),
        ),
        reason_code: ChallengeReasonCode::ForbiddenJobTransition,
        reason_detail: "demo challenge gate".to_string(),
        opened_at: Utc::now().to_rfc3339(),
        status: ChallengeStatus::Open,
        resolved_by: None,
        resolved_by_ed25519_public_key_base64: None,
        resolved_at: None,
        resolution_note: None,
        signature: String::new(),
    };
    challenge.signature = sign_challenge_record(&challenge, &enterprise_signing_key)?;
    store.record_challenge_record(&challenge).await?;
    fs::write(
        demo_root.join("challenge_open.json"),
        serde_json::to_vec_pretty(&challenge)?,
    )?;

    let settlement_blocked = build_settlement_report(&store, job_id).await?;
    fs::write(
        demo_root.join("settlement_status_blocked.json"),
        serde_json::to_vec_pretty(&settlement_blocked)?,
    )?;

    challenge.status = ChallengeStatus::ResolvedRejected;
    challenge.resolved_by = Some("verifier-1".to_string());
    challenge.resolved_by_ed25519_public_key_base64 = Some(verifying_key_to_base64(
        &verifier_signing_key.verifying_key(),
    ));
    challenge.resolved_at = Some(Utc::now().to_rfc3339());
    challenge.resolution_note = Some("demo challenge rejected".to_string());
    challenge.signature = sign_challenge_record(&challenge, &verifier_signing_key)?;
    store.record_challenge_record(&challenge).await?;
    fs::write(
        demo_root.join("challenge_resolved.json"),
        serde_json::to_vec_pretty(&challenge)?,
    )?;

    let provider_capabilities = store.list_provider_capabilities().await?;
    let claims = store.list_job_claims().await?;
    let assignments = store.list_job_assignments().await?;
    let availability_records = store.list_receipt_availability().await?;
    let provider_status = build_provider_network_status(
        &provider_capabilities,
        &claims,
        &assignments,
        &availability_records,
    );
    fs::write(
        demo_root.join("provider_status.json"),
        serde_json::to_vec_pretty(&provider_status)?,
    )?;

    let settlement = build_settlement_report(&store, job_id).await?;
    fs::write(
        demo_root.join("settlement_status.json"),
        serde_json::to_vec_pretty(&settlement)?,
    )?;

    let participant_status_json = build_participant_status_snapshot(&store, job_id).await?;
    fs::write(
        demo_root.join("participant_status.json"),
        serde_json::to_vec_pretty(&participant_status_json)?,
    )?;

    let announcement_record = store.load_job_announcement(&job_id.to_string()).await?;
    let job_spec_record = store.load_job_spec(&job_id.to_string()).await?;
    let claim_records = store.load_job_claims_by_job(&job_id.to_string()).await?;
    let assignment_record = store.load_job_assignment(&job_id.to_string()).await?;
    let receipt_records = store
        .load_receipt_availability_by_job(&job_id.to_string())
        .await?;
    let verification_receipts = store
        .load_verification_receipts_by_job(&job_id.to_string())
        .await?;
    let challenge_records = store
        .load_challenge_records_by_job(&job_id.to_string())
        .await?;
    let job_status_json = serde_json::json!({
        "job_id": job_id,
        "job_spec": job_spec_record,
        "job_announcement": announcement_record,
        "claims": claim_records,
        "assignment": assignment_record,
        "receipt_availability": receipt_records,
        "verification_receipts": verification_receipts,
        "quorum": quorum_before_challenge,
        "challenges": challenge_records,
        "settlement": settlement
    });
    fs::write(
        demo_root.join("job_status.json"),
        serde_json::to_vec_pretty(&job_status_json)?,
    )?;

    let provider_b_executed = store
        .load_receipt_availability(&job_id.to_string(), "provider-b")
        .await?
        .is_some();
    let ready = settlement.settlement_ready;
    let summary = LocalSettlementDemoSummary {
        ready,
        work_root: work_root.display().to_string(),
        repo_root: repo_root.display().to_string(),
        kept_artifacts: keep_artifacts || work_root.starts_with(env::temp_dir()),
        workflow: "local_settlement".to_string(),
        identity_files: serde_json::json!({}),
        job_id,
        provider_a_executed: true,
        provider_b_executed,
        quorum_status: format!("{:?}", quorum_before_challenge.status),
        settlement_ready: settlement.settlement_ready,
        lifecycle_state: format!("{:?}", settlement.lifecycle_state),
        files: serde_json::json!({
            "job_spec": job_spec_path,
            "evidence_dir": output.evidence_dir,
            "verification_receipt_path": verification_output.verification_receipt_path,
            "milestone_record": demo_root.join("milestone_record.json"),
            "participant_status": demo_root.join("participant_status.json"),
            "job_status": demo_root.join("job_status.json"),
            "provider_status": demo_root.join("provider_status.json"),
            "quorum_status": demo_root.join("quorum_status.json"),
            "settlement_status": demo_root.join("settlement_status.json")
        }),
    };
    fs::write(
        demo_root.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    Ok(summary)
}

async fn run_contributor_gpu_peer_demo(
    work_root: Option<PathBuf>,
    repo_root: Option<PathBuf>,
    keep_artifacts: bool,
    output: Option<PathBuf>,
) -> Result<ContributorGpuPeerDemoSummary> {
    let work_root = work_root
        .unwrap_or_else(|| env::temp_dir().join(format!("osciris-contributor-{}", Uuid::now_v7())));
    fs::create_dir_all(&work_root)?;
    let repo_root = repo_root.unwrap_or_else(|| work_root.clone());
    let identity_root = work_root.join("demo-identities");
    fs::create_dir_all(&identity_root)?;

    let enterprise_identity = generate_identity(
        "enterprise-1".to_string(),
        "enterprise".to_string(),
        "Enterprise 1".to_string(),
        Some(identity_root.join("enterprise")),
        None,
        None,
        vec![],
    )
    .await?;
    let provider_identity = generate_identity(
        "provider-a".to_string(),
        "provider".to_string(),
        "Provider A".to_string(),
        Some(identity_root.join("provider-a")),
        None,
        None,
        vec![],
    )
    .await?;
    let verifier_identity = generate_identity(
        "verifier-1".to_string(),
        "verifier".to_string(),
        "Verifier 1".to_string(),
        Some(identity_root.join("verifier-1")),
        None,
        None,
        vec![],
    )
    .await?;

    fs::write(
        identity_root.join("enterprise.json"),
        serde_json::to_vec_pretty(&enterprise_identity)?,
    )?;
    fs::write(
        identity_root.join("provider-a.json"),
        serde_json::to_vec_pretty(&provider_identity)?,
    )?;
    fs::write(
        identity_root.join("verifier-1.json"),
        serde_json::to_vec_pretty(&verifier_identity)?,
    )?;

    let settlement = run_local_settlement_demo(
        Some(work_root.clone()),
        Some(repo_root.clone()),
        keep_artifacts,
    )
    .await?;
    let manifest = serde_json::json!({
        "install": "cargo install --path crates/osciris-cli",
        "identity": {
            "enterprise": {
                "node_id": enterprise_identity.node_id,
                "peer_id": enterprise_identity.peer_id,
                "identity_json": identity_root.join("enterprise.json")
            },
            "provider": {
                "node_id": provider_identity.node_id,
                "peer_id": provider_identity.peer_id,
                "identity_json": identity_root.join("provider-a.json")
            },
            "verifier": {
                "node_id": verifier_identity.node_id,
                "peer_id": verifier_identity.peer_id,
                "identity_json": identity_root.join("verifier-1.json")
            }
        },
        "workflow": [
            "create-provider-capability",
            "create-job-claim",
            "run-provider",
            "create-receipt-availability",
            "run-verifier",
            "publish-milestone",
            "participant-status"
        ]
    });

    let summary = ContributorGpuPeerDemoSummary {
        ready: settlement.ready,
        work_root: work_root.display().to_string(),
        repo_root: repo_root.display().to_string(),
        contributor_manifest: manifest,
        settlement,
    };
    if let Some(output) = output.as_ref() {
        fs::write(output, serde_json::to_vec_pretty(&summary)?)?;
    }
    Ok(summary)
}

async fn generate_identity(
    node_id: String,
    role: String,
    display_name: String,
    work_root: Option<PathBuf>,
    existing_seed_base64: Option<String>,
    evm_private_key_hex: Option<String>,
    bootstrap_peers: Vec<String>,
) -> Result<GeneratedIdentity> {
    let role = parse_node_role(&role)?;
    // Reuse a caller-supplied seed when given (deterministically restoring the same identity,
    // e.g. after a work-root was wiped); otherwise mint a fresh random seed.
    let signing_key = match existing_seed_base64 {
        Some(seed) => load_signing_key_from_base64_seed(seed.trim())?,
        None => {
            let mut seed_bytes = [0_u8; 32];
            OsRng.fill_bytes(&mut seed_bytes);
            ed25519_dalek::SigningKey::from_bytes(&seed_bytes)
        }
    };
    let signing_key_seed_base64 = BASE64.encode(signing_key.to_bytes());
    let ed25519_public_key_base64 = verifying_key_to_base64(&signing_key.verifying_key());
    let peer_id = peer_id_from_signing_seed(&signing_key_seed_base64)?;

    let (evm_address, evm_private_key_hex) = if let Some(private_key_hex) = evm_private_key_hex {
        let signer = private_key_signer_from_hex(&private_key_hex)?;
        (
            Some(format!("{:#x}", signer.address())),
            Some(format_private_key_hex(&private_key_hex)),
        )
    } else {
        (None, None)
    };

    let node_identity = NodeIdentity {
        node_id: node_id.clone(),
        role: role.clone(),
        ed25519_public_key_base64: ed25519_public_key_base64.clone(),
        evm_address: evm_address.clone(),
        display_name: display_name.clone(),
        bootstrap_peers: bootstrap_peers.clone(),
        created_at: Utc::now().to_rfc3339(),
    };

    if let Some(work_root) = work_root.as_ref() {
        let store = ProtocolStore::open(&work_root.join(".osciris")).await?;
        store.record_node_identity(&node_identity).await?;
    }

    Ok(GeneratedIdentity {
        node_id: node_id.clone(),
        role,
        display_name,
        signing_key_seed_base64: signing_key_seed_base64.clone(),
        ed25519_public_key_base64,
        peer_id,
        evm_address,
        evm_private_key_hex,
        bootstrap_peers,
        node_identity,
        suggested_commands: serde_json::json!({
            "status": "osciris-node node status --work-root /path/to/work-root",
            "serve": format!(
                "osciris-node network serve --work-root /path/to/work-root --signing-key-seed-base64 '{}' --listen-addr /ip4/0.0.0.0/tcp/4101",
                signing_key_seed_base64
            )
        }),
    })
}

fn inspect_command(name: &str, version_args: &[&str]) -> CommandAvailability {
    let path = std::env::var_os("PATH")
        .and_then(|_| std::process::Command::new("which").arg(name).output().ok())
        .and_then(|output| {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (!text.is_empty()).then_some(text)
            } else {
                None
            }
        });
    let available = path.is_some();
    let version = if available {
        std::process::Command::new(name)
            .args(version_args)
            .output()
            .ok()
            .map(|output| {
                let text = if output.stdout.is_empty() {
                    String::from_utf8_lossy(&output.stderr).to_string()
                } else {
                    String::from_utf8_lossy(&output.stdout).to_string()
                };
                text.trim().to_string()
            })
            .filter(|text| !text.is_empty())
    } else {
        None
    };
    CommandAvailability {
        available,
        path,
        version,
    }
}

fn run_dsp_doctor(repo_root: &Path) -> Result<DspDoctorStatus> {
    let output = std::process::Command::new("uv")
        .arg("run")
        .arg("osciris")
        .arg("doctor")
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to run DSP doctor in {}", repo_root.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Ok(DspDoctorStatus {
        invoked: true,
        ok: output.status.success(),
        exit_code: output.status.code(),
        output_json: serde_json::from_str(&stdout).ok(),
        stderr: (!stderr.is_empty()).then_some(stderr),
    })
}

fn seed_base64(byte: u8) -> String {
    BASE64.encode([byte; 32])
}

fn parse_job_type(raw: &str) -> Result<JobType> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "llm_lora_economics" | "llm-lora-economics" => Ok(JobType::LlmLoraEconomics),
        "inference_economics" | "inference-economics" => Ok(JobType::InferenceEconomics),
        "production_proof" | "production-proof" => Ok(JobType::ProductionProof),
        other => bail!(
            "unsupported job type {other:?}; expected llm_lora_economics, inference_economics, or production_proof"
        ),
    }
}

fn load_signing_key_from_seed_source(
    signing_key_seed_base64: Option<String>,
    signing_key_seed_file: Option<PathBuf>,
) -> Result<ed25519_dalek::SigningKey> {
    let seed = load_signing_seed_from_source(signing_key_seed_base64, signing_key_seed_file)?;
    load_signing_key_from_base64_seed(seed.trim()).context("invalid signing key seed")
}

fn load_signing_seed_from_source(
    signing_key_seed_base64: Option<String>,
    signing_key_seed_file: Option<PathBuf>,
) -> Result<String> {
    let seed = match (signing_key_seed_base64, signing_key_seed_file) {
        (Some(seed), None) => seed,
        (None, Some(path)) => {
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?
        }
        (Some(_), Some(_)) => bail!(
            "provide only one signing seed source: --signing-key-seed-base64 or --signing-key-seed-file"
        ),
        (None, None) => bail!(
            "missing signing seed source: provide --signing-key-seed-base64 or --signing-key-seed-file"
        ),
    };
    Ok(seed.trim().to_string())
}

fn default_command_for_job_type(job_type: &JobType, command: &str) -> String {
    if command != "uv run osciris llm-lora-economics" {
        return command.to_string();
    }

    match job_type {
        JobType::LlmLoraEconomics => command.to_string(),
        JobType::InferenceEconomics => "uv run osciris inference-economics".to_string(),
        JobType::ProductionProof => "uv run osciris production-proof".to_string(),
    }
}

fn structured_submit_job_args(job_type: &JobType, options: SubmitJobArgOptions) -> Vec<String> {
    match job_type {
        JobType::LlmLoraEconomics => vec![
            "--samples".to_string(),
            options.samples.to_string(),
            "--eval-samples".to_string(),
            options.eval_samples.to_string(),
            "--seed".to_string(),
            options.seed.to_string(),
            "--max-steps".to_string(),
            options.max_steps.to_string(),
        ],
        JobType::InferenceEconomics => vec![
            "--samples".to_string(),
            options.samples.to_string(),
            "--seeds".to_string(),
            options.seeds.unwrap_or_else(|| options.seed.to_string()),
            "--backend".to_string(),
            options.backend,
            "--timeout".to_string(),
            options.timeout.to_string(),
        ],
        JobType::ProductionProof => vec![
            "--samples".to_string(),
            options.samples.to_string(),
            "--seeds".to_string(),
            options.seeds.unwrap_or_else(|| options.seed.to_string()),
        ],
    }
}

fn release_object_for_job_type(job_type: &JobType) -> &'static str {
    match job_type {
        JobType::LlmLoraEconomics => "model",
        JobType::InferenceEconomics => "inference_output",
        JobType::ProductionProof => "evidence_bundle",
    }
}

fn evidence_profile_for_job_type(job_type: &JobType) -> &'static str {
    match job_type {
        JobType::LlmLoraEconomics => "phase1_llm_lora_economics",
        JobType::InferenceEconomics => "phase1_inference_economics",
        JobType::ProductionProof => "phase1_production_proof",
    }
}

fn private_key_signer_from_hex(raw: &str) -> Result<PrivateKeySigner> {
    let normalized = format_private_key_hex(raw);
    normalized
        .parse::<PrivateKeySigner>()
        .context("invalid evm_private_key_hex")
}

fn format_private_key_hex(raw: &str) -> String {
    if raw.starts_with("0x") {
        raw.to_string()
    } else {
        format!("0x{raw}")
    }
}

fn mock_demo_job(job_id: Uuid) -> JobSpec {
    let script = r#"import json, pathlib, sys; output_dir = pathlib.Path(sys.argv[sys.argv.index("--output-dir") + 1]); output_dir.mkdir(parents=True, exist_ok=True); (output_dir / "llm_lora_economics.json").write_text(json.dumps({"kind": "llm_lora_economics_benchmark", "config": {"model_id": "mock-7b"}, "aggregate": {"quality_retention": 0.96}, "runs": [{"mode": "raw_lora", "eval_loss": 1.2}, {"mode": "dsp_prepared_lora", "eval_loss": 1.25}]}, indent=2), encoding="utf-8"); (output_dir / "llm_lora_economics.csv").write_text("mode,quality\nraw_lora,1.0\ndsp_prepared_lora,0.96\n", encoding="utf-8"); print("osciris demo workload complete")"#;
    JobSpec {
        job_id,
        job_type: JobType::LlmLoraEconomics,
        dataset: Some("enterprise_synthetic".to_string()),
        model_id: Some("mock-7b".to_string()),
        command: "python3".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        privacy_policy: PrivacyPolicy {
            privacy_mode: PrivacyMode::DspPrepared,
            release_object: "model".to_string(),
            formal_dp_claim: false,
            sensitive_field_policy: "configured_guard".to_string(),
            evidence_profile: "developer_demo_local_settlement".to_string(),
        },
        required_verifier_count: 1,
        challenge_window_seconds: 3600,
        payment_token: "USDC_TEST".to_string(),
        escrow_amount_atomic: "1000000".to_string(),
        created_at: Utc::now().to_rfc3339(),
    }
}

fn signed_provider_capability(
    node_id: &str,
    public_key: &str,
    signing_key: &ed25519_dalek::SigningKey,
    host_class: &str,
) -> Result<ProviderCapability> {
    let mut capability = ProviderCapability {
        node_id: node_id.to_string(),
        ed25519_public_key_base64: public_key.to_string(),
        host_class: host_class.to_string(),
        gpu_model: "NVIDIA A10G".to_string(),
        gpu_count: 1,
        vram_gb: 24.0,
        cuda_available: true,
        mps_available: false,
        supported_job_types: vec![JobType::LlmLoraEconomics],
        supported_runtimes: vec!["python3".to_string()],
        pricing_hint: Some("demo".to_string()),
        current_load: 0.0,
        active_job_count: 0,
        status: NodeStatus::OnlineIdle,
        updated_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    capability.signature = sign_provider_capability(&capability, signing_key)?;
    Ok(capability)
}

#[allow(clippy::too_many_arguments)]
fn create_signed_provider_capability(
    node_id: &str,
    signing_key: &ed25519_dalek::SigningKey,
    host_class: &str,
    gpu_model: &str,
    gpu_count: u32,
    vram_gb: f64,
    cuda_available: bool,
    mps_available: bool,
    supported_job_types: &[String],
    supported_runtimes: &[String],
    pricing_hint: Option<String>,
    current_load: f64,
    active_job_count: u32,
) -> Result<ProviderCapability> {
    if gpu_count == 0 {
        bail!("gpu_count must be greater than zero");
    }
    if vram_gb <= 0.0 {
        bail!("vram_gb must be greater than zero");
    }
    if current_load < 0.0 {
        bail!("current_load must be zero or greater");
    }

    let job_types = if supported_job_types.is_empty() {
        vec![JobType::LlmLoraEconomics]
    } else {
        supported_job_types
            .iter()
            .map(|job_type| parse_job_type(job_type))
            .collect::<Result<Vec<_>>>()?
    };
    let runtimes = if supported_runtimes.is_empty() {
        vec!["python3".to_string()]
    } else {
        supported_runtimes.to_vec()
    };

    let mut capability = ProviderCapability {
        node_id: node_id.to_string(),
        ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
        host_class: host_class.to_string(),
        gpu_model: gpu_model.to_string(),
        gpu_count,
        vram_gb,
        cuda_available,
        mps_available,
        supported_job_types: job_types,
        supported_runtimes: runtimes,
        pricing_hint,
        current_load,
        active_job_count,
        status: NodeStatus::OnlineIdle,
        updated_at: Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    capability.signature = sign_provider_capability(&capability, signing_key)?;
    Ok(capability)
}

fn signed_job_claim(
    provider_id: &str,
    public_key: &str,
    signing_key: &ed25519_dalek::SigningKey,
    job_id: Uuid,
) -> Result<JobClaim> {
    signed_job_claim_with_note(
        provider_id,
        public_key,
        signing_key,
        job_id,
        Some("demo_claim".to_string()),
    )
}

fn signed_job_claim_with_note(
    provider_id: &str,
    public_key: &str,
    signing_key: &ed25519_dalek::SigningKey,
    job_id: Uuid,
    claim_note: Option<String>,
) -> Result<JobClaim> {
    let mut claim = JobClaim {
        job_id,
        provider_node_id: provider_id.to_string(),
        provider_ed25519_public_key_base64: public_key.to_string(),
        claimed_at: Utc::now().to_rfc3339(),
        claim_note,
        signature: String::new(),
    };
    claim.signature = sign_job_claim(&claim, signing_key)?;
    Ok(claim)
}

struct ScopedGpuEnvironment {
    previous_model: Option<String>,
    previous_driver: Option<String>,
    previous_cuda: Option<String>,
    previous_vram: Option<String>,
}

impl ScopedGpuEnvironment {
    fn set(gpu_model: &str, driver: &str, cuda_available: bool, vram_gb: Option<f64>) -> Self {
        let previous_model = env::var("OSCIRIS_GPU_MODEL").ok();
        let previous_driver = env::var("OSCIRIS_GPU_DRIVER").ok();
        let previous_cuda = env::var("OSCIRIS_CUDA_AVAILABLE").ok();
        let previous_vram = env::var("OSCIRIS_GPU_VRAM_GB").ok();
        env::set_var("OSCIRIS_GPU_MODEL", gpu_model);
        env::set_var("OSCIRIS_GPU_DRIVER", driver);
        env::set_var(
            "OSCIRIS_CUDA_AVAILABLE",
            if cuda_available { "true" } else { "false" },
        );
        if let Some(vram_gb) = vram_gb {
            env::set_var("OSCIRIS_GPU_VRAM_GB", vram_gb.to_string());
        } else {
            env::remove_var("OSCIRIS_GPU_VRAM_GB");
        }
        Self {
            previous_model,
            previous_driver,
            previous_cuda,
            previous_vram,
        }
    }
}

impl Drop for ScopedGpuEnvironment {
    fn drop(&mut self) {
        restore_env_var("OSCIRIS_GPU_MODEL", self.previous_model.take());
        restore_env_var("OSCIRIS_GPU_DRIVER", self.previous_driver.take());
        restore_env_var("OSCIRIS_CUDA_AVAILABLE", self.previous_cuda.take());
        restore_env_var("OSCIRIS_GPU_VRAM_GB", self.previous_vram.take());
    }
}

fn restore_env_var(name: &str, value: Option<String>) {
    if let Some(value) = value {
        env::set_var(name, value);
    } else {
        env::remove_var(name);
    }
}

fn parse_challenge_reason_code(value: &str) -> Result<ChallengeReasonCode> {
    match value {
        "artifact_hash_mismatch" => Ok(ChallengeReasonCode::ArtifactHashMismatch),
        "missing_required_metric" => Ok(ChallengeReasonCode::MissingRequiredMetric),
        "invalid_provider_signature" => Ok(ChallengeReasonCode::InvalidProviderSignature),
        "invalid_verifier_signature" => Ok(ChallengeReasonCode::InvalidVerifierSignature),
        "duplicate_receipt_submission" => Ok(ChallengeReasonCode::DuplicateReceiptSubmission),
        "forbidden_job_transition" => Ok(ChallengeReasonCode::ForbiddenJobTransition),
        other => bail!(
            "unsupported challenge reason {other}; expected one of artifact_hash_mismatch, missing_required_metric, invalid_provider_signature, invalid_verifier_signature, duplicate_receipt_submission, forbidden_job_transition"
        ),
    }
}

fn parse_challenge_resolution(value: &str) -> Result<ChallengeStatus> {
    match value {
        "accepted" => Ok(ChallengeStatus::ResolvedAccepted),
        "rejected" => Ok(ChallengeStatus::ResolvedRejected),
        other => bail!("unsupported challenge resolution {other}; expected accepted or rejected"),
    }
}

fn verify_availability_signature(availability: &ReceiptAvailability) -> Result<()> {
    let verifying_key =
        verifying_key_from_base64(&availability.provider_ed25519_public_key_base64)?;
    verify_receipt_availability_signature(availability, &verifying_key)?;
    Ok(())
}

fn local_path_from_bundle_uri(uri: &str) -> Result<PathBuf> {
    if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("s3://") {
        bail!("bundle URI {uri:?} requires remote transfer support that is not implemented yet");
    }
    let path = uri
        .strip_prefix("file://localhost")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    Ok(PathBuf::from(path))
}

fn copy_dir_recursive_replace(source: &Path, destination: &Path) -> Result<()> {
    if !source.is_dir() {
        bail!(
            "source evidence directory does not exist: {}",
            source.display()
        );
    }
    if destination.exists() {
        fs::remove_dir_all(destination)?;
    }
    fs::create_dir_all(destination)?;
    copy_dir_recursive(source, destination)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)?;
        }
    }
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

fn load_verification_receipts(evidence_dir: &Path) -> Result<Vec<VerificationReceipt>> {
    let verification_dir = evidence_dir.join("verification_receipts");
    if !verification_dir.exists() {
        return Ok(vec![]);
    }
    let mut receipts: Vec<VerificationReceipt> = vec![];
    for entry in std::fs::read_dir(&verification_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            receipts.push(serde_json::from_slice(&std::fs::read(entry.path())?)?);
        }
    }
    receipts.sort_by(|left, right| left.verifier_id.cmp(&right.verifier_id));
    Ok(receipts)
}

async fn record_verified_verification_receipt_announcement(
    store: &ProtocolStore,
    announcement: &VerificationReceiptAnnouncement,
) -> Result<VerificationReceipt> {
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
        .await?;
    Ok(announcement.verification_receipt.clone())
}

fn resolve_provider_address(
    execution_receipt: &ExecutionReceipt,
    provider_address: Option<&str>,
) -> Result<alloy::primitives::Address> {
    if let Some(provider_address) = provider_address {
        osciris_chain::parse_address(provider_address, "provider_address")
    } else {
        provider_address_from_id(&execution_receipt.provider_id)
    }
}

fn resolve_verifier_addresses(
    verification_receipts: &[VerificationReceipt],
    verifier_addresses: &[String],
) -> Result<Vec<alloy::primitives::Address>> {
    if verifier_addresses.is_empty() {
        if verification_receipts.is_empty() {
            bail!("at least one verification receipt is required");
        }
        return verification_receipts
            .iter()
            .map(|receipt| verifier_address_from_id(&receipt.verifier_id))
            .collect::<Result<Vec<_>>>();
    }
    if verifier_addresses.len() != verification_receipts.len() {
        bail!(
            "verifier address count {} does not match verification receipt count {}",
            verifier_addresses.len(),
            verification_receipts.len()
        );
    }
    verifier_addresses
        .iter()
        .map(|address| osciris_chain::parse_address(address, "verifier_address"))
        .collect()
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn parse_node_role(raw: &str) -> Result<NodeRole> {
    serde_json::from_value(serde_json::Value::String(raw.to_string()))
        .with_context(|| format!("unsupported node role {raw:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        sign_verification_receipt, verify_provider_capability_signature, ExecutionStatus,
        GpuMetadata, VerificationChecks, VerificationReceipt, VerificationStatus,
    };

    fn temp_work_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{name}-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn execution_receipt(provider_id: &str) -> ExecutionReceipt {
        ExecutionReceipt {
            receipt_id: Uuid::nil(),
            job_id: Uuid::nil(),
            provider_id: provider_id.to_string(),
            job_type: JobType::LlmLoraEconomics,
            status: ExecutionStatus::Completed,
            command_exit_code: 0,
            started_at: "2026-06-04T00:00:00Z".to_string(),
            finished_at: "2026-06-04T00:00:01Z".to_string(),
            wall_clock_seconds: 1.0,
            stdout_sha256: "00".repeat(32),
            stderr_sha256: "11".repeat(32),
            artifact_root_sha256: "22".repeat(32),
            artifact_manifests: vec![],
            metrics_path: "metrics.json".to_string(),
            gpu_metadata: GpuMetadata {
                gpu_model: "mock".to_string(),
                driver: "mock".to_string(),
                cuda_available: false,
                vram_gb: None,
            },
            signature: "signature".to_string(),
            signing_key_id: "provider-key".to_string(),
        }
    }

    fn verification_receipt(verifier_id: &str) -> VerificationReceipt {
        VerificationReceipt {
            verification_receipt_id: Uuid::nil(),
            receipt_id: Uuid::nil(),
            job_id: Uuid::nil(),
            verifier_id: verifier_id.to_string(),
            verification_status: VerificationStatus::Accepted,
            verified_at: "2026-06-04T00:00:02Z".to_string(),
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
            bundle_sha256: "11".repeat(32),
            signature: "signature".to_string(),
            signing_key_id: "verifier-key".to_string(),
        }
    }

    fn signed_verification_announcement(verifier_id: &str) -> VerificationReceiptAnnouncement {
        let signing_key =
            load_signing_key_from_base64_seed("CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=")
                .unwrap();
        let verifying_key = signing_key.verifying_key();
        let mut receipt = verification_receipt(verifier_id);
        receipt.signature = sign_verification_receipt(&receipt, &signing_key).unwrap();
        VerificationReceiptAnnouncement {
            verifier_node_id: verifier_id.to_string(),
            verifier_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            verification_receipt: receipt,
        }
    }

    #[test]
    fn explicit_provider_address_allows_non_address_provider_id() {
        let address = resolve_provider_address(
            &execution_receipt("provider-a"),
            Some("0x1111111111111111111111111111111111111111"),
        )
        .unwrap();
        assert_eq!(
            address,
            osciris_chain::parse_address("0x1111111111111111111111111111111111111111", "expected")
                .unwrap()
        );
    }

    #[test]
    fn provider_address_falls_back_to_address_provider_id() {
        let address = resolve_provider_address(
            &execution_receipt("0x2222222222222222222222222222222222222222"),
            None,
        )
        .unwrap();
        assert_eq!(
            address,
            osciris_chain::parse_address("0x2222222222222222222222222222222222222222", "expected")
                .unwrap()
        );
    }

    #[test]
    fn explicit_verifier_addresses_allow_non_address_verifier_ids() {
        let receipts = vec![verification_receipt("verifier-a")];
        let addresses = resolve_verifier_addresses(
            &receipts,
            &["0x3333333333333333333333333333333333333333".to_string()],
        )
        .unwrap();
        assert_eq!(addresses.len(), 1);
    }

    #[test]
    fn verifier_address_count_must_match_receipt_count() {
        let receipts = vec![verification_receipt("verifier-a")];
        let err = resolve_verifier_addresses(&receipts, &[]).unwrap_err();
        assert!(err.to_string().contains("invalid address"));

        let err = resolve_verifier_addresses(
            &receipts,
            &[
                "0x3333333333333333333333333333333333333333".to_string(),
                "0x4444444444444444444444444444444444444444".to_string(),
            ],
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn signed_verification_receipt_import_persists_receipt() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let work_root = temp_work_root("osciris-verification-import");
        let store = runtime
            .block_on(ProtocolStore::open(&work_root.join(".osciris")))
            .unwrap();
        let announcement = signed_verification_announcement("verifier-a");

        let imported = runtime
            .block_on(record_verified_verification_receipt_announcement(
                &store,
                &announcement,
            ))
            .unwrap();
        assert_eq!(imported.verifier_id, "verifier-a");

        let receipts = runtime
            .block_on(store.load_verification_receipts_by_verifier("verifier-a"))
            .unwrap();
        assert_eq!(receipts, vec![announcement.verification_receipt]);
        std::fs::remove_dir_all(work_root).unwrap();
    }

    #[test]
    fn signed_verification_receipt_import_rejects_tampering() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let work_root = temp_work_root("osciris-verification-import-tamper");
        let store = runtime
            .block_on(ProtocolStore::open(&work_root.join(".osciris")))
            .unwrap();
        let mut announcement = signed_verification_announcement("verifier-a");
        announcement.verification_receipt.bundle_sha256 = "ff".repeat(32);

        let error = runtime
            .block_on(record_verified_verification_receipt_announcement(
                &store,
                &announcement,
            ))
            .unwrap_err();
        assert!(error.to_string().contains("signature verification"));

        let receipts = runtime
            .block_on(store.load_verification_receipts_by_verifier("verifier-a"))
            .unwrap();
        assert!(receipts.is_empty());
        std::fs::remove_dir_all(work_root).unwrap();
    }

    #[test]
    fn doctor_reports_protocol_readiness() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let work_root = temp_work_root("osciris-doctor");
        let report = runtime
            .block_on(run_doctor(None, Some(work_root.clone())))
            .unwrap();
        assert!(report.ready);
        assert!(report.work_root_writable);
        assert!(report.protocol_store_ready);
        std::fs::remove_dir_all(work_root).unwrap();
    }

    #[test]
    fn local_settlement_demo_reaches_settlement_ready() {
        if !inspect_command("python3", &["--version"]).available {
            return;
        }

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let work_root = temp_work_root("osciris-demo");
        let summary = runtime
            .block_on(run_local_settlement_demo(
                Some(work_root.clone()),
                Some(work_root.clone()),
                true,
            ))
            .unwrap();
        assert!(summary.ready);
        assert!(summary.provider_a_executed);
        assert!(!summary.provider_b_executed);
        assert!(summary.settlement_ready);
        std::fs::remove_dir_all(work_root).unwrap();
    }

    #[test]
    fn create_provider_capability_signs_declared_hardware() {
        let signing_key =
            load_signing_key_from_base64_seed("CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk=")
                .unwrap();
        let capability = create_signed_provider_capability(
            "provider-a",
            &signing_key,
            "aws_g5_xlarge",
            "NVIDIA A10G",
            1,
            24.0,
            true,
            false,
            &[
                "llm_lora_economics".to_string(),
                "inference_economics".to_string(),
            ],
            &["python3".to_string()],
            Some("aws-g5-baseline".to_string()),
            0.25,
            1,
        )
        .unwrap();

        assert_eq!(capability.node_id, "provider-a");
        assert_eq!(capability.gpu_model, "NVIDIA A10G");
        assert_eq!(capability.supported_job_types.len(), 2);
        assert_eq!(
            capability.ed25519_public_key_base64,
            verifying_key_to_base64(&signing_key.verifying_key())
        );
        verify_provider_capability_signature(&capability, &signing_key.verifying_key()).unwrap();
    }

    #[test]
    fn identity_generate_persists_node_identity() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let work_root = temp_work_root("osciris-identity-generate");
        let generated = runtime
            .block_on(generate_identity(
                "provider-a".to_string(),
                "provider".to_string(),
                "Provider A".to_string(),
                Some(work_root.clone()),
                None,
                None,
                vec!["/ip4/127.0.0.1/tcp/4101".to_string()],
            ))
            .unwrap();

        assert_eq!(generated.node_id, "provider-a");
        assert!(!generated.signing_key_seed_base64.is_empty());
        assert!(!generated.peer_id.is_empty());

        let store = runtime
            .block_on(ProtocolStore::open(&work_root.join(".osciris")))
            .unwrap();
        let stored = runtime.block_on(store.load_node_identity()).unwrap();
        assert_eq!(stored, Some(generated.node_identity.clone()));
        std::fs::remove_dir_all(work_root).unwrap();
    }
}
