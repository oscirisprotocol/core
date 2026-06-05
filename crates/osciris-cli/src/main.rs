use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::Compression;
use osciris_chain::{
    private_key_from_env, provider_address_from_id, verifier_address_from_id, ChainConfig,
    OscirisChain, RegisterIdentityRequest, SubmitBundleRequest,
};
use osciris_core::{
    bundle_hash, load_signing_key_from_base64_seed, sha256_file, sign_challenge_record,
    sign_job_announcement, sign_job_assignment, sign_receipt_availability,
    verify_challenge_record_signature, verify_job_claim_signature,
    verify_receipt_availability_signature, verify_verification_receipt_signature,
    verifying_key_from_base64, verifying_key_to_base64, ChainSubmissionStatus, ChallengeReasonCode,
    ChallengeRecord, ChallengeStatus, ExecutionReceipt, JobAnnouncement, JobAssignment, JobClaim,
    JobSpec, JobType, NodeIdentity, NodeRole, PeerPresence, PrivacyMode, PrivacyPolicy,
    ProviderCapability, ReceiptAvailability, ReceiptBundle, VerificationReceipt,
    VerificationReceiptAnnouncement,
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
use serde::Serialize;
use tar::Builder;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "osciris-node")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    SubmitJob {
        #[arg(long, default_value = "enterprise_synthetic")]
        dataset: String,
        #[arg(long, default_value = "Qwen/Qwen2.5-7B-Instruct")]
        model_id: String,
        #[arg(long, default_value = "uv run osciris llm-lora-economics")]
        command: String,
        #[arg(long, default_value_t = 24)]
        samples: u32,
        #[arg(long, default_value_t = 8)]
        eval_samples: u32,
        #[arg(long, default_value_t = 11)]
        seed: u32,
        #[arg(long, default_value_t = 20)]
        max_steps: u32,
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
        signing_key_seed_base64: String,
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
        signing_key_seed_base64: String,
        #[arg(long, default_value = "/ip4/127.0.0.1/tcp/0")]
        listen_addr: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
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
        signing_key_seed_base64: String,
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
        signing_key_seed_base64: String,
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
        signing_key_seed_base64: String,
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
        signing_key_seed_base64: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        repo_root: PathBuf,
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let runtime = tokio::runtime::Runtime::new()?;
    match cli.command {
        Commands::SubmitJob {
            dataset,
            model_id,
            command,
            samples,
            eval_samples,
            seed,
            max_steps,
            required_verifier_count,
            challenge_window_seconds,
            payment_token,
            escrow_amount_atomic,
            output,
        } => {
            let job = JobSpec {
                job_id: Uuid::now_v7(),
                job_type: JobType::LlmLoraEconomics,
                dataset: Some(dataset),
                model_id: Some(model_id),
                command,
                args: vec![
                    "--samples".to_string(),
                    samples.to_string(),
                    "--eval-samples".to_string(),
                    eval_samples.to_string(),
                    "--seed".to_string(),
                    seed.to_string(),
                    "--max-steps".to_string(),
                    max_steps.to_string(),
                ],
                privacy_policy: PrivacyPolicy {
                    privacy_mode: PrivacyMode::DspPrepared,
                    release_object: "model".to_string(),
                    formal_dp_claim: false,
                    sensitive_field_policy: "configured_guard".to_string(),
                    evidence_profile: "phase1_llm_lora_economics".to_string(),
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
            repo_root,
            work_root,
        } => {
            let job: JobSpec = serde_json::from_slice(&std::fs::read(job_spec)?)?;
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
                listen_addr,
                bootstrap_peers,
                presence_interval_seconds,
                run_seconds,
            } => {
                let summary = runtime.block_on(serve_presence(&NetworkServeConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64,
                    listen_addr,
                    bootstrap_peers,
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
                required_capability,
                estimated_runtime_class,
            } => {
                let store = runtime.block_on(ProtocolStore::open(&work_root.join(".osciris")))?;
                let job: JobSpec = serde_json::from_slice(
                    &std::fs::read(&job_spec)
                        .with_context(|| format!("failed to read {}", job_spec.display()))?,
                )?;
                let signing_key = load_signing_key_from_base64_seed(&signing_key_seed_base64)?;
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
                let signing_key = load_signing_key_from_base64_seed(&signing_key_seed_base64)?;
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
                verifier_id,
                signing_key_id,
                listen_addr,
                bootstrap_peers,
                presence_interval_seconds,
                run_seconds,
                announce_seconds,
            } => {
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
                signing_key_id,
                repo_root,
                listen_addr,
                bootstrap_peers,
                presence_interval_seconds,
                run_seconds,
            } => {
                let summary = runtime.block_on(run_auto_provider(&AutoProviderConfig {
                    protocol_root: work_root.join(".osciris"),
                    signing_key_seed_base64,
                    signing_key_id,
                    repo_root,
                    work_root,
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
        sign_verification_receipt, ExecutionStatus, GpuMetadata, VerificationChecks,
        VerificationReceipt, VerificationStatus,
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
}
