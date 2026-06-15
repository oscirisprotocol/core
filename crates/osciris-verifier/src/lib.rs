use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use osciris_chain::{provider_address_from_id, verifier_address_from_id, OscirisChain};
use osciris_core::{
    bundle_hash, canonical_json_sha256, load_signing_key_from_base64_seed, sha256_file,
    sign_verification_receipt, verify_execution_receipt_signature,
    verify_provider_capability_signature, verifying_key_from_base64, BundleIndex, ExecutionReceipt,
    JobType, ProviderCapability, ReceiptBundle, VerificationChecks, VerificationReceipt,
    VerificationStatus,
};
use osciris_node::store::ProtocolStore;
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct VerifierConfig {
    pub verifier_id: String,
    pub signing_key_id: String,
    pub signing_key_seed_base64: String,
}

#[derive(Debug, Clone)]
pub struct VerifyOutput {
    pub verification_receipt_path: PathBuf,
    pub receipt_bundle_path: PathBuf,
}

pub async fn verify_bundle_with_chain(
    evidence_dir: &Path,
    chain: &OscirisChain,
    config: &VerifierConfig,
) -> Result<VerifyOutput> {
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt: ExecutionReceipt =
        serde_json::from_slice(&fs::read(&execution_receipt_path).await?)?;
    let provider_address = provider_address_from_id(&receipt.provider_id)?;
    let identity = chain.fetch_provider_identity(provider_address).await?;
    let provider_public_key_base64 = BASE64.encode(identity.ed25519_public_key.as_slice());
    chain
        .assert_registered_provider_key(provider_address, &provider_public_key_base64)
        .await?;
    let verifier_address = verifier_address_from_id(&config.verifier_id)?;
    chain
        .assert_registered_verifier_seed(verifier_address, &config.signing_key_seed_base64)
        .await?;
    verify_bundle_internal(evidence_dir, &provider_public_key_base64, config).await
}

pub async fn verify_bundle(
    evidence_dir: &Path,
    provider_public_key_base64: &str,
    config: &VerifierConfig,
) -> Result<VerifyOutput> {
    verify_bundle_internal(evidence_dir, provider_public_key_base64, config).await
}

