use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use chrono::Utc;
use osciris_core::{
    canonical_json_sha256, ChainSubmissionStatus, ChallengeRecord, ExecutionReceipt,
    JobAnnouncement, JobAssignment, JobClaim, JobSpec, MilestoneRecord, NodeIdentity, PeerPresence,
    ProviderCapability, ReceiptAvailability, ReceiptBundle, VerificationReceipt,
};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};
use tracing::info;

#[derive(Debug, Clone)]
pub struct ProtocolStore {
    pool: Pool<Sqlite>,
    db_path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredJob {
    pub job_id: String,
    pub status: String,
    pub evidence_dir: Option<String>,
    pub metrics_path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredPeerPresence {
    pub node_id: String,
    pub role: String,
    pub status: String,
    pub current_load: f64,
    pub active_job_count: i64,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredProviderCapability {
    pub node_id: String,
    pub gpu_model: String,
    pub gpu_count: i64,
    pub vram_gb: f64,
    pub status: String,
    pub current_load: f64,
    pub active_job_count: i64,
    pub pricing_hint: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredJobAnnouncement {
    pub job_id: String,
    pub submitter_node_id: String,
    pub job_type: String,
    pub privacy_mode: String,
    pub required_capability: String,
    pub announced_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredJobClaim {
    pub job_id: String,
    pub provider_node_id: String,
    pub claimed_at: String,
    pub claim_note: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredJobAssignment {
    pub job_id: String,
    pub assigned_provider_node_id: String,
    pub assigner_node_id: String,
    pub assignment_reason: String,
    pub assigned_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredReceiptAvailability {
    pub job_id: String,
    pub provider_node_id: String,
    pub execution_receipt_sha256: String,
    pub bundle_sha256: String,
    pub bundle_uri: String,
    pub announced_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredVerificationReceipt {
    pub verification_receipt_id: String,
    pub job_id: String,
    pub verifier_id: String,
    pub verification_status: String,
    pub receipt_sha256: String,
    pub bundle_sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredMilestoneRecord {
    pub milestone_id: String,
    pub job_id: String,
    pub job_type: String,
    pub title: String,
    pub quality_metric_name: String,
    pub quality_metric_value: f64,
    pub evidence_bundle_sha256: String,
    pub published_by: String,
    pub published_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredChallengeRecord {
    pub challenge_id: String,
    pub job_id: String,
    pub bundle_sha256: String,
    pub opened_by: String,
    pub reason_code: String,
    pub status: String,
    pub opened_at: String,
    pub resolved_at: Option<String>,
}

impl ProtocolStore {
    pub async fn open(protocol_root: &Path) -> Result<Self> {
        std::fs::create_dir_all(protocol_root)?;
        let db_path = protocol_root.join("protocol.db");
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let store = Self { pool, db_path };
        store.migrate().await?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub async fn upsert_job_spec(
        &self,
        job_spec: &JobSpec,
        status: &str,
        evidence_dir: Option<&Path>,
        metrics_path: Option<&str>,
    ) -> Result<()> {
        let job_json = serde_json::to_string(job_spec)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO jobs (
                job_id,
                job_type,
                status,
                job_spec_json,
                evidence_dir,
                metrics_path,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
            ON CONFLICT(job_id) DO UPDATE SET
                status = excluded.status,
                job_spec_json = excluded.job_spec_json,
                evidence_dir = excluded.evidence_dir,
                metrics_path = excluded.metrics_path,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(job_spec.job_id.to_string())
        .bind(enum_label(&job_spec.job_type)?)
        .bind(status)
        .bind(job_json)
        .bind(evidence_dir.map(|path| path.display().to_string()))
        .bind(metrics_path)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_execution_receipt(
        &self,
        receipt: &ExecutionReceipt,
        evidence_dir: &Path,
        metrics_path: &str,
    ) -> Result<()> {
        let receipt_json = serde_json::to_string(receipt)?;
        let receipt_sha256 = canonical_json_sha256(receipt)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO execution_receipts (
                receipt_id,
                job_id,
                provider_id,
                status,
                receipt_json,
                receipt_sha256,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(job_id) DO UPDATE SET
                receipt_id = excluded.receipt_id,
                provider_id = excluded.provider_id,
                status = excluded.status,
                receipt_json = excluded.receipt_json,
                receipt_sha256 = excluded.receipt_sha256,
                created_at = excluded.created_at
            "#,
        )
        .bind(receipt.receipt_id.to_string())
        .bind(receipt.job_id.to_string())
        .bind(&receipt.provider_id)
        .bind(enum_label(&receipt.status)?)
        .bind(receipt_json)
        .bind(receipt_sha256)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.upsert_job_status(
            &receipt.job_id.to_string(),
            &enum_label(&receipt.status)?,
            Some(evidence_dir),
            Some(metrics_path),
        )
        .await?;
        Ok(())
    }

    pub async fn record_receipt_bundle(&self, bundle: &ReceiptBundle) -> Result<()> {
        let bundle_json = serde_json::to_string(bundle)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO receipt_bundles (
                bundle_id,
                job_id,
                bundle_json,
                bundle_sha256,
                chain_submission_status,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
            ON CONFLICT(job_id) DO UPDATE SET
                bundle_json = excluded.bundle_json,
                bundle_sha256 = excluded.bundle_sha256,
                chain_submission_status = excluded.chain_submission_status,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(bundle.bundle_id.to_string())
        .bind(bundle.job_id.to_string())
        .bind(bundle_json)
        .bind(&bundle.bundle_sha256)
        .bind(enum_label(&bundle.chain_submission_status)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_receipt_bundle(&self, job_id: &str) -> Result<Option<ReceiptBundle>> {
        let row = sqlx::query(
            r#"
            SELECT bundle_json
            FROM receipt_bundles
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("bundle_json").as_str(),
        )?))
    }

    pub async fn record_chain_submission(
        &self,
        job_id: &str,
        receipt_registry_tx_hash: &str,
        escrow_tx_hash: &str,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO chain_submissions (
                job_id,
                receipt_registry_tx_hash,
                escrow_tx_hash,
                submitted_at
            ) VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(job_id) DO UPDATE SET
                receipt_registry_tx_hash = excluded.receipt_registry_tx_hash,
                escrow_tx_hash = excluded.escrow_tx_hash,
                submitted_at = excluded.submitted_at
            "#,
        )
        .bind(job_id)
        .bind(receipt_registry_tx_hash)
        .bind(escrow_tx_hash)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            UPDATE receipt_bundles
            SET chain_submission_status = ?2,
                updated_at = ?3
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .bind(enum_label(&ChainSubmissionStatus::Submitted)?)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_verification_receipt(&self, receipt: &VerificationReceipt) -> Result<()> {
        let receipt_json = serde_json::to_string(receipt)?;
        let receipt_sha256 = canonical_json_sha256(receipt)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO verification_receipts (
                verification_receipt_id,
                receipt_id,
                job_id,
                verifier_id,
                verification_status,
                receipt_json,
                receipt_sha256,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(job_id, verifier_id) DO UPDATE SET
                verification_receipt_id = excluded.verification_receipt_id,
                verification_status = excluded.verification_status,
                receipt_json = excluded.receipt_json,
                receipt_sha256 = excluded.receipt_sha256
            "#,
        )
        .bind(receipt.verification_receipt_id.to_string())
        .bind(receipt.receipt_id.to_string())
        .bind(receipt.job_id.to_string())
        .bind(&receipt.verifier_id)
        .bind(enum_label(&receipt.verification_status)?)
        .bind(receipt_json)
        .bind(receipt_sha256)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_verification_receipts_by_verifier(
        &self,
        verifier_id: &str,
    ) -> Result<Vec<VerificationReceipt>> {
        let rows = sqlx::query(
            r#"
            SELECT receipt_json
            FROM verification_receipts
            WHERE verifier_id = ?1
            ORDER BY created_at DESC, job_id ASC
            "#,
        )
        .bind(verifier_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<VerificationReceipt>(
                    row.get::<String, _>("receipt_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn load_verification_receipts_by_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<VerificationReceipt>> {
        let rows = sqlx::query(
            r#"
            SELECT receipt_json
            FROM verification_receipts
            WHERE job_id = ?1
            ORDER BY created_at DESC, verifier_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<VerificationReceipt>(
                    row.get::<String, _>("receipt_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_verification_receipts(&self) -> Result<Vec<StoredVerificationReceipt>> {
        let rows = sqlx::query(
            r#"
            SELECT verification_receipt_id, job_id, verifier_id, verification_status, receipt_sha256, receipt_json, created_at
            FROM verification_receipts
            ORDER BY created_at DESC, job_id ASC, verifier_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let receipt: VerificationReceipt =
                    serde_json::from_str(row.get::<String, _>("receipt_json").as_str())?;
                Ok(StoredVerificationReceipt {
                    verification_receipt_id: row.get::<String, _>("verification_receipt_id"),
                    job_id: row.get::<String, _>("job_id"),
                    verifier_id: row.get::<String, _>("verifier_id"),
                    verification_status: row.get::<String, _>("verification_status"),
                    receipt_sha256: row.get::<String, _>("receipt_sha256"),
                    bundle_sha256: receipt.bundle_sha256,
                    created_at: row.get::<String, _>("created_at"),
                })
            })
            .collect::<Result<Vec<_>>>()
    }

    pub async fn record_milestone(&self, milestone: &MilestoneRecord) -> Result<()> {
        let milestone_json = serde_json::to_string(milestone)?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO milestones (
                milestone_id,
                job_id,
                job_type,
                title,
                summary,
                contributing_node_ids_json,
                quality_metric_name,
                quality_metric_value,
                evidence_bundle_sha256,
                verification_receipt_sha256_list_json,
                published_by,
                published_at,
                signing_key_id,
                signature,
                milestone_json,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)
            ON CONFLICT(milestone_id) DO UPDATE SET
                job_id = excluded.job_id,
                job_type = excluded.job_type,
                title = excluded.title,
                summary = excluded.summary,
                contributing_node_ids_json = excluded.contributing_node_ids_json,
                quality_metric_name = excluded.quality_metric_name,
                quality_metric_value = excluded.quality_metric_value,
                evidence_bundle_sha256 = excluded.evidence_bundle_sha256,
                verification_receipt_sha256_list_json = excluded.verification_receipt_sha256_list_json,
                published_by = excluded.published_by,
                published_at = excluded.published_at,
                signing_key_id = excluded.signing_key_id,
                signature = excluded.signature,
                milestone_json = excluded.milestone_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(milestone.milestone_id.to_string())
        .bind(milestone.job_id.to_string())
        .bind(enum_label(&milestone.job_type)?)
        .bind(&milestone.title)
        .bind(&milestone.summary)
        .bind(serde_json::to_string(&milestone.contributing_node_ids)?)
        .bind(&milestone.quality_metric_name)
        .bind(milestone.quality_metric_value)
        .bind(&milestone.evidence_bundle_sha256)
        .bind(serde_json::to_string(&milestone.verification_receipt_sha256_list)?)
        .bind(&milestone.published_by)
        .bind(&milestone.published_at)
        .bind(&milestone.signing_key_id)
        .bind(&milestone.signature)
        .bind(milestone_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_milestone(&self, milestone_id: &str) -> Result<Option<MilestoneRecord>> {
        let row = sqlx::query(
            r#"
            SELECT milestone_json
            FROM milestones
            WHERE milestone_id = ?1
            "#,
        )
        .bind(milestone_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("milestone_json").as_str(),
        )?))
    }

    pub async fn load_milestones_by_job(&self, job_id: &str) -> Result<Vec<MilestoneRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT milestone_json
            FROM milestones
            WHERE job_id = ?1
            ORDER BY published_at DESC, milestone_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<MilestoneRecord>(
                    row.get::<String, _>("milestone_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_milestones(&self) -> Result<Vec<StoredMilestoneRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT milestone_id, job_id, job_type, title, quality_metric_name, quality_metric_value, evidence_bundle_sha256, published_by, published_at
            FROM milestones
            ORDER BY published_at DESC, job_id ASC, milestone_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredMilestoneRecord {
                milestone_id: row.get::<String, _>("milestone_id"),
                job_id: row.get::<String, _>("job_id"),
                job_type: row.get::<String, _>("job_type"),
                title: row.get::<String, _>("title"),
                quality_metric_name: row.get::<String, _>("quality_metric_name"),
                quality_metric_value: row.get::<f64, _>("quality_metric_value"),
                evidence_bundle_sha256: row.get::<String, _>("evidence_bundle_sha256"),
                published_by: row.get::<String, _>("published_by"),
                published_at: row.get::<String, _>("published_at"),
            })
            .collect())
    }

    pub async fn record_node_identity(&self, identity: &NodeIdentity) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO local_node_identity (
                node_id,
                role,
                ed25519_public_key_base64,
                evm_address,
                display_name,
                bootstrap_peers_json,
                identity_json,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
            ON CONFLICT(node_id) DO UPDATE SET
                role = excluded.role,
                ed25519_public_key_base64 = excluded.ed25519_public_key_base64,
                evm_address = excluded.evm_address,
                display_name = excluded.display_name,
                bootstrap_peers_json = excluded.bootstrap_peers_json,
                identity_json = excluded.identity_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&identity.node_id)
        .bind(enum_label(&identity.role)?)
        .bind(&identity.ed25519_public_key_base64)
        .bind(&identity.evm_address)
        .bind(&identity.display_name)
        .bind(serde_json::to_string(&identity.bootstrap_peers)?)
        .bind(serde_json::to_string(identity)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_node_identity(&self) -> Result<Option<NodeIdentity>> {
        let row = sqlx::query(
            r#"
            SELECT identity_json
            FROM local_node_identity
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("identity_json").as_str(),
        )?))
    }

    pub async fn record_peer_presence(&self, presence: &PeerPresence) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO peers (
                node_id,
                role,
                status,
                ed25519_public_key_base64,
                evm_address,
                listen_addrs_json,
                relay_capable,
                protocol_version,
                client_version,
                current_load,
                active_job_count,
                last_seen_at,
                capability_version,
                signature,
                presence_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(node_id) DO UPDATE SET
                role = excluded.role,
                status = excluded.status,
                ed25519_public_key_base64 = excluded.ed25519_public_key_base64,
                evm_address = excluded.evm_address,
                listen_addrs_json = excluded.listen_addrs_json,
                relay_capable = excluded.relay_capable,
                protocol_version = excluded.protocol_version,
                client_version = excluded.client_version,
                current_load = excluded.current_load,
                active_job_count = excluded.active_job_count,
                last_seen_at = excluded.last_seen_at,
                capability_version = excluded.capability_version,
                signature = excluded.signature,
                presence_json = excluded.presence_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&presence.node_id)
        .bind(enum_label(&presence.role)?)
        .bind(enum_label(&presence.status)?)
        .bind(&presence.ed25519_public_key_base64)
        .bind(&presence.evm_address)
        .bind(serde_json::to_string(&presence.listen_addrs)?)
        .bind(presence.relay_capable)
        .bind(&presence.protocol_version)
        .bind(&presence.client_version)
        .bind(presence.current_load)
        .bind(i64::from(presence.active_job_count))
        .bind(&presence.last_seen_at)
        .bind(&presence.capability_version)
        .bind(&presence.signature)
        .bind(serde_json::to_string(presence)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_peer_presences(&self) -> Result<Vec<StoredPeerPresence>> {
        let rows = sqlx::query(
            r#"
            SELECT node_id, role, status, current_load, active_job_count, last_seen_at
            FROM peers
            ORDER BY last_seen_at DESC, node_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredPeerPresence {
                node_id: row.get::<String, _>("node_id"),
                role: row.get::<String, _>("role"),
                status: row.get::<String, _>("status"),
                current_load: row.get::<f64, _>("current_load"),
                active_job_count: row.get::<i64, _>("active_job_count"),
                last_seen_at: row.get::<String, _>("last_seen_at"),
            })
            .collect())
    }

    pub async fn load_peer_presence(&self, node_id: &str) -> Result<Option<PeerPresence>> {
        let row = sqlx::query(
            r#"
            SELECT presence_json
            FROM peers
            WHERE node_id = ?1
            "#,
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("presence_json").as_str(),
        )?))
    }

    pub async fn record_provider_capability(&self, capability: &ProviderCapability) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO provider_capabilities (
                node_id,
                gpu_model,
                gpu_count,
                host_class,
                vram_gb,
                cuda_available,
                mps_available,
                supported_job_types_json,
                supported_runtimes_json,
                pricing_hint,
                current_load,
                active_job_count,
                status,
                capability_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(node_id) DO UPDATE SET
                gpu_model = excluded.gpu_model,
                gpu_count = excluded.gpu_count,
                host_class = excluded.host_class,
                vram_gb = excluded.vram_gb,
                cuda_available = excluded.cuda_available,
                mps_available = excluded.mps_available,
                supported_job_types_json = excluded.supported_job_types_json,
                supported_runtimes_json = excluded.supported_runtimes_json,
                pricing_hint = excluded.pricing_hint,
                current_load = excluded.current_load,
                active_job_count = excluded.active_job_count,
                status = excluded.status,
                capability_json = excluded.capability_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&capability.node_id)
        .bind(&capability.gpu_model)
        .bind(i64::from(capability.gpu_count))
        .bind(&capability.host_class)
        .bind(capability.vram_gb)
        .bind(capability.cuda_available)
        .bind(capability.mps_available)
        .bind(serde_json::to_string(&capability.supported_job_types)?)
        .bind(serde_json::to_string(&capability.supported_runtimes)?)
        .bind(&capability.pricing_hint)
        .bind(capability.current_load)
        .bind(i64::from(capability.active_job_count))
        .bind(enum_label(&capability.status)?)
        .bind(serde_json::to_string(capability)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_provider_capability(
        &self,
        node_id: &str,
    ) -> Result<Option<ProviderCapability>> {
        let row = sqlx::query(
            r#"
            SELECT capability_json
            FROM provider_capabilities
            WHERE node_id = ?1
            "#,
        )
        .bind(node_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("capability_json").as_str(),
        )?))
    }

    pub async fn list_provider_capabilities(&self) -> Result<Vec<StoredProviderCapability>> {
        let rows = sqlx::query(
            r#"
            SELECT node_id, gpu_model, gpu_count, vram_gb, status, current_load, active_job_count, pricing_hint, updated_at
            FROM provider_capabilities
            ORDER BY updated_at DESC, node_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredProviderCapability {
                node_id: row.get::<String, _>("node_id"),
                gpu_model: row.get::<String, _>("gpu_model"),
                gpu_count: row.get::<i64, _>("gpu_count"),
                vram_gb: row.get::<f64, _>("vram_gb"),
                status: row.get::<String, _>("status"),
                current_load: row.get::<f64, _>("current_load"),
                active_job_count: row.get::<i64, _>("active_job_count"),
                pricing_hint: row.get::<Option<String>, _>("pricing_hint"),
                updated_at: row.get::<String, _>("updated_at"),
            })
            .collect())
    }

    pub async fn record_job_announcement(&self, announcement: &JobAnnouncement) -> Result<()> {
        if announcement.job_id != announcement.job_spec.job_id {
            bail!(
                "job announcement id {} does not match embedded job spec id {}",
                announcement.job_id,
                announcement.job_spec.job_id
            );
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO job_announcements (
                job_id,
                submitter_node_id,
                submitter_ed25519_public_key_base64,
                job_type,
                privacy_mode,
                required_capability,
                estimated_runtime_class,
                payment_token,
                escrow_amount_atomic,
                required_verifier_count,
                announced_at,
                signature,
                announcement_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(job_id) DO UPDATE SET
                submitter_node_id = excluded.submitter_node_id,
                submitter_ed25519_public_key_base64 = excluded.submitter_ed25519_public_key_base64,
                job_type = excluded.job_type,
                privacy_mode = excluded.privacy_mode,
                required_capability = excluded.required_capability,
                estimated_runtime_class = excluded.estimated_runtime_class,
                payment_token = excluded.payment_token,
                escrow_amount_atomic = excluded.escrow_amount_atomic,
                required_verifier_count = excluded.required_verifier_count,
                announced_at = excluded.announced_at,
                signature = excluded.signature,
                announcement_json = excluded.announcement_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(announcement.job_id.to_string())
        .bind(&announcement.submitter_node_id)
        .bind(&announcement.submitter_ed25519_public_key_base64)
        .bind(enum_label(&announcement.job_type)?)
        .bind(enum_label(&announcement.privacy_mode)?)
        .bind(&announcement.required_capability)
        .bind(&announcement.estimated_runtime_class)
        .bind(&announcement.payment_token)
        .bind(&announcement.escrow_amount_atomic)
        .bind(i64::from(announcement.required_verifier_count))
        .bind(&announcement.announced_at)
        .bind(&announcement.signature)
        .bind(serde_json::to_string(announcement)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.upsert_job_spec(&announcement.job_spec, "announced", None, None)
            .await?;
        Ok(())
    }

    pub async fn load_job_spec(&self, job_id: &str) -> Result<Option<JobSpec>> {
        let row = sqlx::query(
            r#"
            SELECT job_spec_json
            FROM jobs
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("job_spec_json").as_str(),
        )?))
    }

    pub async fn load_job_announcement(&self, job_id: &str) -> Result<Option<JobAnnouncement>> {
        let row = sqlx::query(
            r#"
            SELECT announcement_json
            FROM job_announcements
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("announcement_json").as_str(),
        )?))
    }

    pub async fn load_job_announcements_by_submitter(
        &self,
        submitter_node_id: &str,
    ) -> Result<Vec<JobAnnouncement>> {
        let rows = sqlx::query(
            r#"
            SELECT announcement_json
            FROM job_announcements
            WHERE submitter_node_id = ?1
            ORDER BY announced_at DESC, job_id ASC
            "#,
        )
        .bind(submitter_node_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<JobAnnouncement>(
                    row.get::<String, _>("announcement_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_job_announcements(&self) -> Result<Vec<StoredJobAnnouncement>> {
        let rows = sqlx::query(
            r#"
            SELECT job_id, submitter_node_id, job_type, privacy_mode, required_capability, announced_at
            FROM job_announcements
            ORDER BY announced_at DESC, job_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredJobAnnouncement {
                job_id: row.get::<String, _>("job_id"),
                submitter_node_id: row.get::<String, _>("submitter_node_id"),
                job_type: row.get::<String, _>("job_type"),
                privacy_mode: row.get::<String, _>("privacy_mode"),
                required_capability: row.get::<String, _>("required_capability"),
                announced_at: row.get::<String, _>("announced_at"),
            })
            .collect())
    }

    pub async fn record_job_claim(&self, claim: &JobClaim) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO job_claims (
                job_id,
                provider_node_id,
                provider_ed25519_public_key_base64,
                claimed_at,
                claim_note,
                signature,
                claim_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(job_id, provider_node_id) DO UPDATE SET
                provider_ed25519_public_key_base64 = excluded.provider_ed25519_public_key_base64,
                claimed_at = excluded.claimed_at,
                claim_note = excluded.claim_note,
                signature = excluded.signature,
                claim_json = excluded.claim_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(claim.job_id.to_string())
        .bind(&claim.provider_node_id)
        .bind(&claim.provider_ed25519_public_key_base64)
        .bind(&claim.claimed_at)
        .bind(&claim.claim_note)
        .bind(&claim.signature)
        .bind(serde_json::to_string(claim)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_job_claims_by_provider(
        &self,
        provider_node_id: &str,
    ) -> Result<Vec<JobClaim>> {
        let rows = sqlx::query(
            r#"
            SELECT claim_json
            FROM job_claims
            WHERE provider_node_id = ?1
            ORDER BY claimed_at DESC, job_id ASC
            "#,
        )
        .bind(provider_node_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<JobClaim>(row.get::<String, _>("claim_json").as_str())
                    .map_err(Into::into)
            })
            .collect()
    }

    pub async fn load_job_claim(
        &self,
        job_id: &str,
        provider_node_id: &str,
    ) -> Result<Option<JobClaim>> {
        let row = sqlx::query(
            r#"
            SELECT claim_json
            FROM job_claims
            WHERE job_id = ?1 AND provider_node_id = ?2
            "#,
        )
        .bind(job_id)
        .bind(provider_node_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("claim_json").as_str(),
        )?))
    }

    pub async fn load_job_claims_by_job(&self, job_id: &str) -> Result<Vec<JobClaim>> {
        let rows = sqlx::query(
            r#"
            SELECT claim_json
            FROM job_claims
            WHERE job_id = ?1
            ORDER BY claimed_at DESC, provider_node_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<JobClaim>(row.get::<String, _>("claim_json").as_str())
                    .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_job_claims(&self) -> Result<Vec<StoredJobClaim>> {
        let rows = sqlx::query(
            r#"
            SELECT job_id, provider_node_id, claimed_at, claim_note
            FROM job_claims
            ORDER BY claimed_at DESC, job_id ASC, provider_node_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredJobClaim {
                job_id: row.get::<String, _>("job_id"),
                provider_node_id: row.get::<String, _>("provider_node_id"),
                claimed_at: row.get::<String, _>("claimed_at"),
                claim_note: row.get::<Option<String>, _>("claim_note"),
            })
            .collect())
    }

    pub async fn record_job_assignment(&self, assignment: &JobAssignment) -> Result<()> {
        let assignment_json = serde_json::to_string(assignment)?;
        if let Some(row) = sqlx::query(
            r#"
            SELECT assignment_json
            FROM job_assignments
            WHERE job_id = ?1
            "#,
        )
        .bind(assignment.job_id.to_string())
        .fetch_optional(&self.pool)
        .await?
        {
            let existing_json = row.get::<String, _>("assignment_json");
            if existing_json == assignment_json {
                return Ok(());
            }
            bail!(
                "conflicting assignment for job {}; existing assignment must be replayed byte-identically",
                assignment.job_id
            );
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO job_assignments (
                job_id,
                assigned_provider_node_id,
                assigner_node_id,
                assigner_ed25519_public_key_base64,
                assignment_reason,
                assigned_at,
                signature,
                assignment_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(assignment.job_id.to_string())
        .bind(&assignment.assigned_provider_node_id)
        .bind(&assignment.assigner_node_id)
        .bind(&assignment.assigner_ed25519_public_key_base64)
        .bind(&assignment.assignment_reason)
        .bind(&assignment.assigned_at)
        .bind(&assignment.signature)
        .bind(assignment_json)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_job_assignment(&self, job_id: &str) -> Result<Option<JobAssignment>> {
        let row = sqlx::query(
            r#"
            SELECT assignment_json
            FROM job_assignments
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("assignment_json").as_str(),
        )?))
    }

    pub async fn load_job_assignments_by_provider(
        &self,
        provider_node_id: &str,
    ) -> Result<Vec<JobAssignment>> {
        let rows = sqlx::query(
            r#"
            SELECT assignment_json
            FROM job_assignments
            WHERE assigned_provider_node_id = ?1
            ORDER BY assigned_at DESC, job_id ASC
            "#,
        )
        .bind(provider_node_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<JobAssignment>(
                    row.get::<String, _>("assignment_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_job_assignment_objects(&self) -> Result<Vec<JobAssignment>> {
        let rows = sqlx::query(
            r#"
            SELECT assignment_json
            FROM job_assignments
            ORDER BY assigned_at DESC, job_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<JobAssignment>(
                    row.get::<String, _>("assignment_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_job_assignments(&self) -> Result<Vec<StoredJobAssignment>> {
        let rows = sqlx::query(
            r#"
            SELECT job_id, assigned_provider_node_id, assigner_node_id, assignment_reason, assigned_at
            FROM job_assignments
            ORDER BY assigned_at DESC, job_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredJobAssignment {
                job_id: row.get::<String, _>("job_id"),
                assigned_provider_node_id: row.get::<String, _>("assigned_provider_node_id"),
                assigner_node_id: row.get::<String, _>("assigner_node_id"),
                assignment_reason: row.get::<String, _>("assignment_reason"),
                assigned_at: row.get::<String, _>("assigned_at"),
            })
            .collect())
    }

    pub async fn record_challenge_record(&self, challenge: &ChallengeRecord) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO challenge_records (
                challenge_id,
                job_id,
                bundle_sha256,
                opened_by,
                opened_by_ed25519_public_key_base64,
                reason_code,
                reason_detail,
                opened_at,
                status,
                resolved_by,
                resolved_by_ed25519_public_key_base64,
                resolved_at,
                resolution_note,
                signature,
                challenge_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(challenge_id) DO UPDATE SET
                status = excluded.status,
                resolved_by = excluded.resolved_by,
                resolved_by_ed25519_public_key_base64 = excluded.resolved_by_ed25519_public_key_base64,
                resolved_at = excluded.resolved_at,
                resolution_note = excluded.resolution_note,
                signature = excluded.signature,
                challenge_json = excluded.challenge_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(challenge.challenge_id.to_string())
        .bind(challenge.job_id.to_string())
        .bind(&challenge.bundle_sha256)
        .bind(&challenge.opened_by)
        .bind(&challenge.opened_by_ed25519_public_key_base64)
        .bind(enum_label(&challenge.reason_code)?)
        .bind(&challenge.reason_detail)
        .bind(&challenge.opened_at)
        .bind(enum_label(&challenge.status)?)
        .bind(&challenge.resolved_by)
        .bind(&challenge.resolved_by_ed25519_public_key_base64)
        .bind(&challenge.resolved_at)
        .bind(&challenge.resolution_note)
        .bind(&challenge.signature)
        .bind(serde_json::to_string(challenge)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_challenge_record(
        &self,
        challenge_id: &str,
    ) -> Result<Option<ChallengeRecord>> {
        let row = sqlx::query(
            r#"
            SELECT challenge_json
            FROM challenge_records
            WHERE challenge_id = ?1
            "#,
        )
        .bind(challenge_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("challenge_json").as_str(),
        )?))
    }

    pub async fn load_challenge_records_by_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<ChallengeRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT challenge_json
            FROM challenge_records
            WHERE job_id = ?1
            ORDER BY opened_at DESC, challenge_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<ChallengeRecord>(
                    row.get::<String, _>("challenge_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_challenge_record_objects(&self) -> Result<Vec<ChallengeRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT challenge_json
            FROM challenge_records
            ORDER BY opened_at DESC, job_id ASC, challenge_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<ChallengeRecord>(
                    row.get::<String, _>("challenge_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_challenge_records(&self) -> Result<Vec<StoredChallengeRecord>> {
        let rows = sqlx::query(
            r#"
            SELECT challenge_id, job_id, bundle_sha256, opened_by, reason_code, status, opened_at, resolved_at
            FROM challenge_records
            ORDER BY opened_at DESC, job_id ASC, challenge_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredChallengeRecord {
                challenge_id: row.get::<String, _>("challenge_id"),
                job_id: row.get::<String, _>("job_id"),
                bundle_sha256: row.get::<String, _>("bundle_sha256"),
                opened_by: row.get::<String, _>("opened_by"),
                reason_code: row.get::<String, _>("reason_code"),
                status: row.get::<String, _>("status"),
                opened_at: row.get::<String, _>("opened_at"),
                resolved_at: row.get::<Option<String>, _>("resolved_at"),
            })
            .collect())
    }

    pub async fn record_receipt_availability(
        &self,
        availability: &ReceiptAvailability,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            INSERT INTO receipt_availability (
                job_id,
                provider_node_id,
                provider_ed25519_public_key_base64,
                execution_receipt_sha256,
                bundle_sha256,
                bundle_uri,
                announced_at,
                signature,
                availability_json,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(job_id, provider_node_id) DO UPDATE SET
                provider_ed25519_public_key_base64 = excluded.provider_ed25519_public_key_base64,
                execution_receipt_sha256 = excluded.execution_receipt_sha256,
                bundle_sha256 = excluded.bundle_sha256,
                bundle_uri = excluded.bundle_uri,
                announced_at = excluded.announced_at,
                signature = excluded.signature,
                availability_json = excluded.availability_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(availability.job_id.to_string())
        .bind(&availability.provider_node_id)
        .bind(&availability.provider_ed25519_public_key_base64)
        .bind(&availability.execution_receipt_sha256)
        .bind(&availability.bundle_sha256)
        .bind(&availability.bundle_uri)
        .bind(&availability.announced_at)
        .bind(&availability.signature)
        .bind(serde_json::to_string(availability)?)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_receipt_availability_by_provider(
        &self,
        provider_node_id: &str,
    ) -> Result<Vec<ReceiptAvailability>> {
        let rows = sqlx::query(
            r#"
            SELECT availability_json
            FROM receipt_availability
            WHERE provider_node_id = ?1
            ORDER BY announced_at DESC, job_id ASC
            "#,
        )
        .bind(provider_node_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<ReceiptAvailability>(
                    row.get::<String, _>("availability_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn load_receipt_availability(
        &self,
        job_id: &str,
        provider_node_id: &str,
    ) -> Result<Option<ReceiptAvailability>> {
        let row = sqlx::query(
            r#"
            SELECT availability_json
            FROM receipt_availability
            WHERE job_id = ?1 AND provider_node_id = ?2
            "#,
        )
        .bind(job_id)
        .bind(provider_node_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(
            row.get::<String, _>("availability_json").as_str(),
        )?))
    }

    pub async fn load_receipt_availability_by_job(
        &self,
        job_id: &str,
    ) -> Result<Vec<ReceiptAvailability>> {
        let rows = sqlx::query(
            r#"
            SELECT availability_json
            FROM receipt_availability
            WHERE job_id = ?1
            ORDER BY announced_at DESC, provider_node_id ASC
            "#,
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<ReceiptAvailability>(
                    row.get::<String, _>("availability_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_receipt_availability_objects(&self) -> Result<Vec<ReceiptAvailability>> {
        let rows = sqlx::query(
            r#"
            SELECT availability_json
            FROM receipt_availability
            ORDER BY announced_at DESC, job_id ASC, provider_node_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                serde_json::from_str::<ReceiptAvailability>(
                    row.get::<String, _>("availability_json").as_str(),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    pub async fn list_receipt_availability(&self) -> Result<Vec<StoredReceiptAvailability>> {
        let rows = sqlx::query(
            r#"
            SELECT job_id, provider_node_id, execution_receipt_sha256, bundle_sha256, bundle_uri, announced_at
            FROM receipt_availability
            ORDER BY announced_at DESC, job_id ASC, provider_node_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredReceiptAvailability {
                job_id: row.get::<String, _>("job_id"),
                provider_node_id: row.get::<String, _>("provider_node_id"),
                execution_receipt_sha256: row.get::<String, _>("execution_receipt_sha256"),
                bundle_sha256: row.get::<String, _>("bundle_sha256"),
                bundle_uri: row.get::<String, _>("bundle_uri"),
                announced_at: row.get::<String, _>("announced_at"),
            })
            .collect())
    }

    pub async fn list_jobs(&self) -> Result<Vec<StoredJob>> {
        let rows = sqlx::query(
            r#"
            SELECT job_id, status, evidence_dir, metrics_path
            FROM jobs
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| StoredJob {
                job_id: row.get::<String, _>("job_id"),
                status: row.get::<String, _>("status"),
                evidence_dir: row.get::<Option<String>, _>("evidence_dir"),
                metrics_path: row.get::<Option<String>, _>("metrics_path"),
            })
            .collect())
    }

    pub async fn verification_receipt_count(&self, job_id: &str) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*) AS count
            FROM verification_receipts
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get::<i64, _>("count"))
    }

    pub async fn load_bundle_hashes(&self, job_id: &str) -> Result<Vec<String>> {
        let row = sqlx::query(
            r#"
            SELECT bundle_json
            FROM receipt_bundles
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(vec![]);
        };
        let payload: Value = serde_json::from_str(row.get::<String, _>("bundle_json").as_str())?;
        let hashes = payload
            .get("verification_receipt_sha256_list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(ToOwned::to_owned))
            .collect();
        Ok(hashes)
    }

    async fn upsert_job_status(
        &self,
        job_id: &str,
        status: &str,
        evidence_dir: Option<&Path>,
        metrics_path: Option<&str>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE jobs
            SET status = ?2,
                evidence_dir = COALESCE(?3, evidence_dir),
                metrics_path = COALESCE(?4, metrics_path),
                updated_at = ?5
            WHERE job_id = ?1
            "#,
        )
        .bind(job_id)
        .bind(status)
        .bind(evidence_dir.map(|path| path.display().to_string()))
        .bind(metrics_path)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn migrate(&self) -> Result<()> {
        info!("migrating protocol store {}", self.db_path.display());
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS local_node_identity (
                node_id TEXT PRIMARY KEY,
                role TEXT NOT NULL,
                ed25519_public_key_base64 TEXT NOT NULL,
                evm_address TEXT,
                display_name TEXT NOT NULL,
                bootstrap_peers_json TEXT NOT NULL,
                identity_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS peers (
                node_id TEXT PRIMARY KEY,
                role TEXT NOT NULL,
                status TEXT NOT NULL,
                ed25519_public_key_base64 TEXT NOT NULL,
                evm_address TEXT,
                listen_addrs_json TEXT NOT NULL,
                relay_capable INTEGER NOT NULL,
                protocol_version TEXT NOT NULL,
                client_version TEXT NOT NULL,
                current_load REAL NOT NULL,
                active_job_count INTEGER NOT NULL,
                last_seen_at TEXT NOT NULL,
                capability_version TEXT,
                signature TEXT NOT NULL,
                presence_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS provider_capabilities (
                node_id TEXT PRIMARY KEY,
                gpu_model TEXT NOT NULL,
                gpu_count INTEGER NOT NULL,
                host_class TEXT NOT NULL,
                vram_gb REAL NOT NULL,
                cuda_available INTEGER NOT NULL,
                mps_available INTEGER NOT NULL,
                supported_job_types_json TEXT NOT NULL,
                supported_runtimes_json TEXT NOT NULL,
                pricing_hint TEXT,
                current_load REAL NOT NULL,
                active_job_count INTEGER NOT NULL,
                status TEXT NOT NULL,
                capability_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS job_announcements (
                job_id TEXT PRIMARY KEY,
                submitter_node_id TEXT NOT NULL,
                submitter_ed25519_public_key_base64 TEXT NOT NULL,
                job_type TEXT NOT NULL,
                privacy_mode TEXT NOT NULL,
                required_capability TEXT NOT NULL,
                estimated_runtime_class TEXT NOT NULL,
                payment_token TEXT NOT NULL,
                escrow_amount_atomic TEXT NOT NULL,
                required_verifier_count INTEGER NOT NULL,
                announced_at TEXT NOT NULL,
                signature TEXT NOT NULL,
                announcement_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS job_claims (
                job_id TEXT NOT NULL,
                provider_node_id TEXT NOT NULL,
                provider_ed25519_public_key_base64 TEXT NOT NULL,
                claimed_at TEXT NOT NULL,
                claim_note TEXT,
                signature TEXT NOT NULL,
                claim_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(job_id, provider_node_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS job_assignments (
                job_id TEXT PRIMARY KEY,
                assigned_provider_node_id TEXT NOT NULL,
                assigner_node_id TEXT NOT NULL,
                assigner_ed25519_public_key_base64 TEXT NOT NULL,
                assignment_reason TEXT NOT NULL,
                assigned_at TEXT NOT NULL,
                signature TEXT NOT NULL,
                assignment_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS receipt_availability (
                job_id TEXT NOT NULL,
                provider_node_id TEXT NOT NULL,
                provider_ed25519_public_key_base64 TEXT NOT NULL,
                execution_receipt_sha256 TEXT NOT NULL,
                bundle_sha256 TEXT NOT NULL,
                bundle_uri TEXT NOT NULL,
                announced_at TEXT NOT NULL,
                signature TEXT NOT NULL,
                availability_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(job_id, provider_node_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS challenge_records (
                challenge_id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                bundle_sha256 TEXT NOT NULL,
                opened_by TEXT NOT NULL,
                opened_by_ed25519_public_key_base64 TEXT NOT NULL,
                reason_code TEXT NOT NULL,
                reason_detail TEXT NOT NULL,
                opened_at TEXT NOT NULL,
                status TEXT NOT NULL,
                resolved_by TEXT,
                resolved_by_ed25519_public_key_base64 TEXT,
                resolved_at TEXT,
                resolution_note TEXT,
                signature TEXT NOT NULL,
                challenge_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS jobs (
                job_id TEXT PRIMARY KEY,
                job_type TEXT NOT NULL,
                status TEXT NOT NULL,
                job_spec_json TEXT NOT NULL,
                evidence_dir TEXT,
                metrics_path TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS execution_receipts (
                receipt_id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL UNIQUE,
                provider_id TEXT NOT NULL,
                status TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                receipt_sha256 TEXT NOT NULL,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS verification_receipts (
                verification_receipt_id TEXT PRIMARY KEY,
                receipt_id TEXT NOT NULL,
                job_id TEXT NOT NULL,
                verifier_id TEXT NOT NULL,
                verification_status TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                receipt_sha256 TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(job_id, verifier_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS receipt_bundles (
                bundle_id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL UNIQUE,
                bundle_json TEXT NOT NULL,
                bundle_sha256 TEXT NOT NULL,
                chain_submission_status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS milestones (
                milestone_id TEXT PRIMARY KEY,
                job_id TEXT NOT NULL,
                job_type TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT NOT NULL,
                contributing_node_ids_json TEXT NOT NULL,
                quality_metric_name TEXT NOT NULL,
                quality_metric_value REAL NOT NULL,
                evidence_bundle_sha256 TEXT NOT NULL,
                verification_receipt_sha256_list_json TEXT NOT NULL,
                published_by TEXT NOT NULL,
                published_at TEXT NOT NULL,
                signing_key_id TEXT NOT NULL,
                signature TEXT NOT NULL,
                milestone_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chain_submissions (
                job_id TEXT PRIMARY KEY,
                receipt_registry_tx_hash TEXT NOT NULL,
                escrow_tx_hash TEXT NOT NULL,
                submitted_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn enum_label<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        ChallengeReasonCode, ChallengeStatus, JobAssignment, JobType, MilestoneRecord, NodeRole,
        NodeStatus, PrivacyMode, PrivacyPolicy, VerificationChecks, VerificationStatus,
    };
    use uuid::Uuid;

    #[tokio::test]
    async fn store_round_trips_network_state() {
        let temp = tempfile::tempdir().unwrap();
        let store = ProtocolStore::open(temp.path()).await.unwrap();

        let identity = NodeIdentity {
            node_id: "node-provider-1".to_string(),
            role: NodeRole::Provider,
            ed25519_public_key_base64: "provider-public-key".to_string(),
            evm_address: Some("0x1111111111111111111111111111111111111111".to_string()),
            display_name: "provider-1".to_string(),
            bootstrap_peers: vec!["/dns/bootstrap/tcp/9000".to_string()],
            created_at: "2026-06-04T00:00:00Z".to_string(),
        };
        store.record_node_identity(&identity).await.unwrap();
        let loaded = store.load_node_identity().await.unwrap().unwrap();
        assert_eq!(loaded, identity);

        let presence = PeerPresence {
            node_id: "node-provider-2".to_string(),
            role: NodeRole::Provider,
            ed25519_public_key_base64: "peer-public-key".to_string(),
            evm_address: Some("0x2222222222222222222222222222222222222222".to_string()),
            listen_addrs: vec!["/ip4/10.0.0.8/tcp/9001".to_string()],
            relay_capable: true,
            protocol_version: "0.1.0".to_string(),
            client_version: "0.1.0".to_string(),
            status: NodeStatus::OnlineIdle,
            current_load: 0.0,
            active_job_count: 0,
            last_seen_at: "2026-06-04T00:00:15Z".to_string(),
            capability_version: Some("cap-v1".to_string()),
            signature: "signature".to_string(),
        };
        store.record_peer_presence(&presence).await.unwrap();
        let peers = store.list_peer_presences().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_id, presence.node_id);
        assert_eq!(peers[0].status, "online_idle");
        let loaded_presence = store
            .load_peer_presence(&presence.node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_presence, presence);

        let capability = ProviderCapability {
            node_id: presence.node_id.clone(),
            ed25519_public_key_base64: presence.ed25519_public_key_base64.clone(),
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
            updated_at: "2026-06-04T00:00:15Z".to_string(),
            signature: "cap-signature".to_string(),
        };
        store.record_provider_capability(&capability).await.unwrap();
        let capabilities = store.list_provider_capabilities().await.unwrap();
        assert_eq!(capabilities.len(), 1);
        assert_eq!(capabilities[0].node_id, capability.node_id);
        assert_eq!(capabilities[0].gpu_model, capability.gpu_model);
        let loaded_capability = store
            .load_provider_capability(&capability.node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_capability, capability);

        let announced_job_spec = JobSpec {
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
            created_at: "2026-06-04T00:00:30Z".to_string(),
        };
        let announcement = JobAnnouncement {
            job_id: announced_job_spec.job_id,
            job_spec: announced_job_spec.clone(),
            submitter_node_id: "enterprise-node-1".to_string(),
            submitter_ed25519_public_key_base64: "enterprise-public-key".to_string(),
            job_type: JobType::LlmLoraEconomics,
            privacy_mode: PrivacyMode::DspPrepared,
            required_capability: "gpu>=24gb".to_string(),
            estimated_runtime_class: "short".to_string(),
            payment_token: "USDC_TEST".to_string(),
            escrow_amount_atomic: "1000000".to_string(),
            required_verifier_count: 1,
            announced_at: "2026-06-04T00:00:30Z".to_string(),
            signature: "announcement-signature".to_string(),
        };
        store.record_job_announcement(&announcement).await.unwrap();
        let announcements = store.list_job_announcements().await.unwrap();
        assert_eq!(announcements.len(), 1);
        assert_eq!(
            announcements[0].submitter_node_id,
            announcement.submitter_node_id
        );
        let submitter_announcements = store
            .load_job_announcements_by_submitter(&announcement.submitter_node_id)
            .await
            .unwrap();
        assert_eq!(submitter_announcements, vec![announcement.clone()]);
        let loaded_announcement = store
            .load_job_announcement(&announcement.job_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_announcement, announcement);
        let loaded_job_spec = store
            .load_job_spec(&announcement.job_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_job_spec, announced_job_spec);

        let claim = JobClaim {
            job_id: submitter_announcements[0].job_id,
            provider_node_id: capability.node_id.clone(),
            provider_ed25519_public_key_base64: capability.ed25519_public_key_base64.clone(),
            claimed_at: "2026-06-04T00:00:45Z".to_string(),
            claim_note: Some("ready".to_string()),
            signature: "claim-signature".to_string(),
        };
        store.record_job_claim(&claim).await.unwrap();
        let claims = store.list_job_claims().await.unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].job_id, claim.job_id.to_string());
        assert_eq!(claims[0].provider_node_id, claim.provider_node_id);
        let provider_claims = store
            .load_job_claims_by_provider(&claim.provider_node_id)
            .await
            .unwrap();
        assert_eq!(provider_claims, vec![claim.clone()]);
        let job_claims = store
            .load_job_claims_by_job(&claim.job_id.to_string())
            .await
            .unwrap();
        assert_eq!(job_claims, vec![claim]);
        let loaded_claim = store
            .load_job_claim(
                &provider_claims[0].job_id.to_string(),
                &provider_claims[0].provider_node_id,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_claim, provider_claims[0]);

        let assignment = JobAssignment {
            job_id: provider_claims[0].job_id,
            assigned_provider_node_id: provider_claims[0].provider_node_id.clone(),
            assigner_node_id: "enterprise-node-1".to_string(),
            assigner_ed25519_public_key_base64: "enterprise-public-key".to_string(),
            assignment_reason: "manual_assignment".to_string(),
            assigned_at: "2026-06-04T00:00:50Z".to_string(),
            signature: "assignment-signature".to_string(),
        };
        store.record_job_assignment(&assignment).await.unwrap();
        let loaded_assignment = store
            .load_job_assignment(&assignment.job_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_assignment, assignment);
        let provider_assignments = store
            .load_job_assignments_by_provider(&assignment.assigned_provider_node_id)
            .await
            .unwrap();
        assert_eq!(provider_assignments, vec![assignment.clone()]);
        let assignment_objects = store.list_job_assignment_objects().await.unwrap();
        assert_eq!(assignment_objects, vec![assignment.clone()]);
        let assignment_rows = store.list_job_assignments().await.unwrap();
        assert_eq!(assignment_rows.len(), 1);
        assert_eq!(
            assignment_rows[0].assigned_provider_node_id,
            assignment.assigned_provider_node_id
        );
        store.record_job_assignment(&assignment).await.unwrap();
        let conflicting_assignment = JobAssignment {
            assigned_provider_node_id: "different-provider".to_string(),
            signature: "different-signature".to_string(),
            ..assignment.clone()
        };
        assert!(store
            .record_job_assignment(&conflicting_assignment)
            .await
            .is_err());

        let availability = ReceiptAvailability {
            job_id: provider_claims[0].job_id,
            provider_node_id: provider_claims[0].provider_node_id.clone(),
            provider_ed25519_public_key_base64: capability.ed25519_public_key_base64.clone(),
            execution_receipt_sha256: "a".repeat(64),
            bundle_sha256: "b".repeat(64),
            bundle_uri: "file:///tmp/evidence".to_string(),
            announced_at: "2026-06-04T00:01:00Z".to_string(),
            signature: "availability-signature".to_string(),
        };
        store
            .record_receipt_availability(&availability)
            .await
            .unwrap();
        let availability_rows = store.list_receipt_availability().await.unwrap();
        assert_eq!(availability_rows.len(), 1);
        assert_eq!(availability_rows[0].job_id, availability.job_id.to_string());
        let provider_availability = store
            .load_receipt_availability_by_provider(&availability.provider_node_id)
            .await
            .unwrap();
        assert_eq!(provider_availability, vec![availability.clone()]);
        let loaded_availability = store
            .load_receipt_availability(
                &availability.job_id.to_string(),
                &availability.provider_node_id,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_availability, availability);
        let job_availability = store
            .load_receipt_availability_by_job(&availability.job_id.to_string())
            .await
            .unwrap();
        assert_eq!(job_availability, vec![availability.clone()]);
        let availability_objects = store.list_receipt_availability_objects().await.unwrap();
        assert_eq!(availability_objects, vec![availability]);

        let milestone = MilestoneRecord {
            milestone_id: Uuid::now_v7(),
            job_id: loaded_availability.job_id,
            job_type: JobType::InferenceEconomics,
            title: "Community inference milestone".to_string(),
            summary: "GPU contributors published a shared inference quality checkpoint."
                .to_string(),
            contributing_node_ids: vec!["provider-a".to_string(), "verifier-1".to_string()],
            quality_metric_name: "quality_retention".to_string(),
            quality_metric_value: 0.91,
            evidence_bundle_sha256: loaded_availability.bundle_sha256.clone(),
            verification_receipt_sha256_list: vec!["c".repeat(64)],
            published_by: "enterprise-node-1".to_string(),
            published_at: "2026-06-04T00:01:30Z".to_string(),
            signing_key_id: "enterprise-key".to_string(),
            signature: "milestone-signature".to_string(),
        };
        store.record_milestone(&milestone).await.unwrap();
        let loaded_milestone = store
            .load_milestone(&milestone.milestone_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_milestone, milestone);
        let job_milestones = store
            .load_milestones_by_job(&milestone.job_id.to_string())
            .await
            .unwrap();
        assert_eq!(job_milestones, vec![milestone.clone()]);
        let milestone_rows = store.list_milestones().await.unwrap();
        assert_eq!(milestone_rows.len(), 1);
        assert_eq!(
            milestone_rows[0].milestone_id,
            milestone.milestone_id.to_string()
        );

        let mut challenge = ChallengeRecord {
            challenge_id: Uuid::now_v7(),
            job_id: loaded_availability.job_id,
            bundle_sha256: loaded_availability.bundle_sha256.clone(),
            opened_by: "verifier-node-1".to_string(),
            opened_by_ed25519_public_key_base64: "verifier-public-key".to_string(),
            reason_code: ChallengeReasonCode::ForbiddenJobTransition,
            reason_detail: "settlement blocked for test".to_string(),
            opened_at: "2026-06-04T00:01:10Z".to_string(),
            status: ChallengeStatus::Open,
            resolved_by: None,
            resolved_by_ed25519_public_key_base64: None,
            resolved_at: None,
            resolution_note: None,
            signature: "challenge-signature".to_string(),
        };
        store.record_challenge_record(&challenge).await.unwrap();
        let loaded_challenge = store
            .load_challenge_record(&challenge.challenge_id.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded_challenge, challenge);
        challenge.status = ChallengeStatus::ResolvedRejected;
        challenge.resolved_by = Some("enterprise-node-1".to_string());
        challenge.resolved_by_ed25519_public_key_base64 = Some("enterprise-public-key".to_string());
        challenge.resolved_at = Some("2026-06-04T00:01:30Z".to_string());
        challenge.resolution_note = Some("challenge rejected".to_string());
        challenge.signature = "resolved-challenge-signature".to_string();
        store.record_challenge_record(&challenge).await.unwrap();
        let job_challenges = store
            .load_challenge_records_by_job(&challenge.job_id.to_string())
            .await
            .unwrap();
        assert_eq!(job_challenges, vec![challenge]);
        let challenge_rows = store.list_challenge_records().await.unwrap();
        assert_eq!(challenge_rows.len(), 1);
        assert_eq!(challenge_rows[0].status, "resolved_rejected");

        let verification_receipt = VerificationReceipt {
            verification_receipt_id: Uuid::now_v7(),
            receipt_id: Uuid::now_v7(),
            job_id: loaded_availability.job_id,
            verifier_id: "verifier-node-1".to_string(),
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
            bundle_sha256: loaded_availability.bundle_sha256.clone(),
            signature: "verification-signature".to_string(),
            signing_key_id: "verifier-key-1".to_string(),
        };
        store
            .record_verification_receipt(&verification_receipt)
            .await
            .unwrap();
        let verification_rows = store.list_verification_receipts().await.unwrap();
        assert_eq!(verification_rows.len(), 1);
        assert_eq!(
            verification_rows[0].verification_receipt_id,
            verification_receipt.verification_receipt_id.to_string()
        );
        assert_eq!(verification_rows[0].bundle_sha256, "b".repeat(64));
        let verifier_receipts = store
            .load_verification_receipts_by_verifier(&verification_receipt.verifier_id)
            .await
            .unwrap();
        assert_eq!(verifier_receipts, vec![verification_receipt.clone()]);
        let job_verification_receipts = store
            .load_verification_receipts_by_job(&verification_receipt.job_id.to_string())
            .await
            .unwrap();
        assert_eq!(job_verification_receipts, vec![verification_receipt]);
    }
}
