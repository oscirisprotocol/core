use std::fs;
use std::future::Future;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::contract::{CallBuilder, CallDecoder};
use alloy::primitives::{keccak256, Address, FixedBytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use anyhow::{anyhow, bail, Context, Result};
use osciris_core::{
    load_signing_key_from_base64_seed, verifying_key_from_base64, ChainSubmissionStatus, JobSpec,
    ReceiptBundle,
};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

const RPC_MAX_ATTEMPTS: usize = 5;
const TX_RECEIPT_MAX_ATTEMPTS: usize = 30;
const TX_RECEIPT_POLL_SECONDS: u64 = 2;
const TX_JOURNAL_ENV: &str = "OSCIRIS_CHAIN_TX_JOURNAL_DIR";
const TX_LOCK_ENV: &str = "OSCIRIS_CHAIN_LOCK_DIR";
const GAS_LIMIT_BUFFER_PERCENT: u64 = 20;
const SIGNER_LOCK_WAIT_SECONDS: u64 = 180;
const SIGNER_LOCK_STALE_SECONDS: u64 = 1_800;
const SIGNER_LOCK_POLL_MILLIS: u64 = 250;

sol! {
    #[sol(rpc)]
    interface IOscirisProviderRegistry {
        function registerProvider(address stakeToken, uint256 stakeAmount, bytes32 ed25519PublicKey, string calldata metadataURI) external;
        function registerVerifier(bytes32 ed25519PublicKey, string calldata metadataURI) external;
        function getProviderIdentity(address provider) external view returns (bool active, address stakeToken, uint256 stakeAmount, bytes32 ed25519PublicKey, string memory metadataURI);
        function getVerifierIdentity(address verifier) external view returns (bool active, bytes32 ed25519PublicKey, string memory metadataURI);
        function isProviderActive(address provider) external view returns (bool);
        function isVerifierActive(address verifier) external view returns (bool);
    }

    #[sol(rpc)]
    interface IOscirisReceiptRegistry {
        function submitReceiptBundle(
            bytes32 jobId,
            address provider,
            bytes32 executionReceiptHash,
            bytes32 bundleHash,
            bytes32[] calldata verifierReceiptHashes,
            address[] calldata verifierAddresses
        ) external;

        function getJobBundle(bytes32 jobId)
            external
            view
            returns (
                bytes32 executionReceiptHash,
                bytes32 bundleHash,
                uint256 verifierReceiptCount,
                address provider,
                bool exists
            );
    }

    #[sol(rpc)]
    interface IOscirisJobEscrow {
        function createJobEscrow(
            bytes32 jobId,
            address paymentToken,
            uint256 amount,
            uint256 challengeWindowSeconds,
            uint8 requiredVerifierCount
        ) external payable;

        function markJobSubmitted(bytes32 jobId, address provider, bytes32 bundleHash) external;
        function finalizeSettlement(bytes32 jobId) external;

        function escrows(bytes32 jobId)
            external
            view
            returns (
                address client,
                address paymentToken,
                uint256 amount,
                uint256 challengeWindowEndsAt,
                uint8 requiredVerifierCount,
                bytes32 bundleHash,
                address provider,
                uint8 status
            );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    pub provider_registry: Address,
    pub job_escrow: Address,
    pub receipt_registry: Address,
    pub explorer_url: Option<String>,
    pub stake_token: Option<Address>,
    pub payment_token: Option<Address>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnchainIdentity {
    pub address: Address,
    pub active: bool,
    pub ed25519_public_key: B256,
    pub metadata_uri: String,
    pub stake_token: Option<Address>,
    pub stake_amount: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainSubmissionRecord {
    pub chain_id: u64,
    pub receipt_registry_tx_hash: String,
    pub escrow_tx_hash: String,
    pub bundle_hash: String,
    pub job_id: String,
    pub provider: Address,
    pub verifier_addresses: Vec<Address>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchSnapshot {
    pub chain_id: u64,
    pub job_id: String,
    pub bundle_exists: bool,
    pub provider: Option<Address>,
    pub execution_receipt_hash: Option<String>,
    pub bundle_hash: Option<String>,
    pub verifier_receipt_count: u64,
    pub escrow_client: Option<Address>,
    pub escrow_payment_token: Option<Address>,
    pub escrow_amount: Option<String>,
    pub challenge_window_ends_at: Option<u64>,
    pub required_verifier_count: Option<u8>,
    pub escrow_provider: Option<Address>,
    pub escrow_status: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterIdentityRequest {
    pub metadata_uri: String,
    pub ed25519_public_key_base64: String,
    pub stake_token: Option<Address>,
    pub stake_amount: Option<U256>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitBundleRequest {
    pub job_id: Uuid,
    pub provider_address: Address,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
    pub verifier_receipt_sha256_list: Vec<String>,
    pub verifier_addresses: Vec<Address>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingTransactionRecord {
    pub chain_id: u64,
    pub operation_key: String,
    pub operation_kind: String,
    pub operation_subject: String,
    pub tx_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_tx: Option<String>,
    pub signer_address: String,
    pub contract_address: String,
    pub created_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingTransactionJournal {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct PendingTransactionContext<'a> {
    operation_key: &'a str,
    operation_kind: &'a str,
    operation_subject: &'a str,
    signer_address: Address,
    contract_address: Address,
    label: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SignerTransactionLockRecord {
    chain_id: u64,
    signer_address: String,
    pid: u32,
    acquired_at_unix_seconds: u64,
}

#[derive(Debug)]
struct SignerTransactionLock {
    lock_dir: PathBuf,
    active: bool,
}

impl PendingTransactionJournal {
    fn from_env_or_default() -> Result<Self> {
        if let Ok(raw) = std::env::var(TX_JOURNAL_ENV) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Ok(Self {
                    root: PathBuf::from(trimmed),
                });
            }
        }
        let home =
            std::env::var("HOME").context("HOME is not set; set OSCIRIS_CHAIN_TX_JOURNAL_DIR")?;
        Ok(Self {
            root: PathBuf::from(home)
                .join(".config")
                .join("osciris")
                .join("pending-transactions"),
        })
    }

    #[cfg(test)]
    fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, chain_id: u64, operation_key: &str) -> PathBuf {
        self.root
            .join(chain_id.to_string())
            .join(format!("{}.json", sanitize_journal_key(operation_key)))
    }

    fn load(&self, chain_id: u64, operation_key: &str) -> Result<Option<PendingTransactionRecord>> {
        let path = self.path_for(chain_id, operation_key);
        match fs::read(&path) {
            Ok(raw) => {
                let record = serde_json::from_slice(&raw).with_context(|| {
                    format!("failed to parse pending tx journal {}", path.display())
                })?;
                Ok(Some(record))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("failed to read pending tx journal {}", path.display())),
        }
    }

    fn save(&self, record: &PendingTransactionRecord) -> Result<()> {
        let path = self.path_for(record.chain_id, &record.operation_key);
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("pending tx journal path has no parent"))?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create pending tx journal dir {}",
                parent.display()
            )
        })?;
        let tmp_path = path.with_extension("json.tmp");
        let raw =
            serde_json::to_vec_pretty(record).context("failed to encode pending tx record")?;
        fs::write(&tmp_path, raw).with_context(|| {
            format!("failed to write pending tx journal {}", tmp_path.display())
        })?;
        #[cfg(unix)]
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "failed to restrict pending tx journal permissions {}",
                tmp_path.display()
            )
        })?;
        fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to publish pending tx journal {}", path.display()))?;
        Ok(())
    }

    fn clear(&self, chain_id: u64, operation_key: &str) -> Result<()> {
        let path = self.path_for(chain_id, operation_key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error)
                .with_context(|| format!("failed to clear pending tx journal {}", path.display())),
        }
    }
}

