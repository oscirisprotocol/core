use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use osciris_core::{
    bundle_hash, canonical_json_sha256, load_signing_key_from_base64_seed, sha256_file,
    sign_execution_receipt, single_chunk_manifest, BundleIndex, ChainSubmissionStatus,
    CommandMetadata, ExecutionReceipt, ExecutionStatus, GpuMetadata, JobSpec, JobType,
    ReceiptBundle,
};
use tokio::fs;
use tokio::process::Command;
use tracing::info;
use uuid::Uuid;
use walkdir::WalkDir;

pub mod network;
pub mod status;
pub mod store;

use store::ProtocolStore;

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider_id: String,
    pub signing_key_id: String,
    pub signing_key_seed_base64: String,
    pub repo_root: PathBuf,
    pub work_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RunJobOutput {
    pub evidence_dir: PathBuf,
    pub execution_receipt_path: PathBuf,
    pub receipt_bundle_path: PathBuf,
    pub metrics_path: PathBuf,
}

pub async fn run_job(job_spec: &JobSpec, provider: &ProviderConfig) -> Result<RunJobOutput> {
    let signing_key = load_signing_key_from_base64_seed(&provider.signing_key_seed_base64)
        .context("failed to load provider signing key")?;
    let started = Utc::now();
    let protocol_root = provider.work_root.join(".osciris");
    let store = ProtocolStore::open(&protocol_root).await?;
    let evidence_dir = protocol_root
        .join("evidence")
        .join(job_spec.job_id.to_string());
    let python_output_dir = evidence_dir.join("python-output");
    fs::create_dir_all(&python_output_dir).await?;
    store
        .upsert_job_spec(job_spec, "created", Some(&evidence_dir), None)
        .await?;

    let job_spec_path = evidence_dir.join("job_spec.json");
    let stdout_path = evidence_dir.join("stdout.log");
    let stderr_path = evidence_dir.join("stderr.log");
    let command_path = evidence_dir.join("command.json");
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
    let bundle_index_path = evidence_dir.join("bundle_index.json");

    fs::write(&job_spec_path, serde_json::to_vec_pretty(job_spec)?).await?;

    let argv = build_command_argv(job_spec, &python_output_dir)?;
    let program = argv
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("job command resolved to an empty argv"))?;

    info!("running {} for job {}", program, job_spec.job_id);
    let output = Command::new(&program)
        .args(&argv[1..])
        .current_dir(&provider.repo_root)
        .output()
        .await
        .with_context(|| format!("failed to execute {}", program))?;
    let finished = Utc::now();

    fs::write(&stdout_path, &output.stdout).await?;
    fs::write(&stderr_path, &output.stderr).await?;

    let command_metadata = CommandMetadata {
        command: job_spec.command.clone(),
        argv: argv.clone(),
        working_directory: provider.repo_root.display().to_string(),
        started_at: started.to_rfc3339(),
        finished_at: finished.to_rfc3339(),
        exit_code: output.status.code().unwrap_or(-1),
    };
    fs::write(&command_path, serde_json::to_vec_pretty(&command_metadata)?).await?;

    let metrics_path = expected_metrics_path(job_spec.job_type.clone(), &python_output_dir);
    let status = if output.status.success() {
        if !metrics_path.exists() {
            bail!(
                "job {} exited successfully but metrics file {} was not created",
                job_spec.job_id,
                metrics_path.display()
            );
        }
        ExecutionStatus::Completed
    } else {
        ExecutionStatus::Failed
    };

    let artifact_manifests = collect_artifacts(
        &evidence_dir,
        &[
            job_spec_path.clone(),
            command_path.clone(),
            stdout_path.clone(),
            stderr_path.clone(),
        ],
    )?;
    let artifact_root_sha256 = canonical_json_sha256(&artifact_manifests)?;
    let mut receipt = ExecutionReceipt {
        receipt_id: Uuid::now_v7(),
        job_id: job_spec.job_id,
        provider_id: provider.provider_id.clone(),
        job_type: job_spec.job_type.clone(),
        status,
        command_exit_code: output.status.code().unwrap_or(-1),
        started_at: started.to_rfc3339(),
        finished_at: finished.to_rfc3339(),
        wall_clock_seconds: (finished - started).num_milliseconds() as f64 / 1000.0,
        stdout_sha256: sha256_file(&stdout_path)?,
        stderr_sha256: sha256_file(&stderr_path)?,
        artifact_root_sha256,
        artifact_manifests,
        metrics_path: relative_to(&metrics_path, &evidence_dir)?,
        gpu_metadata: gpu_metadata_from_environment(),
        signature: String::new(),
        signing_key_id: provider.signing_key_id.clone(),
    };
    receipt.signature = sign_execution_receipt(&receipt, &signing_key)?;
    fs::write(
        &execution_receipt_path,
        serde_json::to_vec_pretty(&receipt)?,
    )
    .await?;
    store
        .record_execution_receipt(&receipt, &evidence_dir, &receipt.metrics_path)
        .await?;

    let bundle_index = BundleIndex {
        job_id: job_spec.job_id,
        artifacts: receipt.artifact_manifests.clone(),
        execution_receipt_path: "execution_receipt.json".to_string(),
        verification_receipt_paths: vec![],
    };
    fs::write(
        &bundle_index_path,
        serde_json::to_vec_pretty(&bundle_index)?,
    )
    .await?;

    let mut bundle = ReceiptBundle {
        bundle_id: Uuid::now_v7(),
        job_id: job_spec.job_id,
        job_spec_sha256: sha256_file(&job_spec_path)?,
        execution_receipt_sha256: sha256_file(&execution_receipt_path)?,
        verification_receipt_sha256_list: vec![],
        bundle_sha256: String::new(),
        artifact_index_path: "bundle_index.json".to_string(),
        chain_submission_status: ChainSubmissionStatus::Pending,
    };
    bundle.bundle_sha256 = bundle_hash(&bundle)?;
    fs::write(&receipt_bundle_path, serde_json::to_vec_pretty(&bundle)?).await?;
    store.record_receipt_bundle(&bundle).await?;

    Ok(RunJobOutput {
        evidence_dir,
        execution_receipt_path,
        receipt_bundle_path,
        metrics_path,
    })
}

