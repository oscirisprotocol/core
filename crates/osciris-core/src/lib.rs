use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub const SHA256_ALGORITHM: &str = "sha256";
pub const MERKLE_CHUNK_SIZE: usize = 8192;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("base64 decode failed: {0}")]
    Base64Decode(#[from] base64::DecodeError),
    #[error("invalid ed25519 key length: expected 32 bytes, got {0}")]
    InvalidKeyLength(usize),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("signature verification failed")]
    SignatureVerification,
    #[error("path {0} is not under base directory {1}")]
    PathNotRelative(PathBuf, PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    LlmLoraEconomics,
    ProductionProof,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    RawBaseline,
    DspPrepared,
    DpModelRelease,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Created,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Accepted,
    Rejected,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChainSubmissionStatus {
    Pending,
    Ready,
    Submitted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeReasonCode {
    ArtifactHashMismatch,
    MissingRequiredMetric,
    InvalidProviderSignature,
    InvalidVerifierSignature,
    DuplicateReceiptSubmission,
    ForbiddenJobTransition,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrivacyPolicy {
    pub privacy_mode: PrivacyMode,
    pub release_object: String,
    pub formal_dp_claim: bool,
    pub sensitive_field_policy: String,
    pub evidence_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    pub job_id: Uuid,
    pub job_type: JobType,
    pub dataset: Option<String>,
    pub model_id: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub privacy_policy: PrivacyPolicy,
    pub required_verifier_count: u8,
    pub challenge_window_seconds: u64,
    pub payment_token: String,
    pub escrow_amount_atomic: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifest {
    pub name: String,
    pub algorithm: String,
    pub chunk_size: usize,
    pub chunks: Vec<String>,
    pub merkle_root: String,
    pub byte_length: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuMetadata {
    pub gpu_model: String,
    pub driver: String,
    pub cuda_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionReceipt {
    pub receipt_id: Uuid,
    pub job_id: Uuid,
    pub provider_id: String,
    pub job_type: JobType,
    pub status: ExecutionStatus,
    pub command_exit_code: i32,
    pub started_at: String,
    pub finished_at: String,
    pub wall_clock_seconds: f64,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub artifact_root_sha256: String,
    pub artifact_manifests: Vec<ArtifactManifest>,
    pub metrics_path: String,
    pub gpu_metadata: GpuMetadata,
    pub signature: String,
    pub signing_key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationChecks {
    pub manifest_valid: bool,
    pub stdout_hash_valid: bool,
    pub stderr_hash_valid: bool,
    pub artifact_root_valid: bool,
    pub required_metrics_present: bool,
    pub signature_valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationReceipt {
    pub verification_receipt_id: Uuid,
    pub receipt_id: Uuid,
    pub job_id: Uuid,
    pub verifier_id: String,
    pub verification_status: VerificationStatus,
    pub verified_at: String,
    pub checks: VerificationChecks,
    pub failure_reasons: Vec<String>,
    pub bundle_sha256: String,
    pub signature: String,
    pub signing_key_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptBundle {
    pub bundle_id: Uuid,
    pub job_id: Uuid,
    pub job_spec_sha256: String,
    pub execution_receipt_sha256: String,
    pub verification_receipt_sha256_list: Vec<String>,
    pub bundle_sha256: String,
    pub artifact_index_path: String,
    pub chain_submission_status: ChainSubmissionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleIndex {
    pub job_id: Uuid,
    pub artifacts: Vec<ArtifactManifest>,
    pub execution_receipt_path: String,
    pub verification_receipt_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChallengeRecord {
    pub challenge_id: Uuid,
    pub job_id: Uuid,
    pub bundle_id: Uuid,
    pub opened_by: String,
    pub reason_code: ChallengeReasonCode,
    pub reason_detail: String,
    pub opened_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettlementRecord {
    pub job_id: Uuid,
    pub escrow_contract: String,
    pub receipt_registry_contract: String,
    pub payment_token: String,
    pub stake_token: String,
    pub escrow_amount_atomic: String,
    pub provider_payout_atomic: String,
    pub protocol_fee_atomic: String,
    pub verifier_fee_atomic: String,
    pub settlement_tx_hash: String,
    pub settled_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandMetadata {
    pub command: String,
    pub argv: Vec<String>,
    pub working_directory: String,
    pub started_at: String,
    pub finished_at: String,
    pub exit_code: i32,
}

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CoreError> {
    let value = serde_json::to_value(value)?;
    let canonical = canonicalize_value(value);
    Ok(serde_json::to_vec(&canonical)?)
}

pub fn canonical_json_sha256<T: Serialize>(value: &T) -> Result<String, CoreError> {
    Ok(sha256_bytes(&canonical_json_bytes(value)?))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String, CoreError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn load_signing_key_from_base64_seed(seed_base64: &str) -> Result<SigningKey, CoreError> {
    let seed = BASE64.decode(seed_base64)?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|raw: Vec<u8>| CoreError::InvalidKeyLength(raw.len()))?;
    Ok(SigningKey::from_bytes(&seed))
}

pub fn verifying_key_to_base64(verifying_key: &VerifyingKey) -> String {
    BASE64.encode(verifying_key.to_bytes())
}

pub fn verifying_key_from_base64(encoded: &str) -> Result<VerifyingKey, CoreError> {
    let bytes = BASE64.decode(encoded)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|raw: Vec<u8>| CoreError::InvalidKeyLength(raw.len()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_execution_receipt(
    receipt: &ExecutionReceipt,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_execution_receipt_signature(
    receipt: &ExecutionReceipt,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&receipt.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_verification_receipt(
    receipt: &VerificationReceipt,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn bundle_hash(bundle: &ReceiptBundle) -> Result<String, CoreError> {
    let mut unsigned = bundle.clone();
    unsigned.bundle_sha256.clear();
    canonical_json_sha256(&unsigned)
}

pub fn single_chunk_manifest(path: &Path, base_dir: &Path) -> Result<ArtifactManifest, CoreError> {
    let bytes = std::fs::read(path)?;
    let digest = sha256_bytes(&bytes);
    let relative = path
        .strip_prefix(base_dir)
        .map_err(|_| CoreError::PathNotRelative(path.to_path_buf(), base_dir.to_path_buf()))?;
    Ok(ArtifactManifest {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| relative.display().to_string()),
        algorithm: SHA256_ALGORITHM.to_string(),
        chunk_size: MERKLE_CHUNK_SIZE,
        chunks: vec![digest.clone()],
        merkle_root: digest,
        byte_length: bytes.len() as u64,
        path: relative.display().to_string(),
    })
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_value).collect()),
        Value::Object(map) => {
            let ordered = map
                .into_iter()
                .map(|(key, value)| (key, canonicalize_value(value)))
                .collect::<BTreeMap<_, _>>();
            let mut canonical = Map::new();
            for (key, value) in ordered {
                canonical.insert(key, value);
            }
            Value::Object(canonical)
        }
        primitive => primitive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_hash_ignores_object_key_order() {
        let left = serde_json::json!({
            "b": 2,
            "a": {
                "z": true,
                "m": "value"
            }
        });
        let right = serde_json::json!({
            "a": {
                "m": "value",
                "z": true
            },
            "b": 2
        });

        assert_eq!(
            canonical_json_sha256(&left).unwrap(),
            canonical_json_sha256(&right).unwrap()
        );
    }

    #[test]
    fn execution_receipt_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut receipt = ExecutionReceipt {
            receipt_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            provider_id: "provider-1".to_string(),
            job_type: JobType::LlmLoraEconomics,
            status: ExecutionStatus::Completed,
            command_exit_code: 0,
            started_at: "2026-06-04T00:00:00Z".to_string(),
            finished_at: "2026-06-04T00:01:00Z".to_string(),
            wall_clock_seconds: 60.0,
            stdout_sha256: "stdout".to_string(),
            stderr_sha256: "stderr".to_string(),
            artifact_root_sha256: "artifact".to_string(),
            artifact_manifests: vec![],
            metrics_path: "metrics.json".to_string(),
            gpu_metadata: GpuMetadata {
                gpu_model: "A10G".to_string(),
                driver: "driver".to_string(),
                cuda_available: true,
            },
            signature: String::new(),
            signing_key_id: "provider-key-1".to_string(),
        };

        receipt.signature = sign_execution_receipt(&receipt, &signing_key).unwrap();
        verify_execution_receipt_signature(&receipt, &verifying_key).unwrap();
    }

    #[test]
    fn single_chunk_manifest_uses_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("example.txt");
        std::fs::write(&path, b"osciris").unwrap();

        let manifest = single_chunk_manifest(&path, temp.path()).unwrap();
        assert_eq!(manifest.path, "example.txt");
        assert_eq!(manifest.algorithm, SHA256_ALGORITHM);
        assert_eq!(manifest.chunks.len(), 1);
    }
}