impl SignerTransactionLock {
    async fn acquire(chain_id: u64, signer_address: Address) -> Result<Self> {
        let root = chain_lock_root_from_env_or_default()?;
        Self::acquire_in_root(root, chain_id, signer_address).await
    }

    async fn acquire_in_root(
        root: PathBuf,
        chain_id: u64,
        signer_address: Address,
    ) -> Result<Self> {
        let lock_dir = signer_lock_dir(&root, chain_id, signer_address);
        let deadline = unix_timestamp_seconds()?.saturating_add(SIGNER_LOCK_WAIT_SECONDS);
        loop {
            match Self::try_acquire(&lock_dir, chain_id, signer_address)? {
                Some(lock) => return Ok(lock),
                None => {
                    if unix_timestamp_seconds()? >= deadline {
                        bail!(
                            "timed out waiting for signer transaction lock {} after {} seconds",
                            lock_dir.display(),
                            SIGNER_LOCK_WAIT_SECONDS
                        );
                    }
                    sleep(Duration::from_millis(SIGNER_LOCK_POLL_MILLIS)).await;
                }
            }
        }
    }

    fn try_acquire(
        lock_dir: &Path,
        chain_id: u64,
        signer_address: Address,
    ) -> Result<Option<Self>> {
        let parent = lock_dir
            .parent()
            .ok_or_else(|| anyhow!("signer lock path has no parent"))?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create signer lock root {}", parent.display()))?;

        match fs::create_dir(lock_dir) {
            Ok(()) => {
                #[cfg(unix)]
                fs::set_permissions(lock_dir, fs::Permissions::from_mode(0o700)).with_context(
                    || format!("failed to restrict signer lock dir {}", lock_dir.display()),
                )?;
                let record = SignerTransactionLockRecord {
                    chain_id,
                    signer_address: signer_address.to_string(),
                    pid: std::process::id(),
                    acquired_at_unix_seconds: unix_timestamp_seconds()?,
                };
                write_signer_lock_record(lock_dir, &record)?;
                Ok(Some(Self {
                    lock_dir: lock_dir.to_path_buf(),
                    active: true,
                }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if signer_lock_is_stale(lock_dir)? {
                    fs::remove_dir_all(lock_dir).with_context(|| {
                        format!("failed to remove stale signer lock {}", lock_dir.display())
                    })?;
                    Ok(None)
                } else {
                    Ok(None)
                }
            }
            Err(error) => Err(error)
                .with_context(|| format!("failed to create signer lock {}", lock_dir.display())),
        }
    }
}

impl Drop for SignerTransactionLock {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::remove_dir_all(&self.lock_dir);
            self.active = false;
        }
    }
}