fn build_command_argv(job_spec: &JobSpec, python_output_dir: &Path) -> Result<Vec<String>> {
    let mut argv = shell_words::split(&job_spec.command)
        .with_context(|| format!("failed to parse command string {:?}", job_spec.command))?;
    merge_structured_job_args(&mut argv, job_spec);
    ensure_option_value(
        &mut argv,
        "--output-dir",
        python_output_dir.display().to_string(),
    );

    Ok(argv)
}

fn merge_structured_job_args(argv: &mut Vec<String>, job_spec: &JobSpec) {
    let mut index = 0usize;
    while index < job_spec.args.len() {
        let arg = &job_spec.args[index];
        if arg.starts_with("--") {
            let next = job_spec.args.get(index + 1);
            if let Some(value) = next {
                ensure_option_value(argv, arg, value.clone());
                index += 2;
                continue;
            }
            ensure_flag(argv, arg);
            index += 1;
            continue;
        }

        if !argv.contains(arg) {
            argv.push(arg.clone());
        }
        index += 1;
    }

    if let Some(model_id) = &job_spec.model_id {
        ensure_option_value(argv, "--model-id", model_id.clone());
    }
}

fn ensure_option_value(argv: &mut Vec<String>, flag: &str, value: String) {
    if has_flag(argv, flag) {
        return;
    }

    argv.push(flag.to_string());
    argv.push(value);
}

fn ensure_flag(argv: &mut Vec<String>, flag: &str) {
    if !has_flag(argv, flag) {
        argv.push(flag.to_string());
    }
}

fn has_flag(argv: &[String], flag: &str) -> bool {
    argv.iter().any(|arg| arg == flag)
}

fn expected_metrics_path(job_type: JobType, output_dir: &Path) -> PathBuf {
    match job_type {
        JobType::LlmLoraEconomics => output_dir.join("llm_lora_economics.json"),
        JobType::ProductionProof => output_dir.join("production_proof_suite.json"),
    }
}

fn collect_artifacts(
    evidence_dir: &Path,
    required_files: &[PathBuf],
) -> Result<Vec<osciris_core::ArtifactManifest>> {
    let mut manifests = vec![];
    let mut seen = BTreeMap::new();
    for file in required_files {
        seen.insert(file.clone(), ());
    }
    for entry in WalkDir::new(evidence_dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            let path = entry.into_path();
            seen.insert(path, ());
        }
    }
    for path in seen.into_keys() {
        if path.file_name().is_some_and(|name| {
            name == "execution_receipt.json"
                || name == "receipt_bundle.json"
                || name == "bundle_index.json"
        }) {
            continue;
        }
        manifests.push(single_chunk_manifest(&path, evidence_dir)?);
    }
    manifests.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(manifests)
}

fn gpu_metadata_from_environment() -> GpuMetadata {
    GpuMetadata {
        gpu_model: std::env::var("OSCIRIS_GPU_MODEL").unwrap_or_else(|_| "unknown".to_string()),
        driver: std::env::var("OSCIRIS_GPU_DRIVER").unwrap_or_else(|_| "unknown".to_string()),
        cuda_available: std::env::var("OSCIRIS_CUDA_AVAILABLE")
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE"))
            .unwrap_or(false),
        vram_gb: std::env::var("OSCIRIS_GPU_VRAM_GB")
            .ok()
            .and_then(|raw| raw.parse::<f64>().ok()),
    }
}