async fn verify_bundle_internal(
    evidence_dir: &Path,
    provider_public_key_base64: &str,
    config: &VerifierConfig,
) -> Result<VerifyOutput> {
    let protocol_root = protocol_root_from_evidence_dir(evidence_dir)?;
    let store = ProtocolStore::open(&protocol_root).await?;
    let execution_receipt_path = evidence_dir.join("execution_receipt.json");
    let receipt_bundle_path = evidence_dir.join("receipt_bundle.json");
    let bundle_index_path = evidence_dir.join("bundle_index.json");
    let receipt: ExecutionReceipt =
        serde_json::from_slice(&fs::read(&execution_receipt_path).await?)?;
    let provider_key = verifying_key_from_base64(provider_public_key_base64)?;
    let signature_valid = verify_execution_receipt_signature(&receipt, &provider_key).is_ok();
    let stdout_hash_valid = sha256_file(&evidence_dir.join("stdout.log"))? == receipt.stdout_sha256;
    let stderr_hash_valid = sha256_file(&evidence_dir.join("stderr.log"))? == receipt.stderr_sha256;
    let manifest_valid = verify_manifests(evidence_dir, &receipt)?;
    let artifact_root_valid =
        canonical_json_sha256(&receipt.artifact_manifests)? == receipt.artifact_root_sha256;
    let required_metrics_present = verify_metrics(evidence_dir, &receipt).await?;
    let provider_capability = store.load_provider_capability(&receipt.provider_id).await?;
    let hardware_claim_valid = verify_hardware_claim(provider_capability.as_ref(), &receipt);

    let checks = VerificationChecks {
        manifest_valid,
        stdout_hash_valid,
        stderr_hash_valid,
        artifact_root_valid,
        required_metrics_present,
        signature_valid,
        hardware_claim_valid,
    };
    let failure_reasons = collect_failure_reasons(&checks);
    let verification_status = if failure_reasons.is_empty() {
        VerificationStatus::Accepted
    } else {
        VerificationStatus::Rejected
    };
    let bundle: ReceiptBundle = serde_json::from_slice(&fs::read(&receipt_bundle_path).await?)?;
    let signing_key = load_signing_key_from_base64_seed(&config.signing_key_seed_base64)?;
    let mut verification_receipt = VerificationReceipt {
        verification_receipt_id: Uuid::now_v7(),
        receipt_id: receipt.receipt_id,
        job_id: receipt.job_id,
        verifier_id: config.verifier_id.clone(),
        verification_status,
        verified_at: Utc::now().to_rfc3339(),
        checks,
        failure_reasons,
        bundle_sha256: bundle.bundle_sha256.clone(),
        signature: String::new(),
        signing_key_id: config.signing_key_id.clone(),
    };
    verification_receipt.signature =
        sign_verification_receipt(&verification_receipt, &signing_key)?;
    store
        .record_verification_receipt(&verification_receipt)
        .await?;

    let verification_dir = evidence_dir.join("verification_receipts");
    fs::create_dir_all(&verification_dir).await?;
    let verification_receipt_path = verification_dir.join(format!("{}.json", config.verifier_id));
    fs::write(
        &verification_receipt_path,
        serde_json::to_vec_pretty(&verification_receipt)?,
    )
    .await?;

    let mut updated_bundle = bundle;
    let verification_hash = sha256_file(&verification_receipt_path)?;
    if !updated_bundle
        .verification_receipt_sha256_list
        .iter()
        .any(|existing| existing == &verification_hash)
    {
        updated_bundle
            .verification_receipt_sha256_list
            .push(verification_hash);
    }
    updated_bundle.bundle_sha256 = bundle_hash(&updated_bundle)?;
    fs::write(
        &receipt_bundle_path,
        serde_json::to_vec_pretty(&updated_bundle)?,
    )
    .await?;
    store.record_receipt_bundle(&updated_bundle).await?;

    let mut bundle_index: BundleIndex =
        serde_json::from_slice(&fs::read(&bundle_index_path).await?)?;
    let relative_verification = verification_receipt_path
        .strip_prefix(evidence_dir)
        .context("verification receipt path was not under evidence directory")?
        .display()
        .to_string();
    if !bundle_index
        .verification_receipt_paths
        .iter()
        .any(|path| path == &relative_verification)
    {
        bundle_index
            .verification_receipt_paths
            .push(relative_verification);
    }
    fs::write(
        &bundle_index_path,
        serde_json::to_vec_pretty(&bundle_index)?,
    )
    .await?;

    Ok(VerifyOutput {
        verification_receipt_path,
        receipt_bundle_path,
    })
}

fn protocol_root_from_evidence_dir(evidence_dir: &Path) -> Result<PathBuf> {
    let evidence_root = evidence_dir
        .parent()
        .context("evidence directory has no parent")?;
    let protocol_root = evidence_root
        .parent()
        .context("evidence directory is not nested under .osciris/evidence")?;
    Ok(protocol_root.to_path_buf())
}