impl ChainConfig {
    pub fn from_path(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path)
            .with_context(|| format!("failed to read chain config {}", path.display()))?;
        let config: Self = serde_json::from_slice(&raw)
            .with_context(|| format!("failed to parse chain config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.rpc_url.trim().is_empty() {
            bail!("rpc_url must not be empty");
        }
        if self.chain_id == 0 {
            bail!("chain_id must not be zero");
        }
        if self.provider_registry.is_zero() {
            bail!("provider_registry must not be zero");
        }
        if self.job_escrow.is_zero() {
            bail!("job_escrow must not be zero");
        }
        if self.receipt_registry.is_zero() {
            bail!("receipt_registry must not be zero");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct OscirisChain {
    config: ChainConfig,
}

impl OscirisChain {
    pub fn new(config: ChainConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &ChainConfig {
        &self.config
    }

    pub async fn fetch_provider_identity(&self, provider: Address) -> Result<OnchainIdentity> {
        let identity = retry_rpc("fetch provider identity", || {
            let rpc = self.read_provider()?;
            let registry = IOscirisProviderRegistry::new(self.config.provider_registry, rpc);
            Ok(async move {
                registry
                    .getProviderIdentity(provider)
                    .call()
                    .await
                    .context("failed to fetch provider identity")
            })
        })
        .await?;
        Ok(OnchainIdentity {
            address: provider,
            active: identity.active,
            ed25519_public_key: identity.ed25519PublicKey,
            metadata_uri: identity.metadataURI,
            stake_token: Some(identity.stakeToken),
            stake_amount: Some(identity.stakeAmount.to_string()),
        })
    }

    pub async fn fetch_verifier_identity(&self, verifier: Address) -> Result<OnchainIdentity> {
        let identity = retry_rpc("fetch verifier identity", || {
            let rpc = self.read_provider()?;
            let registry = IOscirisProviderRegistry::new(self.config.provider_registry, rpc);
            Ok(async move {
                registry
                    .getVerifierIdentity(verifier)
                    .call()
                    .await
                    .context("failed to fetch verifier identity")
            })
        })
        .await?;
        Ok(OnchainIdentity {
            address: verifier,
            active: identity.active,
            ed25519_public_key: identity.ed25519PublicKey,
            metadata_uri: identity.metadataURI,
            stake_token: None,
            stake_amount: None,
        })
    }

    pub async fn assert_registered_provider_key(
        &self,
        provider: Address,
        provider_public_key_base64: &str,
    ) -> Result<OnchainIdentity> {
        let identity = self.fetch_provider_identity(provider).await?;
        if !identity.active {
            bail!("provider {provider} is not active on-chain");
        }
        let expected = base64_public_key_to_b256(provider_public_key_base64)?;
        if identity.ed25519_public_key != expected {
            bail!("provider public key does not match on-chain registry");
        }
        Ok(identity)
    }

    pub async fn assert_registered_verifier_seed(
        &self,
        verifier: Address,
        signing_key_seed_base64: &str,
    ) -> Result<OnchainIdentity> {
        let identity = self.fetch_verifier_identity(verifier).await?;
        if !identity.active {
            bail!("verifier {verifier} is not active on-chain");
        }
        let signing_key = load_signing_key_from_base64_seed(signing_key_seed_base64)?;
        let expected = B256::from(signing_key.verifying_key().to_bytes());
        if identity.ed25519_public_key != expected {
            bail!("verifier signing key does not match on-chain registry");
        }
        Ok(identity)
    }

    pub async fn register_provider(
        &self,
        private_key_hex: &str,
        request: RegisterIdentityRequest,
    ) -> Result<String> {
        let signer = signer_from_private_key(private_key_hex)?;
        let provider_address = signer.address();
        let stake_token = request
            .stake_token
            .or(self.config.stake_token)
            .unwrap_or(Address::ZERO);
        let stake_amount = request.stake_amount.unwrap_or_default();
        let signing_key = base64_public_key_to_b256(&request.ed25519_public_key_base64)?;
        let operation_key = provider_registration_operation_key(provider_address);
        let existing = self.fetch_provider_identity(provider_address).await?;
        if existing.active {
            if existing.ed25519_public_key != signing_key {
                bail!("provider is already registered with a different signing key");
            }
            self.clear_pending_transaction(&operation_key)?;
            return Ok("already_registered".to_string());
        }
        let rpc = self.read_provider()?;
        let registry = IOscirisProviderRegistry::new(self.config.provider_registry, rpc);
        let tx_hash = self
            .broadcast_transaction_with_journal(
                PendingTransactionContext {
                    operation_key: &operation_key,
                    operation_kind: "provider_registration",
                    operation_subject: &provider_address.to_string(),
                    signer_address: provider_address,
                    contract_address: self.config.provider_registry,
                    label: "provider registration",
                },
                || async {
                    let call = registry.registerProvider(
                        stake_token,
                        stake_amount,
                        signing_key,
                        request.metadata_uri,
                    );
                    self.sign_prepared_call(call, signer.clone(), provider_address)
                        .await
                        .context("failed to pre-sign provider registration")
                },
            )
            .await?;
        Ok(format!("{tx_hash:#x}"))
    }

    pub async fn register_verifier(
        &self,
        private_key_hex: &str,
        request: RegisterIdentityRequest,
    ) -> Result<String> {
        let signer = signer_from_private_key(private_key_hex)?;
        let verifier_address = signer.address();
        let signing_key = base64_public_key_to_b256(&request.ed25519_public_key_base64)?;
        let operation_key = verifier_registration_operation_key(verifier_address);
        let existing = self.fetch_verifier_identity(verifier_address).await?;
        if existing.active {
            if existing.ed25519_public_key != signing_key {
                bail!("verifier is already registered with a different signing key");
            }
            self.clear_pending_transaction(&operation_key)?;
            return Ok("already_registered".to_string());
        }
        let rpc = self.read_provider()?;
        let registry = IOscirisProviderRegistry::new(self.config.provider_registry, rpc);
        let tx_hash = self
            .broadcast_transaction_with_journal(
                PendingTransactionContext {
                    operation_key: &operation_key,
                    operation_kind: "verifier_registration",
                    operation_subject: &verifier_address.to_string(),
                    signer_address: verifier_address,
                    contract_address: self.config.provider_registry,
                    label: "verifier registration",
                },
                || async {
                    let call = registry.registerVerifier(signing_key, request.metadata_uri);
                    self.sign_prepared_call(call, signer.clone(), verifier_address)
                        .await
                        .context("failed to pre-sign verifier registration")
                },
            )
            .await?;
        Ok(format!("{tx_hash:#x}"))
    }

    pub async fn create_job_escrow(
        &self,
        private_key_hex: &str,
        job_spec: &JobSpec,
    ) -> Result<String> {
        let signer = signer_from_private_key(private_key_hex)?;
        let client_address = signer.address();
        let job_id = uuid_to_b256(job_spec.job_id);
        let operation_key = job_escrow_creation_operation_key(job_spec.job_id);
        let payment_token = self
            .config
            .payment_token
            .ok_or_else(|| anyhow!("payment_token is not configured in chain config"))?;
        let amount = U256::from_str(&job_spec.escrow_amount_atomic).with_context(|| {
            format!(
                "invalid escrow_amount_atomic {}",
                job_spec.escrow_amount_atomic
            )
        })?;
        let existing = retry_rpc("inspect existing escrow", || {
            let rpc = self.read_provider()?;
            let escrow = IOscirisJobEscrow::new(self.config.job_escrow, rpc);
            Ok(async move {
                escrow
                    .escrows(job_id)
                    .call()
                    .await
                    .context("failed to inspect existing escrow")
            })
        })
        .await?;
        if existing.status != 0 {
            if existing.paymentToken != payment_token {
                bail!("job escrow already exists with a different payment token");
            }
            if existing.amount != amount {
                bail!("job escrow already exists with a different amount");
            }
            if existing.requiredVerifierCount != job_spec.required_verifier_count {
                bail!("job escrow already exists with a different verifier count");
            }
            self.clear_pending_transaction(&operation_key)?;
            return Ok("already_created".to_string());
        }
        let challenge_window_seconds = job_spec.challenge_window_seconds;
        let required_verifier_count = job_spec.required_verifier_count;
        let rpc = self.read_provider()?;
        let escrow = IOscirisJobEscrow::new(self.config.job_escrow, rpc);
        let call = escrow.createJobEscrow(
            job_id,
            payment_token,
            amount,
            U256::from(challenge_window_seconds),
            required_verifier_count,
        );
        let tx_hash = self
            .broadcast_transaction_with_journal(
                PendingTransactionContext {
                    operation_key: &operation_key,
                    operation_kind: "job_escrow_creation",
                    operation_subject: &job_spec.job_id.to_string(),
                    signer_address: client_address,
                    contract_address: self.config.job_escrow,
                    label: "job escrow creation",
                },
                || async {
                    self.sign_prepared_call(
                        call.value(escrow_native_value(payment_token, amount)),
                        signer.clone(),
                        client_address,
                    )
                    .await
                    .context("failed to pre-sign job escrow creation")
                },
            )
            .await?;
        Ok(format!("{tx_hash:#x}"))
    }

    pub async fn submit_receipt_bundle(
        &self,
        private_key_hex: &str,
        request: SubmitBundleRequest,
    ) -> Result<ChainSubmissionRecord> {
        let signer = signer_from_private_key(private_key_hex)?;
        let sender_address = signer.address();
        let job_id = uuid_to_b256(request.job_id);
        let execution_receipt_hash = hex_digest_to_b256(&request.execution_receipt_sha256)?;
        let bundle_hash = hex_digest_to_b256(&request.bundle_sha256)?;
        let registry_operation_key =
            receipt_bundle_submission_operation_key(request.job_id, bundle_hash);
        let escrow_operation_key =
            escrow_submission_operation_key(request.job_id, bundle_hash, request.provider_address);
        let verifier_receipt_hashes = request
            .verifier_receipt_sha256_list
            .iter()
            .map(|value| hex_digest_to_b256(value))
            .collect::<Result<Vec<_>>>()?;

        let existing_bundle = retry_rpc("inspect existing receipt bundle state", || {
            let registry =
                IOscirisReceiptRegistry::new(self.config.receipt_registry, self.read_provider()?);
            Ok(async move {
                registry
                    .getJobBundle(job_id)
                    .call()
                    .await
                    .context("failed to inspect existing receipt bundle state")
            })
        })
        .await?;
        let registry_hash = if existing_bundle.exists {
            if existing_bundle.bundleHash != bundle_hash {
                bail!("job already has a different bundle hash on-chain");
            }
            if existing_bundle.executionReceiptHash != execution_receipt_hash {
                bail!("job already has a different execution receipt hash on-chain");
            }
            if existing_bundle.provider != request.provider_address {
                bail!("job already has a different provider address on-chain");
            }
            if existing_bundle.verifierReceiptCount.to::<usize>() < request.verifier_addresses.len()
            {
                bail!("existing on-chain bundle has fewer verifier receipts than requested");
            }
            self.clear_pending_transaction(&registry_operation_key)?;
            "already_registered".to_string()
        } else {
            let rpc = self.read_provider()?;
            let registry = IOscirisReceiptRegistry::new(self.config.receipt_registry, rpc);
            let registry_tx_hash = self
                .broadcast_transaction_with_journal(
                    PendingTransactionContext {
                        operation_key: &registry_operation_key,
                        operation_kind: "receipt_bundle_submission",
                        operation_subject: &format!("{}:{bundle_hash:#x}", request.job_id),
                        signer_address: sender_address,
                        contract_address: self.config.receipt_registry,
                        label: "receipt bundle submission",
                    },
                    || async {
                        let call = registry.submitReceiptBundle(
                            job_id,
                            request.provider_address,
                            execution_receipt_hash,
                            bundle_hash,
                            verifier_receipt_hashes.clone(),
                            request.verifier_addresses.clone(),
                        );
                        self.sign_prepared_call(call, signer.clone(), sender_address)
                            .await
                            .context("failed to pre-sign receipt bundle submission")
                    },
                )
                .await?;
            format!("{registry_tx_hash:#x}")
        };

        let escrow_state = retry_rpc("inspect existing escrow state", || {
            let escrow = IOscirisJobEscrow::new(self.config.job_escrow, self.read_provider()?);
            Ok(async move {
                escrow
                    .escrows(job_id)
                    .call()
                    .await
                    .context("failed to inspect existing escrow state")
            })
        })
        .await?;
        let escrow_hash = if escrow_state.status == 2 {
            if escrow_state.bundleHash != bundle_hash {
                bail!("escrow is already submitted with a different bundle hash");
            }
            if escrow_state.provider != request.provider_address {
                bail!("escrow is already submitted with a different provider address");
            }
            self.clear_pending_transaction(&escrow_operation_key)?;
            "already_submitted".to_string()
        } else {
            let rpc = self.read_provider()?;
            let escrow = IOscirisJobEscrow::new(self.config.job_escrow, rpc);
            let escrow_tx_hash = self
                .broadcast_transaction_with_journal(
                    PendingTransactionContext {
                        operation_key: &escrow_operation_key,
                        operation_kind: "escrow_submission",
                        operation_subject: &format!(
                            "{}:{bundle_hash:#x}:{}",
                            request.job_id, request.provider_address
                        ),
                        signer_address: sender_address,
                        contract_address: self.config.job_escrow,
                        label: "escrow submission",
                    },
                    || async {
                        let call =
                            escrow.markJobSubmitted(job_id, request.provider_address, bundle_hash);
                        self.sign_prepared_call(call, signer.clone(), sender_address)
                            .await
                            .context("failed to pre-sign escrow submission")
                    },
                )
                .await?;
            format!("{escrow_tx_hash:#x}")
        };

        Ok(ChainSubmissionRecord {
            chain_id: self.config.chain_id,
            receipt_registry_tx_hash: registry_hash,
            escrow_tx_hash: escrow_hash,
            bundle_hash: request.bundle_sha256,
            job_id: request.job_id.to_string(),
            provider: request.provider_address,
            verifier_addresses: request.verifier_addresses,
        })
    }

    pub async fn finalize_settlement(&self, private_key_hex: &str, job_id: Uuid) -> Result<String> {
        let signer = signer_from_private_key(private_key_hex)?;
        let signer_address = signer.address();
        let key = uuid_to_b256(job_id);
        let operation_key = settlement_finalization_operation_key(job_id);
        let escrow_state = retry_rpc("inspect escrow before finalization", || {
            let escrow = IOscirisJobEscrow::new(self.config.job_escrow, self.read_provider()?);
            Ok(async move {
                escrow
                    .escrows(key)
                    .call()
                    .await
                    .context("failed to inspect escrow before finalization")
            })
        })
        .await?;
        if escrow_state.status == 3 {
            self.clear_pending_transaction(&operation_key)?;
            return Ok("already_finalized".to_string());
        }
        if escrow_state.status != 2 {
            bail!("escrow must be submitted before settlement can finalize");
        }
        let rpc = self.read_provider()?;
        let escrow = IOscirisJobEscrow::new(self.config.job_escrow, rpc);
        let tx_hash = self
            .broadcast_transaction_with_journal(
                PendingTransactionContext {
                    operation_key: &operation_key,
                    operation_kind: "settlement_finalization",
                    operation_subject: &job_id.to_string(),
                    signer_address,
                    contract_address: self.config.job_escrow,
                    label: "settlement finalization",
                },
                || async {
                    let call = escrow.finalizeSettlement(key);
                    self.sign_prepared_call(call, signer.clone(), signer_address)
                        .await
                        .context("failed to pre-sign settlement finalization")
                },
            )
            .await?;
        Ok(format!("{tx_hash:#x}"))
    }

    pub async fn watch_job(&self, job_id: Uuid) -> Result<WatchSnapshot> {
        let key = uuid_to_b256(job_id);
        let bundle = retry_rpc("load receipt bundle state", || {
            let registry =
                IOscirisReceiptRegistry::new(self.config.receipt_registry, self.read_provider()?);
            Ok(async move {
                registry
                    .getJobBundle(key)
                    .call()
                    .await
                    .context("failed to load receipt bundle state")
            })
        })
        .await?;
        let escrow_state = retry_rpc("load escrow state", || {
            let escrow = IOscirisJobEscrow::new(self.config.job_escrow, self.read_provider()?);
            Ok(async move {
                escrow
                    .escrows(key)
                    .call()
                    .await
                    .context("failed to load escrow state")
            })
        })
        .await?;

        Ok(WatchSnapshot {
            chain_id: self.config.chain_id,
            job_id: job_id.to_string(),
            bundle_exists: bundle.exists,
            provider: bundle.exists.then_some(bundle.provider),
            execution_receipt_hash: bundle
                .exists
                .then_some(format!("{:#x}", bundle.executionReceiptHash)),
            bundle_hash: bundle.exists.then_some(format!("{:#x}", bundle.bundleHash)),
            verifier_receipt_count: bundle.verifierReceiptCount.to::<u64>(),
            escrow_client: (!escrow_state.client.is_zero()).then_some(escrow_state.client),
            escrow_payment_token: (!escrow_state.paymentToken.is_zero())
                .then_some(escrow_state.paymentToken),
            escrow_amount: (!escrow_state.amount.is_zero())
                .then_some(escrow_state.amount.to_string()),
            challenge_window_ends_at: Some(escrow_state.challengeWindowEndsAt.to::<u64>()),
            required_verifier_count: Some(escrow_state.requiredVerifierCount),
            escrow_provider: (!escrow_state.provider.is_zero()).then_some(escrow_state.provider),
            escrow_status: Some(escrow_state.status),
        })
    }

    fn read_provider(&self) -> Result<impl Provider> {
        let url = self
            .config
            .rpc_url
            .parse()
            .with_context(|| format!("invalid rpc url {}", self.config.rpc_url))?;
        Ok(ProviderBuilder::new().connect_http(url))
    }

    async fn broadcast_transaction_with_journal<Fut, SendFn>(
        &self,
        context: PendingTransactionContext<'_>,
        send: SendFn,
    ) -> Result<B256>
    where
        SendFn: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let _signer_lock =
            SignerTransactionLock::acquire(self.config.chain_id, context.signer_address).await?;
        if let Some(tx_hash) = self
            .resume_pending_transaction(context.operation_key, context.label)
            .await?
        {
            return Ok(tx_hash);
        }

        let raw_tx = send().await?;
        let tx_hash = raw_transaction_hash(&raw_tx);
        self.save_pending_transaction(PendingTransactionRecord {
            chain_id: self.config.chain_id,
            operation_key: context.operation_key.to_string(),
            operation_kind: context.operation_kind.to_string(),
            operation_subject: context.operation_subject.to_string(),
            tx_hash: format!("{tx_hash:#x}"),
            raw_tx: Some(format!("0x{}", hex::encode(&raw_tx))),
            signer_address: context.signer_address.to_string(),
            contract_address: context.contract_address.to_string(),
            created_at_unix_seconds: unix_timestamp_seconds()?,
        })?;

        self.broadcast_raw_transaction(&raw_tx, tx_hash, context.label)
            .await?;
        match self.confirm_transaction(tx_hash, context.label).await {
            Ok(()) => {
                self.clear_pending_transaction(context.operation_key)?;
                Ok(tx_hash)
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "{} transaction {tx_hash:#x} is recorded in the pending transaction journal; rerun the same command to resume confirmation before broadcasting again",
                    context.label
                )
            }),
        }
    }

    async fn sign_prepared_call<P, D>(
        &self,
        call: CallBuilder<P, D>,
        signer: PrivateKeySigner,
        from: Address,
    ) -> Result<Vec<u8>>
    where
        P: Provider,
        D: CallDecoder,
    {
        let base_call = call.from(from).chain_id(self.config.chain_id);
        let gas_estimate = base_call
            .estimate_gas()
            .await
            .context("failed to estimate gas")?;
        let gas_limit = gas_with_buffer(gas_estimate);
        let gas_price = retry_rpc("fetch gas price", || {
            let rpc = self.read_provider()?;
            Ok(async move {
                rpc.get_gas_price()
                    .await
                    .context("failed to fetch gas price")
            })
        })
        .await?;
        let nonce = retry_rpc("fetch pending signer nonce", || {
            let rpc = self.read_provider()?;
            Ok(async move {
                rpc.get_transaction_count(from)
                    .pending()
                    .await
                    .context("failed to fetch pending signer nonce")
            })
        })
        .await?;

        base_call
            .gas(gas_limit)
            .gas_price(gas_price)
            .nonce(nonce)
            .build_raw_transaction(signer)
            .await
            .map_err(|error| anyhow!(error))
            .context("failed to build signed raw transaction")
    }

    async fn broadcast_raw_transaction(
        &self,
        raw_tx: &[u8],
        expected_tx_hash: B256,
        label: &str,
    ) -> Result<()> {
        let provider = self.read_provider()?;
        let pending = provider
            .send_raw_transaction(raw_tx)
            .await
            .with_context(|| {
                format!(
                    "{label} raw broadcast failed after pre-signing transaction {expected_tx_hash:#x}; rerun the same command to resume confirmation or rebroadcast the same signed transaction"
                )
            })?;
        let returned_hash = *pending.tx_hash();
        if returned_hash != expected_tx_hash {
            bail!(
                "{label} raw broadcast returned unexpected transaction hash {returned_hash:#x}; expected {expected_tx_hash:#x}"
            );
        }
        Ok(())
    }

    async fn resume_pending_transaction(
        &self,
        operation_key: &str,
        label: &str,
    ) -> Result<Option<B256>> {
        let journal = PendingTransactionJournal::from_env_or_default()?;
        let Some(record) = journal.load(self.config.chain_id, operation_key)? else {
            return Ok(None);
        };
        let tx_hash = B256::from_str(&record.tx_hash)
            .with_context(|| format!("invalid pending tx hash {}", record.tx_hash))?;
        if let Err(confirm_error) = self.confirm_transaction(tx_hash, label).await {
            let raw_tx = record
                .raw_tx
                .as_deref()
                .map(hex_to_bytes)
                .transpose()
                .context("failed to decode pending raw transaction")?;
            let Some(raw_tx) = raw_tx else {
                return Err(confirm_error).with_context(|| {
                    format!(
                        "pending {label} transaction {tx_hash:#x} is not confirmed yet and no signed raw transaction is available; leaving journal entry in place"
                    )
                });
            };
            self.broadcast_raw_transaction(&raw_tx, tx_hash, label)
                .await
                .with_context(|| {
                    format!(
                        "pending {label} transaction {tx_hash:#x} was not confirmed and rebroadcasting the same signed transaction failed; leaving journal entry in place"
                    )
                })?;
            self.confirm_transaction(tx_hash, label)
                .await
                .with_context(|| {
                    format!(
                        "pending {label} transaction {tx_hash:#x} was rebroadcast but is still not confirmed; leaving journal entry in place"
                    )
                })?;
        }
        journal.clear(self.config.chain_id, operation_key)?;
        Ok(Some(tx_hash))
    }

    fn save_pending_transaction(&self, record: PendingTransactionRecord) -> Result<()> {
        PendingTransactionJournal::from_env_or_default()?.save(&record)
    }

    fn clear_pending_transaction(&self, operation_key: &str) -> Result<()> {
        PendingTransactionJournal::from_env_or_default()?.clear(self.config.chain_id, operation_key)
    }

    async fn confirm_transaction(&self, tx_hash: B256, label: &str) -> Result<()> {
        let mut last_error = None;
        for attempt in 1..=TX_RECEIPT_MAX_ATTEMPTS {
            let provider = self.read_provider()?;
            match provider.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    if receipt.status() {
                        return Ok(());
                    }
                    bail!(
                        "{} transaction {tx_hash:#x} was mined with failed status",
                        label
                    );
                }
                Ok(None) => {
                    sleep(Duration::from_secs(TX_RECEIPT_POLL_SECONDS)).await;
                }
                Err(error) => {
                    let error = anyhow!(error);
                    if is_retryable_rpc_error(&error) && attempt < TX_RECEIPT_MAX_ATTEMPTS {
                        last_error = Some(error);
                        sleep(Duration::from_secs(TX_RECEIPT_POLL_SECONDS)).await;
                    } else {
                        return Err(error)
                            .with_context(|| format!("{label} receipt polling failed"));
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            anyhow!(
                "{} transaction {tx_hash:#x} was not confirmed after {} receipt polls",
                label,
                TX_RECEIPT_MAX_ATTEMPTS
            )
        }))
    }
}

