use std::fs::File;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;
use clap::{Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::Compression;
use osciris_core::{JobSpec, JobType, PrivacyMode, PrivacyPolicy};
use osciris_node::store::ProtocolStore;
use osciris_node::{run_job, ProviderConfig};
use osciris_verifier::{verify_bundle, VerifierConfig};
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
    VerifyReceipt {
        #[arg(long)]
        evidence_dir: PathBuf,
        #[arg(long)]
        provider_public_key_base64: String,
        #[arg(long)]
        verifier_id: String,
        #[arg(long)]
        signing_key_id: String,
        #[arg(long)]
        signing_key_seed_base64: String,
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
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
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
        }
        Commands::RunProvider {
            job_spec,
            provider_id,
            signing_key_id,
            signing_key_seed_base64,
            repo_root,
            work_root,
        } => {
            let runtime = tokio::runtime::Runtime::new()?;
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
        Commands::VerifyReceipt {
            evidence_dir,
            provider_public_key_base64,
            verifier_id,
            signing_key_id,
            signing_key_seed_base64,
        } => {
            let runtime = tokio::runtime::Runtime::new()?;
            let verifier = VerifierConfig {
                verifier_id,
                signing_key_id,
                signing_key_seed_base64,
            };
            let output = runtime.block_on(verify_bundle(
                &evidence_dir,
                &provider_public_key_base64,
                &verifier,
            ))?;
            println!("{}", output.verification_receipt_path.display());
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
            let runtime = tokio::runtime::Runtime::new()?;
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
    }
    Ok(())
}
