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

fn default_true() -> bool {
    true
}

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
#[serde(rename_all = "snake_case")]
pub enum ChallengeStatus {
    Open,
    ResolvedAccepted,
    ResolvedRejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Provider,
    Verifier,
    Enterprise,
    Relay,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    OnlineIdle,
    OnlineBusy,
    Degraded,
    OfflinePlanned,
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
pub struct NodeIdentity {
    pub node_id: String,
    pub role: NodeRole,
    pub ed25519_public_key_base64: String,
    pub evm_address: Option<String>,
    pub display_name: String,
    pub bootstrap_peers: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerPresence {
    pub node_id: String,
    pub role: NodeRole,
    pub ed25519_public_key_base64: String,
    pub evm_address: Option<String>,
    pub listen_addrs: Vec<String>,
    pub relay_capable: bool,
    pub protocol_version: String,
    pub client_version: String,
    pub status: NodeStatus,
    pub current_load: f64,
    pub active_job_count: u32,
    pub last_seen_at: String,
    pub capability_version: Option<String>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderCapability {
    pub node_id: String,
    pub ed25519_public_key_base64: String,
    pub host_class: String,
    pub gpu_model: String,
    pub gpu_count: u32,
    pub vram_gb: f64,
    pub cuda_available: bool,
    pub mps_available: bool,
    pub supported_job_types: Vec<JobType>,
    pub supported_runtimes: Vec<String>,
    pub pricing_hint: Option<String>,
    pub current_load: f64,
    pub active_job_count: u32,
    pub status: NodeStatus,
    pub updated_at: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobAnnouncement {
    pub job_id: Uuid,
    pub job_spec: JobSpec,
    pub submitter_node_id: String,
    pub submitter_ed25519_public_key_base64: String,
    pub job_type: JobType,
    pub privacy_mode: PrivacyMode,
    pub required_capability: String,
    pub estimated_runtime_class: String,
    pub payment_token: String,
    pub escrow_amount_atomic: String,
    pub required_verifier_count: u8,
    pub announced_at: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobClaim {
    pub job_id: Uuid,
    pub provider_node_id: String,
    pub provider_ed25519_public_key_base64: String,
    pub claimed_at: String,
    pub claim_note: Option<String>,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobAssignment {
    pub job_id: Uuid,
    pub assigned_provider_node_id: String,
    pub assigner_node_id: String,
    pub assigner_ed25519_public_key_base64: String,
    pub assignment_reason: String,
    pub assigned_at: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptAvailability {
    pub job_id: Uuid,
    pub provider_node_id: String,
    pub provider_ed25519_public_key_base64: String,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
    pub bundle_uri: String,
    pub announced_at: String,
    pub signature: String,
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
    #[serde(default)]
    pub vram_gb: Option<f64>,
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
    #[serde(default = "default_true")]
    pub hardware_claim_valid: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationReceiptAnnouncement {
    pub verifier_node_id: String,
    pub verifier_ed25519_public_key_base64: String,
    pub verification_receipt: VerificationReceipt,
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
    pub bundle_sha256: String,
    pub opened_by: String,
    pub opened_by_ed25519_public_key_base64: String,
    pub reason_code: ChallengeReasonCode,
    pub reason_detail: String,
    pub opened_at: String,
    pub status: ChallengeStatus,
    pub resolved_by: Option<String>,
    pub resolved_by_ed25519_public_key_base64: Option<String>,
    pub resolved_at: Option<String>,
    pub resolution_note: Option<String>,
    pub signature: String,
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

pub fn verify_verification_receipt_signature(
    receipt: &VerificationReceipt,
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

pub fn sign_peer_presence(
    presence: &PeerPresence,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = presence.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_peer_presence_signature(
    presence: &PeerPresence,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&presence.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = presence.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_provider_capability(
    capability: &ProviderCapability,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = capability.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_provider_capability_signature(
    capability: &ProviderCapability,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&capability.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = capability.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_job_announcement(
    announcement: &JobAnnouncement,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = announcement.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_job_announcement_signature(
    announcement: &JobAnnouncement,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&announcement.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = announcement.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_job_claim(claim: &JobClaim, signing_key: &SigningKey) -> Result<String, CoreError> {
    let mut unsigned = claim.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_job_claim_signature(
    claim: &JobClaim,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&claim.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = claim.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_job_assignment(
    assignment: &JobAssignment,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = assignment.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_job_assignment_signature(
    assignment: &JobAssignment,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&assignment.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = assignment.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_receipt_availability(
    availability: &ReceiptAvailability,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = availability.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_receipt_availability_signature(
    availability: &ReceiptAvailability,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&availability.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = availability.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
}

pub fn sign_challenge_record(
    challenge: &ChallengeRecord,
    signing_key: &SigningKey,
) -> Result<String, CoreError> {
    let mut unsigned = challenge.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    Ok(BASE64.encode(signing_key.sign(&bytes).to_bytes()))
}

pub fn verify_challenge_record_signature(
    challenge: &ChallengeRecord,
    verifying_key: &VerifyingKey,
) -> Result<(), CoreError> {
    let signature = BASE64.decode(&challenge.signature)?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| CoreError::SignatureVerification)?;
    let mut unsigned = challenge.clone();
    unsigned.signature.clear();
    let bytes = canonical_json_bytes(&unsigned)?;
    verifying_key
        .verify(&bytes, &signature)
        .map_err(|_| CoreError::SignatureVerification)
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
                vram_gb: Some(24.0),
            },
            signature: String::new(),
            signing_key_id: "provider-key-1".to_string(),
        };

        receipt.signature = sign_execution_receipt(&receipt, &signing_key).unwrap();
        verify_execution_receipt_signature(&receipt, &verifying_key).unwrap();
    }

    #[test]
    fn verification_receipt_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[8_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut receipt = VerificationReceipt {
            verification_receipt_id: Uuid::now_v7(),
            receipt_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            verifier_id: "verifier-1".to_string(),
            verification_status: VerificationStatus::Accepted,
            verified_at: "2026-06-04T00:02:00Z".to_string(),
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
            bundle_sha256: "b".repeat(64),
            signature: String::new(),
            signing_key_id: "verifier-key-1".to_string(),
        };

        receipt.signature = sign_verification_receipt(&receipt, &signing_key).unwrap();
        verify_verification_receipt_signature(&receipt, &verifying_key).unwrap();
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

    #[test]
    fn peer_presence_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut presence = PeerPresence {
            node_id: "provider-1".to_string(),
            role: NodeRole::Provider,
            ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            evm_address: Some("0x1111111111111111111111111111111111111111".to_string()),
            listen_addrs: vec!["/ip4/127.0.0.1/tcp/9000".to_string()],
            relay_capable: false,
            protocol_version: "0.1.0".to_string(),
            client_version: "0.1.0".to_string(),
            status: NodeStatus::OnlineIdle,
            current_load: 0.0,
            active_job_count: 0,
            last_seen_at: "2026-06-04T00:00:00Z".to_string(),
            capability_version: Some("cap-v1".to_string()),
            signature: String::new(),
        };

        presence.signature = sign_peer_presence(&presence, &signing_key).unwrap();
        verify_peer_presence_signature(&presence, &verifying_key).unwrap();
    }

    #[test]
    fn provider_capability_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[10_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut capability = ProviderCapability {
            node_id: "provider-1".to_string(),
            ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            host_class: "aws-g5".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            cuda_available: true,
            mps_available: false,
            supported_job_types: vec![JobType::LlmLoraEconomics],
            supported_runtimes: vec!["python".to_string(), "cuda".to_string()],
            pricing_hint: Some("1.01 USD/hour".to_string()),
            current_load: 0.0,
            active_job_count: 0,
            status: NodeStatus::OnlineIdle,
            updated_at: "2026-06-04T00:00:00Z".to_string(),
            signature: String::new(),
        };

        capability.signature = sign_provider_capability(&capability, &signing_key).unwrap();
        verify_provider_capability_signature(&capability, &verifying_key).unwrap();
    }

    #[test]
    fn job_announcement_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[31_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let job_spec = JobSpec {
            job_id: Uuid::now_v7(),
            job_type: JobType::LlmLoraEconomics,
            dataset: Some("enterprise_synthetic".to_string()),
            model_id: Some("mock-7b".to_string()),
            command: "mock_llm_lora_economics.py".to_string(),
            args: vec!["--samples".to_string(), "8".to_string()],
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
        let mut announcement = JobAnnouncement {
            job_id: job_spec.job_id,
            job_spec,
            submitter_node_id: "enterprise-node-1".to_string(),
            submitter_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            job_type: JobType::LlmLoraEconomics,
            privacy_mode: PrivacyMode::DspPrepared,
            required_capability: "gpu>=24gb".to_string(),
            estimated_runtime_class: "short".to_string(),
            payment_token: "USDC_TEST".to_string(),
            escrow_amount_atomic: "1000000".to_string(),
            required_verifier_count: 1,
            announced_at: "2026-06-04T00:00:00Z".to_string(),
            signature: String::new(),
        };

        announcement.signature = sign_job_announcement(&announcement, &signing_key).unwrap();
        verify_job_announcement_signature(&announcement, &verifying_key).unwrap();
    }

    #[test]
    fn job_claim_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[41_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut claim = JobClaim {
            job_id: Uuid::now_v7(),
            provider_node_id: "provider-node-1".to_string(),
            provider_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            claimed_at: "2026-06-04T00:00:00Z".to_string(),
            claim_note: Some("gpu>=24gb available".to_string()),
            signature: String::new(),
        };

        claim.signature = sign_job_claim(&claim, &signing_key).unwrap();
        verify_job_claim_signature(&claim, &verifying_key).unwrap();
    }

    #[test]
    fn job_assignment_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[45_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut assignment = JobAssignment {
            job_id: Uuid::now_v7(),
            assigned_provider_node_id: "provider-node-1".to_string(),
            assigner_node_id: "enterprise-node-1".to_string(),
            assigner_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            assignment_reason: "manual_assignment".to_string(),
            assigned_at: "2026-06-04T00:00:00Z".to_string(),
            signature: String::new(),
        };

        assignment.signature = sign_job_assignment(&assignment, &signing_key).unwrap();
        verify_job_assignment_signature(&assignment, &verifying_key).unwrap();
    }

    #[test]
    fn receipt_availability_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[51_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut availability = ReceiptAvailability {
            job_id: Uuid::now_v7(),
            provider_node_id: "provider-node-1".to_string(),
            provider_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            execution_receipt_sha256: "a".repeat(64),
            bundle_sha256: "b".repeat(64),
            bundle_uri: "file:///tmp/evidence".to_string(),
            announced_at: "2026-06-04T00:00:00Z".to_string(),
            signature: String::new(),
        };

        availability.signature = sign_receipt_availability(&availability, &signing_key).unwrap();
        verify_receipt_availability_signature(&availability, &verifying_key).unwrap();
    }

    #[test]
    fn challenge_record_signature_round_trips() {
        let signing_key = SigningKey::from_bytes(&[55_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let mut challenge = ChallengeRecord {
            challenge_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            bundle_sha256: "b".repeat(64),
            opened_by: "verifier-node-1".to_string(),
            opened_by_ed25519_public_key_base64: verifying_key_to_base64(&verifying_key),
            reason_code: ChallengeReasonCode::MissingRequiredMetric,
            reason_detail: "metrics JSON missing aggregate".to_string(),
            opened_at: "2026-06-04T00:00:00Z".to_string(),
            status: ChallengeStatus::Open,
            resolved_by: None,
            resolved_by_ed25519_public_key_base64: None,
            resolved_at: None,
            resolution_note: None,
            signature: String::new(),
        };

        challenge.signature = sign_challenge_record(&challenge, &signing_key).unwrap();
        verify_challenge_record_signature(&challenge, &verifying_key).unwrap();
    }
}