async fn retry_rpc<T, Fut, Op>(label: &str, mut operation: Op) -> Result<T>
where
    Op: FnMut() -> Result<Fut>,
    Fut: Future<Output = Result<T>>,
{
    let mut last_error = None;
    for attempt in 1..=RPC_MAX_ATTEMPTS {
        match operation()?.await {
            Ok(value) => return Ok(value),
            Err(error) if is_retryable_rpc_error(&error) && attempt < RPC_MAX_ATTEMPTS => {
                last_error = Some(error);
                sleep(Duration::from_millis(250 * attempt as u64)).await;
            }
            Err(error) => return Err(error).with_context(|| format!("{label} failed")),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("{label} failed without an error")))
        .with_context(|| format!("{label} failed after {RPC_MAX_ATTEMPTS} attempts"))
}

fn is_retryable_rpc_error(error: &anyhow::Error) -> bool {
    let details = format!("{error:#}");
    let retryable_transport = [
        "BadRecordMac",
        "error sending request",
        "connection error",
        "client error (SendRequest)",
        "deadline has elapsed",
        "operation timed out",
    ];
    let non_retryable_contract = [
        "execution reverted",
        "server returned an error response",
        "already active",
        "already exists",
    ];
    retryable_transport
        .iter()
        .any(|needle| details.contains(needle))
        && !non_retryable_contract
            .iter()
            .any(|needle| details.contains(needle))
}