fn verify_manifests(evidence_dir: &Path, receipt: &ExecutionReceipt) -> Result<bool> {
    for manifest in &receipt.artifact_manifests {
        let path = evidence_dir.join(&manifest.path);
        if !path.exists() {
            return Ok(false);
        }
        let digest = sha256_file(&path)?;
        if manifest.algorithm != "sha256"
            || manifest.chunk_size != osciris_core::MERKLE_CHUNK_SIZE
            || manifest.chunks != vec![digest.clone()]
            || manifest.merkle_root != digest
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn verify_metrics(evidence_dir: &Path, receipt: &ExecutionReceipt) -> Result<bool> {
    let metrics_path = evidence_dir.join(&receipt.metrics_path);
    if !metrics_path.exists() {
        return Ok(false);
    }
    let payload: serde_json::Value = serde_json::from_slice(&fs::read(&metrics_path).await?)?;
    match receipt.job_type {
        JobType::LlmLoraEconomics => Ok(payload.get("kind").and_then(|v| v.as_str())
            == Some("llm_lora_economics_benchmark")
            && payload.get("aggregate").is_some()
            && payload.get("runs").is_some()
            && payload.get("config").is_some()),
        JobType::InferenceEconomics => Ok(payload.get("kind").and_then(|v| v.as_str())
            == Some("inference_economics_benchmark")
            && payload.get("aggregate").is_some()
            && payload.get("runs").is_some()
            && payload.get("config").is_some()),
        JobType::ProductionProof => {
            Ok(payload.get("tracks").is_some() && payload.get("status_counts").is_some())
        }
    }
}

fn collect_failure_reasons(checks: &VerificationChecks) -> Vec<String> {
    let mut reasons = vec![];
    if !checks.manifest_valid {
        reasons.push("artifact_hash_mismatch".to_string());
    }
    if !checks.stdout_hash_valid {
        reasons.push("stdout_hash_mismatch".to_string());
    }
    if !checks.stderr_hash_valid {
        reasons.push("stderr_hash_mismatch".to_string());
    }
    if !checks.artifact_root_valid {
        reasons.push("artifact_root_mismatch".to_string());
    }
    if !checks.required_metrics_present {
        reasons.push("missing_required_metric".to_string());
    }
    if !checks.signature_valid {
        reasons.push("invalid_provider_signature".to_string());
    }
    if !checks.hardware_claim_valid {
        reasons.push("invalid_hardware_claim".to_string());
    }
    reasons
}

fn verify_hardware_claim(
    capability: Option<&ProviderCapability>,
    receipt: &ExecutionReceipt,
) -> bool {
    let Some(capability) = capability else {
        return false;
    };

    let Ok(capability_key) = verifying_key_from_base64(&capability.ed25519_public_key_base64)
    else {
        return false;
    };
    if verify_provider_capability_signature(capability, &capability_key).is_err() {
        return false;
    }

    if capability.node_id != receipt.provider_id {
        return false;
    }

    if capability.cuda_available != receipt.gpu_metadata.cuda_available {
        return false;
    }

    if capability.gpu_count > 0 {
        if receipt.gpu_metadata.gpu_model.trim().is_empty() {
            return false;
        }
        let observed_model = receipt.gpu_metadata.gpu_model.to_ascii_lowercase();
        if observed_model == "unknown" || observed_model == "mock" {
            return false;
        }
        if capability.cuda_available && !receipt.gpu_metadata.cuda_available {
            return false;
        }
    }

    if let Some(observed_vram_gb) = receipt.gpu_metadata.vram_gb {
        if observed_vram_gb + f64::EPSILON < capability.vram_gb {
            return false;
        }
    } else if capability.gpu_count > 0 && capability.vram_gb > 0.0 {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        load_signing_key_from_base64_seed, sign_provider_capability, verifying_key_to_base64,
        ExecutionStatus, GpuMetadata, JobSpec, JobType, NodeStatus, PrivacyMode, PrivacyPolicy,
        ProviderCapability,
    };
    use osciris_node::{run_job, ProviderConfig};
    use uuid::Uuid;

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
    async fn verifier_accepts_valid_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let provider_seed = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string();
        let provider = ProviderConfig {
            provider_id: "provider-1".to_string(),
            signing_key_id: "provider-key-1".to_string(),
            signing_key_seed_base64: provider_seed.clone(),
            repo_root: temp.path().to_path_buf(),
            work_root: temp.path().to_path_buf(),
        };
        let script = r#"import json, pathlib, sys; output_dir = pathlib.Path(sys.argv[sys.argv.index("--output-dir") + 1]); output_dir.mkdir(parents=True, exist_ok=True); (output_dir / "llm_lora_economics.json").write_text(json.dumps({"kind": "llm_lora_economics_benchmark", "config": {"model_id": "mock-model"}, "aggregate": {"quality_retention": 1.0}, "runs": [{"mode": "raw_lora"}, {"mode": "dsp_prepared_lora"}]}, indent=2), encoding="utf-8")"#;
        let job = sample_job("python3", vec!["-c".to_string(), script.to_string()]);
        let run = run_job(&job, &provider).await.unwrap();
        let store = ProtocolStore::open(&temp.path().join(".osciris"))
            .await
            .unwrap();
        let provider_signing_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let mut provider_capability = ProviderCapability {
            node_id: provider.provider_id.clone(),
            ed25519_public_key_base64: verifying_key_to_base64(
                &provider_signing_key.verifying_key(),
            ),
            host_class: "local-mock".to_string(),
            gpu_model: "none".to_string(),
            gpu_count: 0,
            vram_gb: 0.0,
            cuda_available: false,
            mps_available: false,
            supported_job_types: vec![JobType::LlmLoraEconomics],
            supported_runtimes: vec!["python".to_string()],
            pricing_hint: Some("local test".to_string()),
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: "2026-06-05T00:00:00Z".to_string(),
            signature: String::new(),
        };
        provider_capability.signature =
            sign_provider_capability(&provider_capability, &provider_signing_key).unwrap();
        store
            .record_provider_capability(&provider_capability)
            .await
            .unwrap();

        let verifier = VerifierConfig {
            verifier_id: "verifier-1".to_string(),
            signing_key_id: "verifier-key-1".to_string(),
            signing_key_seed_base64: "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=".to_string(),
        };
        let provider_key = load_signing_key_from_base64_seed(&provider_seed)
            .unwrap()
            .verifying_key();
        let output = verify_bundle(
            &run.evidence_dir,
            &verifying_key_to_base64(&provider_key),
            &verifier,
        )
        .await
        .unwrap();
        assert!(output.verification_receipt_path.exists());

        assert_eq!(
            store
                .verification_receipt_count(&job.job_id.to_string())
                .await
                .unwrap(),
            1
        );

        verify_bundle(
            &run.evidence_dir,
            &verifying_key_to_base64(&provider_key),
            &verifier,
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .verification_receipt_count(&job.job_id.to_string())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn verifier_accepts_inference_economics_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let provider_seed = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=".to_string();
        let provider = ProviderConfig {
            provider_id: "provider-1".to_string(),
            signing_key_id: "provider-key-1".to_string(),
            signing_key_seed_base64: provider_seed.clone(),
            repo_root: temp.path().to_path_buf(),
            work_root: temp.path().to_path_buf(),
        };
        let script = r#"import json, pathlib, sys; output_dir = pathlib.Path(sys.argv[sys.argv.index("--output-dir") + 1]); output_dir.mkdir(parents=True, exist_ok=True); (output_dir / "inference_economics.json").write_text(json.dumps({"kind": "inference_economics_benchmark", "config": {"model": "mock-instruct"}, "aggregate": {"cost_to_quality_savings_mean": 0.42}, "runs": [{"seed": 11}]}, indent=2), encoding="utf-8")"#;
        let mut job = sample_job("python3", vec!["-c".to_string(), script.to_string()]);
        job.job_type = JobType::InferenceEconomics;
        job.model_id = Some("mock-instruct".to_string());
        let run = run_job(&job, &provider).await.unwrap();
        let store = ProtocolStore::open(&temp.path().join(".osciris"))
            .await
            .unwrap();
        let provider_signing_key = load_signing_key_from_base64_seed(&provider_seed).unwrap();
        let mut provider_capability = ProviderCapability {
            node_id: provider.provider_id.clone(),
            ed25519_public_key_base64: verifying_key_to_base64(
                &provider_signing_key.verifying_key(),
            ),
            host_class: "local-mock".to_string(),
            gpu_model: "none".to_string(),
            gpu_count: 0,
            vram_gb: 0.0,
            cuda_available: false,
            mps_available: false,
            supported_job_types: vec![JobType::InferenceEconomics],
            supported_runtimes: vec!["python".to_string()],
            pricing_hint: Some("local test".to_string()),
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: "2026-06-15T00:00:00Z".to_string(),
            signature: String::new(),
        };
        provider_capability.signature =
            sign_provider_capability(&provider_capability, &provider_signing_key).unwrap();
        store
            .record_provider_capability(&provider_capability)
            .await
            .unwrap();

        let verifier = VerifierConfig {
            verifier_id: "verifier-1".to_string(),
            signing_key_id: "verifier-key-1".to_string(),
            signing_key_seed_base64: "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=".to_string(),
        };
        let provider_key = load_signing_key_from_base64_seed(&provider_seed)
            .unwrap()
            .verifying_key();
        let output = verify_bundle(
            &run.evidence_dir,
            &verifying_key_to_base64(&provider_key),
            &verifier,
        )
        .await
        .unwrap();
        assert!(output.verification_receipt_path.exists());
        assert_eq!(
            store
                .verification_receipt_count(&job.job_id.to_string())
                .await
                .unwrap(),
            1
        );
    }

    fn hardware_receipt(provider_id: &str, gpu_metadata: GpuMetadata) -> ExecutionReceipt {
        ExecutionReceipt {
            receipt_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            provider_id: provider_id.to_string(),
            job_type: JobType::LlmLoraEconomics,
            status: ExecutionStatus::Completed,
            command_exit_code: 0,
            started_at: "2026-06-05T00:00:00Z".to_string(),
            finished_at: "2026-06-05T00:01:00Z".to_string(),
            wall_clock_seconds: 60.0,
            stdout_sha256: "stdout".to_string(),
            stderr_sha256: "stderr".to_string(),
            artifact_root_sha256: "artifact".to_string(),
            artifact_manifests: vec![],
            metrics_path: "llm_lora_economics.json".to_string(),
            gpu_metadata,
            signature: "signature".to_string(),
            signing_key_id: "provider-key".to_string(),
        }
    }

    fn hardware_capability(provider_id: &str) -> ProviderCapability {
        let signing_key =
            load_signing_key_from_base64_seed("CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk=")
                .unwrap();
        let mut capability = ProviderCapability {
            node_id: provider_id.to_string(),
            ed25519_public_key_base64: verifying_key_to_base64(&signing_key.verifying_key()),
            host_class: "aws-g5".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            cuda_available: true,
            mps_available: false,
            supported_job_types: vec![JobType::LlmLoraEconomics],
            supported_runtimes: vec!["python".to_string(), "cuda".to_string()],
            pricing_hint: Some("g5.xlarge".to_string()),
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: "2026-06-05T00:00:00Z".to_string(),
            signature: String::new(),
        };
        capability.signature = sign_provider_capability(&capability, &signing_key).unwrap();
        capability
    }

    #[test]
    fn hardware_claim_accepts_matching_gpu_metadata() {
        let capability = hardware_capability("provider-a");
        let receipt = hardware_receipt(
            "provider-a",
            GpuMetadata {
                gpu_model: "NVIDIA A10G".to_string(),
                driver: "570.86".to_string(),
                cuda_available: true,
                vram_gb: Some(24.0),
            },
        );

        assert!(verify_hardware_claim(Some(&capability), &receipt));
    }

    #[test]
    fn hardware_claim_rejects_mock_or_missing_gpu_metadata() {
        let capability = hardware_capability("provider-a");
        let receipt = hardware_receipt(
            "provider-a",
            GpuMetadata {
                gpu_model: "mock".to_string(),
                driver: "mock".to_string(),
                cuda_available: false,
                vram_gb: None,
            },
        );

        assert!(!verify_hardware_claim(Some(&capability), &receipt));
        assert!(!verify_hardware_claim(None, &receipt));
    }
}
