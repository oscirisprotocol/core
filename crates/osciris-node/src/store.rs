use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use osciris_core::{
    canonical_json_sha256, ExecutionReceipt, JobSpec, ReceiptBundle, VerificationReceipt,
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

#[derive(Debug, Clone)]
pub struct StoredJob {
    pub job_id: String,
    pub status: String,
    pub evidence_dir: Option<String>,
    pub metrics_path: Option<String>,
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
            ON CONFLICT(receipt_id) DO UPDATE SET
                status = excluded.status,
                receipt_json = excluded.receipt_json,
                receipt_sha256 = excluded.receipt_sha256
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
        Ok(())
    }
}

fn enum_label<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_string())
}