fn raw_transaction_hash(raw_tx: &[u8]) -> B256 {
    keccak256(raw_tx)
}

fn gas_with_buffer(estimate: u64) -> u64 {
    let buffer = estimate.saturating_mul(GAS_LIMIT_BUFFER_PERCENT) / 100;
    estimate.saturating_add(buffer).max(estimate)
}

fn escrow_native_value(payment_token: Address, amount: U256) -> U256 {
    if payment_token.is_zero() {
        amount
    } else {
        U256::ZERO
    }
}

fn hex_to_bytes(raw: &str) -> Result<Vec<u8>> {
    hex::decode(raw.trim_start_matches("0x"))
        .with_context(|| format!("invalid hex bytes {}", abbreviate_for_error(raw)))
}

fn chain_lock_root_from_env_or_default() -> Result<PathBuf> {
    if let Ok(raw) = std::env::var(TX_LOCK_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set; set OSCIRIS_CHAIN_LOCK_DIR")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("osciris")
        .join("chain-locks"))
}

fn signer_lock_dir(root: &Path, chain_id: u64, signer_address: Address) -> PathBuf {
    root.join(chain_id.to_string()).join(format!(
        "{}.lock",
        sanitize_journal_key(&signer_address.to_string())
    ))
}

fn write_signer_lock_record(lock_dir: &Path, record: &SignerTransactionLockRecord) -> Result<()> {
    let path = lock_dir.join("lock.json");
    let raw = serde_json::to_vec_pretty(record).context("failed to encode signer lock record")?;
    fs::write(&path, raw)
        .with_context(|| format!("failed to write signer lock record {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).with_context(|| {
        format!(
            "failed to restrict signer lock record permissions {}",
            path.display()
        )
    })?;
    Ok(())
}