fn relative_to(path: &Path, base: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(base)
        .with_context(|| format!("{} is not under {}", path.display(), base.display()))?
        .display()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{JobType, PrivacyMode, PrivacyPolicy};

    fn sample_job(command: &str, args: Vec<String>) -> JobSpec {
        JobSpec {
            job_id: Uuid::now_v7(),
            job_type: JobType::LlmLoraEconomics,
            dataset: Some("enterprise_synthetic".to_string()),
            model_id: Some("mock-model".to_string()),
            command: command.to_string(),
            args,
            privacy_policy: PrivacyPolicy {
                privacy_mode: PrivacyMode::DspPrepared,
                release_object: "model".to_string(),
                formal_dp_claim: false,
                sensitive_field_policy: "configured_guard".to_string(),
                evidence_profile: "phase1_test".to_string(),
            },
            required_verifier_count: 1,
            challenge_window_seconds: 3600,
            payment_token: "USDC_TEST".to_string(),
            escrow_amount_atomic: "1000000".to_string(),
            created_at: "2026-06-04T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn run_job_writes_evidence_and_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let provider = ProviderConfig {
            provider_id: "provider-1".to_string(),
            signing_key_id: "provider-key-1".to_string(),
            signing_key_seed_base64: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string(),
            repo_root: temp.path().to_path_buf(),
            work_root: temp.path().to_path_buf(),
        };
        let script = r#"import json, pathlib, sys; output_dir = pathlib.Path(sys.argv[sys.argv.index("--output-dir") + 1]); output_dir.mkdir(parents=True, exist_ok=True); (output_dir / "llm_lora_economics.json").write_text(json.dumps({"kind": "llm_lora_economics_benchmark", "config": {"model_id": "mock-model"}, "aggregate": {"quality_retention": 1.0}, "runs": [{"mode": "raw_lora"}, {"mode": "dsp_prepared_lora"}]}, indent=2), encoding="utf-8"); (output_dir / "llm_lora_economics.csv").write_text("mode,quality\nraw,1.0\n", encoding="utf-8"); print("mock benchmark complete")"#;
        let job = sample_job("python3", vec!["-c".to_string(), script.to_string()]);

        let output = run_job(&job, &provider).await.unwrap();
        assert!(output.execution_receipt_path.exists());
        assert!(output.receipt_bundle_path.exists());
        assert!(output.metrics_path.exists());

        let store = ProtocolStore::open(&temp.path().join(".osciris"))
            .await
            .unwrap();
        let jobs = store.list_jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "completed");
    }

    #[test]
    fn build_command_argv_preserves_explicit_command_overrides() {
        let job = sample_job(
            "uv run osciris llm-lora-economics --model-id Qwen/Qwen2.5-0.5B-Instruct --samples 6 --eval-samples 2 --max-steps 1",
            vec![
                "--samples".to_string(),
                "24".to_string(),
                "--eval-samples".to_string(),
                "8".to_string(),
                "--seed".to_string(),
                "11".to_string(),
                "--max-steps".to_string(),
                "20".to_string(),
            ],
        );

        let argv = build_command_argv(&job, Path::new("/tmp/output")).unwrap();
        let rendered = argv.join(" ");

        assert!(rendered.contains("--model-id Qwen/Qwen2.5-0.5B-Instruct"));
        assert!(rendered.contains("--samples 6"));
        assert!(rendered.contains("--eval-samples 2"));
        assert!(rendered.contains("--max-steps 1"));
        assert!(rendered.contains("--seed 11"));
        assert!(rendered.contains("--output-dir /tmp/output"));
        assert_eq!(
            argv.iter()
                .filter(|arg| arg.as_str() == "--samples")
                .count(),
            1
        );
        assert_eq!(
            argv.iter()
                .filter(|arg| arg.as_str() == "--eval-samples")
                .count(),
            1
        );
        assert_eq!(
            argv.iter()
                .filter(|arg| arg.as_str() == "--max-steps")
                .count(),
            1
        );
    }

    #[test]
    fn build_command_argv_adds_model_id_when_command_omits_it() {
        let mut job = sample_job("uv run osciris llm-lora-economics", vec![]);
        job.model_id = Some("Qwen/Qwen2.5-7B-Instruct".to_string());

        let argv = build_command_argv(&job, Path::new("/tmp/output")).unwrap();
        let rendered = argv.join(" ");

        assert!(rendered.contains("--model-id Qwen/Qwen2.5-7B-Instruct"));
        assert!(rendered.contains("--output-dir /tmp/output"));
    }
}