fn read_signer_lock_record(lock_dir: &Path) -> Result<Option<SignerTransactionLockRecord>> {
    let path = lock_dir.join("lock.json");
    match fs::read(&path) {
        Ok(raw) => {
            let record = serde_json::from_slice(&raw).with_context(|| {
                format!("failed to parse signer lock record {}", path.display())
            })?;
            Ok(Some(record))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to read signer lock {}", path.display()))
        }
    }
}

fn signer_lock_is_stale(lock_dir: &Path) -> Result<bool> {
    let Some(record) = read_signer_lock_record(lock_dir)? else {
        return Ok(true);
    };
    let now = unix_timestamp_seconds()?;
    Ok(now.saturating_sub(record.acquired_at_unix_seconds) > SIGNER_LOCK_STALE_SECONDS)
}

fn abbreviate_for_error(raw: &str) -> String {
    const MAX: usize = 24;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...", &raw[..MAX])
    }
}

fn provider_registration_operation_key(provider_address: Address) -> String {
    format!("provider_registration_{provider_address}")
}

fn verifier_registration_operation_key(verifier_address: Address) -> String {
    format!("verifier_registration_{verifier_address}")
}

fn job_escrow_creation_operation_key(job_id: Uuid) -> String {
    format!("job_escrow_creation_{job_id}")
}

fn receipt_bundle_submission_operation_key(job_id: Uuid, bundle_hash: B256) -> String {
    format!("receipt_bundle_submission_{job_id}_{bundle_hash:#x}")
}

fn escrow_submission_operation_key(job_id: Uuid, bundle_hash: B256, provider: Address) -> String {
    format!("escrow_submission_{job_id}_{bundle_hash:#x}_{provider}")
}

fn settlement_finalization_operation_key(job_id: Uuid) -> String {
    format!("settlement_finalization_{job_id}")
}

fn sanitize_journal_key(raw: &str) -> String {
    raw.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '_',
        })
        .collect()
}

fn unix_timestamp_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs())
}

fn signer_from_private_key(private_key_hex: &str) -> Result<PrivateKeySigner> {
    PrivateKeySigner::from_str(private_key_hex.trim()).context("invalid private key hex")
}

pub fn parse_address(raw: &str, field: &str) -> Result<Address> {
    Address::from_str(raw).with_context(|| format!("invalid address for {field}: {raw}"))
}

pub fn base64_public_key_to_b256(raw: &str) -> Result<B256> {
    let key = verifying_key_from_base64(raw)?;
    Ok(B256::from(key.to_bytes()))
}

pub fn signer_seed_to_b256(raw: &str) -> Result<B256> {
    let key = load_signing_key_from_base64_seed(raw)?;
    Ok(B256::from(key.verifying_key().to_bytes()))
}

pub fn hex_digest_to_b256(raw: &str) -> Result<B256> {
    let trimmed = raw.trim_start_matches("0x");
    let bytes = hex::decode(trimmed).with_context(|| format!("invalid hex digest {raw}"))?;
    if bytes.len() != 32 {
        bail!("hex digest must decode to 32 bytes, got {}", bytes.len());
    }
    Ok(B256::from_slice(&bytes))
}

pub fn uuid_to_b256(job_id: Uuid) -> B256 {
    let mut bytes = [0_u8; 32];
    bytes[16..].copy_from_slice(job_id.as_bytes());
    FixedBytes::<32>::from(bytes)
}

pub fn private_key_from_env(env_name: &str) -> Result<String> {
    std::env::var(env_name)
        .with_context(|| format!("environment variable {env_name} is not set"))
        .map(|value| value.trim().to_string())
}

pub fn bundle_is_submitted(bundle: &ReceiptBundle) -> bool {
    bundle.chain_submission_status == ChainSubmissionStatus::Submitted
}

pub fn provider_address_from_id(provider_id: &str) -> Result<Address> {
    parse_address(provider_id, "provider_id")
}

pub fn verifier_address_from_id(verifier_id: &str) -> Result<Address> {
    parse_address(verifier_id, "verifier_id")
}

pub fn public_key_commitment_hex(raw_base64: &str) -> Result<String> {
    Ok(format!("{:#x}", base64_public_key_to_b256(raw_base64)?))
}

pub fn env_has_private_key(env_name: &str) -> bool {
    std::env::var(env_name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_to_b256_left_pads_uuid_bytes() {
        let job_id = Uuid::parse_str("018f7b7b-1f2d-74f8-b6a0-7df4f5ffb0f1").unwrap();
        let bytes = uuid_to_b256(job_id);
        assert_eq!(&bytes[..16], &[0_u8; 16]);
        assert_eq!(&bytes[16..], job_id.as_bytes());
    }

    #[test]
    fn signer_seed_commitment_matches_verifying_key_bytes() {
        let seed = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
        let commitment = signer_seed_to_b256(seed).unwrap();
        assert_eq!(commitment.as_slice().len(), 32);
    }

    #[test]
    fn hex_digest_requires_full_32_byte_payload() {
        assert!(hex_digest_to_b256("abcd").is_err());
    }

    #[test]
    fn retry_classifier_accepts_transport_errors() {
        let error = anyhow!(
            "error sending request\nclient error (SendRequest)\nreceived fatal alert: BadRecordMac"
        );
        assert!(is_retryable_rpc_error(&error));
    }

    #[test]
    fn retry_classifier_rejects_contract_reverts() {
        let error =
            anyhow!("server returned an error response: execution reverted: already exists");
        assert!(!is_retryable_rpc_error(&error));
    }

    #[test]
    fn pending_transaction_journal_round_trips_and_clears_records() {
        let temp = tempfile::tempdir().unwrap();
        let journal = PendingTransactionJournal::new(temp.path());
        let record = PendingTransactionRecord {
            chain_id: 2651420,
            operation_key: "receipt_bundle_submission:job/1".to_string(),
            operation_kind: "receipt_bundle_submission".to_string(),
            operation_subject: "job/1".to_string(),
            tx_hash: "0x1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
            raw_tx: Some("0x010203".to_string()),
            signer_address: "0x0000000000000000000000000000000000000001".to_string(),
            contract_address: "0x0000000000000000000000000000000000000002".to_string(),
            created_at_unix_seconds: 1,
        };

        journal.save(&record).unwrap();
        let loaded = journal
            .load(record.chain_id, &record.operation_key)
            .unwrap()
            .unwrap();
        assert_eq!(loaded, record);

        journal
            .clear(record.chain_id, &record.operation_key)
            .unwrap();
        assert!(journal
            .load(record.chain_id, &record.operation_key)
            .unwrap()
            .is_none());
    }

    #[test]
    fn pending_transaction_journal_sanitizes_operation_keys() {
        let temp = tempfile::tempdir().unwrap();
        let journal = PendingTransactionJournal::new(temp.path());
        let path = journal.path_for(1, "receipt:bundle/job#1");
        assert!(path.ends_with("1/receipt_bundle_job_1.json"));
    }

    #[tokio::test]
    async fn signer_transaction_lock_acquires_and_releases_directory() {
        let temp = tempfile::tempdir().unwrap();
        let signer = parse_address("0x0000000000000000000000000000000000000001", "signer").unwrap();
        let lock_dir = signer_lock_dir(temp.path(), 2651420, signer);
        {
            let _lock =
                SignerTransactionLock::acquire_in_root(temp.path().to_path_buf(), 2651420, signer)
                    .await
                    .unwrap();
            assert!(lock_dir.exists());
            assert!(lock_dir.join("lock.json").exists());
        }
        assert!(!lock_dir.exists());
    }

    #[test]
    fn signer_transaction_lock_rejects_active_lock() {
        let temp = tempfile::tempdir().unwrap();
        let signer = parse_address("0x0000000000000000000000000000000000000001", "signer").unwrap();
        let lock_dir = signer_lock_dir(temp.path(), 2651420, signer);
        let first = SignerTransactionLock::try_acquire(&lock_dir, 2651420, signer)
            .unwrap()
            .unwrap();
        assert!(
            SignerTransactionLock::try_acquire(&lock_dir, 2651420, signer)
                .unwrap()
                .is_none()
        );
        drop(first);
    }

    #[test]
    fn signer_transaction_lock_replaces_stale_lock() {
        let temp = tempfile::tempdir().unwrap();
        let signer = parse_address("0x0000000000000000000000000000000000000001", "signer").unwrap();
        let lock_dir = signer_lock_dir(temp.path(), 2651420, signer);
        fs::create_dir_all(&lock_dir).unwrap();
        write_signer_lock_record(
            &lock_dir,
            &SignerTransactionLockRecord {
                chain_id: 2651420,
                signer_address: signer.to_string(),
                pid: 1,
                acquired_at_unix_seconds: unix_timestamp_seconds()
                    .unwrap()
                    .saturating_sub(SIGNER_LOCK_STALE_SECONDS + 1),
            },
        )
        .unwrap();
        assert!(signer_lock_is_stale(&lock_dir).unwrap());
        assert!(
            SignerTransactionLock::try_acquire(&lock_dir, 2651420, signer)
                .unwrap()
                .is_none()
        );
        let lock = SignerTransactionLock::try_acquire(&lock_dir, 2651420, signer)
            .unwrap()
            .unwrap();
        drop(lock);
    }

    #[test]
    fn raw_transaction_hash_is_keccak256_of_signed_bytes() {
        let raw = hex_to_bytes("0x010203").unwrap();
        assert_eq!(raw_transaction_hash(&raw), keccak256([1_u8, 2, 3]));
    }

    #[test]
    fn gas_buffer_adds_twenty_percent_without_decreasing_estimate() {
        assert_eq!(gas_with_buffer(100), 120);
        assert_eq!(gas_with_buffer(0), 0);
        assert_eq!(gas_with_buffer(u64::MAX), u64::MAX);
    }

    #[test]
    fn native_escrow_attaches_native_value() {
        assert_eq!(
            escrow_native_value(Address::ZERO, U256::from(1_000_000_u64)),
            U256::from(1_000_000_u64)
        );
    }

    #[test]
    fn erc20_escrow_attaches_no_native_value() {
        let token = parse_address(
            "0x0000000000000000000000000000000000000001",
            "payment_token",
        )
        .unwrap();
        assert_eq!(
            escrow_native_value(token, U256::from(1_000_000_u64)),
            U256::ZERO
        );
    }

    #[test]
    fn private_key_signer_derives_expected_address() {
        let signer = signer_from_private_key(
            "0x59c6995e998f97a5a0044966f0945389d9e86dae25c9e8f73a64b6fb75e5d3f5",
        )
        .unwrap();
        assert_eq!(
            signer.address(),
            parse_address("0x1fa6c8b2c924ec320e276a1e3019e9803a3e97c0", "expected").unwrap()
        );
    }
}
