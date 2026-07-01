use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use osciris_core::{
    ChallengeRecord, ChallengeStatus, NodeStatus, ProviderCapability, ReceiptBundle,
    VerificationReceipt, VerificationStatus,
};
use serde::Serialize;
use uuid::Uuid;

use crate::store::{
    StoredJobAssignment, StoredJobClaim, StoredPeerPresence, StoredProviderCapability,
    StoredReceiptAvailability,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuorumState {
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QuorumStatusReport {
    pub job_id: Uuid,
    pub bundle_sha256: Option<String>,
    pub required_verifier_count: u8,
    pub accepted_verifier_count: usize,
    pub rejected_verifier_count: usize,
    pub accepted_verifier_ids: Vec<String>,
    pub rejected_verifier_ids: Vec<String>,
    pub status: QuorumState,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobLifecycleState {
    Announced,
    Claimed,
    Assigned,
    Executing,
    ReceiptAvailable,
    Verified,
    QuorumAccepted,
    ChallengeOpen,
    ChallengeRejected,
    SettlementReady,
    Settled,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SettlementStatusReport {
    pub job_id: Uuid,
    pub lifecycle_state: JobLifecycleState,
    pub assigned_provider_node_id: Option<String>,
    pub bundle_sha256: Option<String>,
    pub required_verifier_count: u8,
    pub accepted_verifier_count: usize,
    pub rejected_verifier_count: usize,
    pub quorum_status: QuorumState,
    pub challenge_window_ends_at: Option<String>,
    pub active_challenge_count: usize,
    pub upheld_challenge_count: usize,
    pub rejected_challenge_count: usize,
    pub settlement_ready: bool,
    pub settlement_blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAvailability {
    Free,
    Claiming,
    Tasked,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProviderStatusRow {
    pub provider_node_id: String,
    pub gpu_model: String,
    pub gpu_count: i64,
    pub vram_gb: f64,
    pub node_status: String,
    pub availability: ProviderAvailability,
    pub current_load: f64,
    pub active_job_count: i64,
    pub pricing_hint: Option<String>,
    pub open_claim_count: usize,
    pub assigned_job_count: usize,
    pub completed_job_count: usize,
    pub open_claimed_job_ids: Vec<String>,
    pub assigned_job_ids: Vec<String>,
    pub completed_job_ids: Vec<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProviderNetworkStatusReport {
    pub provider_count: usize,
    pub free_provider_count: usize,
    pub claiming_provider_count: usize,
    pub tasked_provider_count: usize,
    pub degraded_provider_count: usize,
    pub offline_provider_count: usize,
    pub providers: Vec<ProviderStatusRow>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct InferenceReadinessReport {
    pub profile_id: String,
    pub provider_target: u32,
    pub healthy_providers: u32,
    pub provider_gap: u32,
    pub slot_target: u32,
    pub available_slots: u32,
    pub slot_gap: u32,
    pub verifier_target: u32,
    pub online_verifiers: u32,
    pub verifier_gap: u32,
    pub compatible_provider_ids: Vec<String>,
    pub available_provider_ids: Vec<String>,
    pub online_verifier_ids: Vec<String>,
}

pub fn calculate_quorum_status(
    job_id: Uuid,
    required_verifier_count: u8,
    receipts: &[VerificationReceipt],
) -> QuorumStatusReport {
    let mut accepted = BTreeSet::new();
    let mut rejected = BTreeSet::new();
    let mut bundle_sha256 = None;

    for receipt in receipts.iter().filter(|receipt| receipt.job_id == job_id) {
        if bundle_sha256.is_none() {
            bundle_sha256 = Some(receipt.bundle_sha256.clone());
        }
        match receipt.verification_status {
            VerificationStatus::Accepted => {
                accepted.insert(receipt.verifier_id.clone());
                rejected.remove(&receipt.verifier_id);
            }
            VerificationStatus::Rejected => {
                if !accepted.contains(&receipt.verifier_id) {
                    rejected.insert(receipt.verifier_id.clone());
                }
            }
            VerificationStatus::Inconclusive => {}
        }
    }

    let accepted_verifier_ids = accepted.into_iter().collect::<Vec<_>>();
    let rejected_verifier_ids = rejected.into_iter().collect::<Vec<_>>();
    let accepted_verifier_count = accepted_verifier_ids.len();
    let rejected_verifier_count = rejected_verifier_ids.len();
    let status = if accepted_verifier_count >= usize::from(required_verifier_count) {
        QuorumState::Accepted
    } else if rejected_verifier_count > 0 {
        QuorumState::Rejected
    } else {
        QuorumState::Pending
    };

    QuorumStatusReport {
        job_id,
        bundle_sha256,
        required_verifier_count,
        accepted_verifier_count,
        rejected_verifier_count,
        accepted_verifier_ids,
        rejected_verifier_ids,
        status,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn calculate_settlement_status(
    job_id: Uuid,
    challenge_window_seconds: u64,
    has_announcement: bool,
    claims: &[StoredJobClaim],
    assignment: Option<&StoredJobAssignment>,
    receipt_availability: &[StoredReceiptAvailability],
    verification_receipts: &[VerificationReceipt],
    quorum: &QuorumStatusReport,
    challenges: &[ChallengeRecord],
    receipt_bundle: Option<&ReceiptBundle>,
    chain_submitted: bool,
    now: DateTime<Utc>,
) -> SettlementStatusReport {
    let accepted_verified_at = verification_receipts
        .iter()
        .filter(|receipt| {
            receipt.job_id == job_id && receipt.verification_status == VerificationStatus::Accepted
        })
        .filter_map(|receipt| parse_rfc3339_utc(&receipt.verified_at))
        .max();
    let challenge_window_ends_at = accepted_verified_at.map(|verified_at| {
        verified_at + Duration::seconds(i64::try_from(challenge_window_seconds).unwrap_or(i64::MAX))
    });

    let challenge_window_closed = challenge_window_ends_at
        .map(|ends_at| now >= ends_at)
        .unwrap_or(false);
    let active_challenge_count = challenges
        .iter()
        .filter(|challenge| challenge.job_id == job_id && challenge.status == ChallengeStatus::Open)
        .count();
    let upheld_challenge_count = challenges
        .iter()
        .filter(|challenge| {
            challenge.job_id == job_id && challenge.status == ChallengeStatus::ResolvedAccepted
        })
        .count();
    let rejected_challenge_count = challenges
        .iter()
        .filter(|challenge| {
            challenge.job_id == job_id && challenge.status == ChallengeStatus::ResolvedRejected
        })
        .count();
    let any_challenge_resolved = upheld_challenge_count > 0 || rejected_challenge_count > 0;

    let mut blockers = Vec::new();
    if !has_announcement {
        blockers.push("missing_job_announcement".to_string());
    }
    if claims.is_empty() {
        blockers.push("missing_provider_claim".to_string());
    }
    if assignment.is_none() {
        blockers.push("missing_provider_assignment".to_string());
    }
    if receipt_availability.is_empty() {
        blockers.push("missing_receipt_availability".to_string());
    }
    if quorum.status != QuorumState::Accepted {
        blockers.push("quorum_not_accepted".to_string());
    }
    if active_challenge_count > 0 {
        blockers.push("active_challenge".to_string());
    }
    if upheld_challenge_count > 0 {
        blockers.push("challenge_upheld".to_string());
    }
    if quorum.status == QuorumState::Accepted
        && active_challenge_count == 0
        && upheld_challenge_count == 0
        && !challenge_window_closed
        && !any_challenge_resolved
    {
        blockers.push("challenge_window_open".to_string());
    }

    let settlement_ready = blockers.is_empty();
    let lifecycle_state = if chain_submitted {
        JobLifecycleState::Settled
    } else if upheld_challenge_count > 0 {
        JobLifecycleState::ChallengeRejected
    } else if active_challenge_count > 0 {
        JobLifecycleState::ChallengeOpen
    } else if settlement_ready {
        JobLifecycleState::SettlementReady
    } else if quorum.status == QuorumState::Accepted {
        JobLifecycleState::QuorumAccepted
    } else if !verification_receipts.is_empty() {
        JobLifecycleState::Verified
    } else if !receipt_availability.is_empty() {
        JobLifecycleState::ReceiptAvailable
    } else if assignment.is_some() {
        JobLifecycleState::Assigned
    } else if !claims.is_empty() {
        JobLifecycleState::Claimed
    } else {
        JobLifecycleState::Announced
    };

    SettlementStatusReport {
        job_id,
        lifecycle_state,
        assigned_provider_node_id: assignment
            .map(|assignment| assignment.assigned_provider_node_id.clone()),
        bundle_sha256: receipt_bundle
            .map(|bundle| bundle.bundle_sha256.clone())
            .or_else(|| quorum.bundle_sha256.clone())
            .or_else(|| {
                receipt_availability
                    .first()
                    .map(|availability| availability.bundle_sha256.clone())
            }),
        required_verifier_count: quorum.required_verifier_count,
        accepted_verifier_count: quorum.accepted_verifier_count,
        rejected_verifier_count: quorum.rejected_verifier_count,
        quorum_status: quorum.status.clone(),
        challenge_window_ends_at: challenge_window_ends_at.map(|ends_at| ends_at.to_rfc3339()),
        active_challenge_count,
        upheld_challenge_count,
        rejected_challenge_count,
        settlement_ready,
        settlement_blockers: blockers,
    }
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub fn build_provider_network_status(
    capabilities: &[StoredProviderCapability],
    claims: &[StoredJobClaim],
    assignments: &[StoredJobAssignment],
    receipt_availability: &[StoredReceiptAvailability],
) -> ProviderNetworkStatusReport {
    let mut completed_by_provider: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for availability in receipt_availability {
        completed_by_provider
            .entry(availability.provider_node_id.clone())
            .or_default()
            .insert(availability.job_id.clone());
    }

    let mut open_by_provider: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for claim in claims {
        let completed = completed_by_provider
            .get(&claim.provider_node_id)
            .is_some_and(|jobs| jobs.contains(&claim.job_id));
        if !completed {
            open_by_provider
                .entry(claim.provider_node_id.clone())
                .or_default()
                .insert(claim.job_id.clone());
        }
    }

    let mut assigned_by_provider: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for assignment in assignments {
        let completed = completed_by_provider
            .get(&assignment.assigned_provider_node_id)
            .is_some_and(|jobs| jobs.contains(&assignment.job_id));
        if !completed {
            assigned_by_provider
                .entry(assignment.assigned_provider_node_id.clone())
                .or_default()
                .insert(assignment.job_id.clone());
        }
    }

    let mut providers = capabilities
        .iter()
        .map(|capability| {
            let open_claimed_job_ids = open_by_provider
                .get(&capability.node_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>();
            let assigned_job_ids = assigned_by_provider
                .get(&capability.node_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>();
            let completed_job_ids = completed_by_provider
                .get(&capability.node_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>();
            let availability = provider_availability(
                &capability.status,
                capability.active_job_count,
                !assigned_job_ids.is_empty(),
                !open_claimed_job_ids.is_empty(),
            );
            ProviderStatusRow {
                provider_node_id: capability.node_id.clone(),
                gpu_model: capability.gpu_model.clone(),
                gpu_count: capability.gpu_count,
                vram_gb: capability.vram_gb,
                node_status: capability.status.clone(),
                availability,
                current_load: capability.current_load,
                active_job_count: capability.active_job_count,
                pricing_hint: capability.pricing_hint.clone(),
                open_claim_count: open_claimed_job_ids.len(),
                assigned_job_count: assigned_job_ids.len(),
                completed_job_count: completed_job_ids.len(),
                open_claimed_job_ids,
                assigned_job_ids,
                completed_job_ids,
                updated_at: capability.updated_at.clone(),
            }
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.provider_node_id.cmp(&right.provider_node_id));

    ProviderNetworkStatusReport {
        provider_count: providers.len(),
        free_provider_count: providers
            .iter()
            .filter(|row| row.availability == ProviderAvailability::Free)
            .count(),
        claiming_provider_count: providers
            .iter()
            .filter(|row| row.availability == ProviderAvailability::Claiming)
            .count(),
        tasked_provider_count: providers
            .iter()
            .filter(|row| row.availability == ProviderAvailability::Tasked)
            .count(),
        degraded_provider_count: providers
            .iter()
            .filter(|row| row.availability == ProviderAvailability::Degraded)
            .count(),
        offline_provider_count: providers
            .iter()
            .filter(|row| row.availability == ProviderAvailability::Offline)
            .count(),
        providers,
    }
}

fn provider_availability(
    node_status: &str,
    active_job_count: i64,
    has_assigned_jobs: bool,
    has_open_claims: bool,
) -> ProviderAvailability {
    match node_status {
        "offline_planned" => ProviderAvailability::Offline,
        "degraded" => ProviderAvailability::Degraded,
        _ if active_job_count > 0 || has_assigned_jobs => ProviderAvailability::Tasked,
        _ if has_open_claims => ProviderAvailability::Claiming,
        _ => ProviderAvailability::Free,
    }
}

pub fn build_inference_readiness_report(
    profile_id: &str,
    peer_presences: &[StoredPeerPresence],
    provider_capabilities: &[ProviderCapability],
) -> InferenceReadinessReport {
    const PROVIDER_TARGET: u32 = 4;
    const SLOT_TARGET: u32 = 3;
    const VERIFIER_TARGET: u32 = 2;
    const ONLINE_FRESHNESS_MINUTES: i64 = 15;

    let freshness_cutoff = Utc::now() - Duration::minutes(ONLINE_FRESHNESS_MINUTES);

    let mut compatible_provider_ids = provider_capabilities
        .iter()
        .filter(|capability| provider_is_healthy_for_inference(capability, freshness_cutoff))
        .filter(|capability| provider_supports_inference(capability))
        .map(|capability| capability.node_id.clone())
        .collect::<Vec<_>>();
    compatible_provider_ids.sort();

    let mut available_provider_ids = provider_capabilities
        .iter()
        .filter(|capability| provider_is_healthy_for_inference(capability, freshness_cutoff))
        .filter(|capability| provider_supports_inference(capability))
        .filter(|capability| capability.active_job_count == 0)
        .map(|capability| capability.node_id.clone())
        .collect::<Vec<_>>();
    available_provider_ids.sort();

    let mut online_verifier_ids = peer_presences
        .iter()
        .filter(|presence| presence.role == "verifier")
        .filter(|presence| peer_is_online(&presence.status))
        .filter_map(|presence| {
            let seen_at = parse_rfc3339_utc(&presence.last_seen_at)?;
            (seen_at >= freshness_cutoff).then(|| presence.node_id.clone())
        })
        .collect::<Vec<_>>();
    online_verifier_ids.sort();
    online_verifier_ids.dedup();

    let healthy_providers = compatible_provider_ids.len() as u32;
    let available_slots = available_provider_ids.len() as u32;
    let online_verifiers = online_verifier_ids.len() as u32;

    InferenceReadinessReport {
        profile_id: profile_id.to_string(),
        provider_target: PROVIDER_TARGET,
        healthy_providers,
        provider_gap: PROVIDER_TARGET.saturating_sub(healthy_providers),
        slot_target: SLOT_TARGET,
        available_slots,
        slot_gap: SLOT_TARGET.saturating_sub(available_slots),
        verifier_target: VERIFIER_TARGET,
        online_verifiers,
        verifier_gap: VERIFIER_TARGET.saturating_sub(online_verifiers),
        compatible_provider_ids,
        available_provider_ids,
        online_verifier_ids,
    }
}

fn provider_supports_inference(capability: &ProviderCapability) -> bool {
    capability
        .supported_job_types
        .iter()
        .any(|job_type| matches!(job_type, osciris_core::JobType::InferenceEconomics))
        && capability.supported_runtimes.iter().any(|runtime| {
            let runtime = runtime.to_ascii_lowercase();
            runtime == "llama-cpp" || runtime == "deterministic"
        })
}

fn provider_is_healthy_for_inference(
    capability: &ProviderCapability,
    freshness_cutoff: DateTime<Utc>,
) -> bool {
    matches!(capability.status, NodeStatus::OnlineIdle | NodeStatus::OnlineBusy)
        && parse_rfc3339_utc(&capability.updated_at)
            .is_some_and(|updated_at| updated_at >= freshness_cutoff)
}

fn peer_is_online(status: &str) -> bool {
    matches!(status, "online_idle" | "online_busy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use osciris_core::{
        ChainSubmissionStatus, ChallengeReasonCode, JobType, NodeStatus, VerificationChecks,
    };

    fn receipt(job_id: Uuid, verifier_id: &str, status: VerificationStatus) -> VerificationReceipt {
        VerificationReceipt {
            verification_receipt_id: Uuid::now_v7(),
            receipt_id: Uuid::now_v7(),
            job_id,
            verifier_id: verifier_id.to_string(),
            verification_status: status,
            verified_at: "2026-06-04T00:00:00Z".to_string(),
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
            signature: "signature".to_string(),
            signing_key_id: "verifier-key".to_string(),
        }
    }

    fn accepted_receipt_at(job_id: Uuid, verified_at: &str) -> VerificationReceipt {
        let mut receipt = receipt(job_id, "verifier-1", VerificationStatus::Accepted);
        receipt.verified_at = verified_at.to_string();
        receipt
    }

    fn assignment(job_id: Uuid) -> StoredJobAssignment {
        StoredJobAssignment {
            job_id: job_id.to_string(),
            assigned_provider_node_id: "provider-1".to_string(),
            assigner_node_id: "enterprise-1".to_string(),
            assignment_reason: "manual_assignment".to_string(),
            assigned_at: "2026-06-04T00:00:02Z".to_string(),
        }
    }

    fn claim(job_id: Uuid) -> StoredJobClaim {
        StoredJobClaim {
            job_id: job_id.to_string(),
            provider_node_id: "provider-1".to_string(),
            claimed_at: "2026-06-04T00:00:01Z".to_string(),
            claim_note: None,
        }
    }

    fn availability(job_id: Uuid) -> StoredReceiptAvailability {
        StoredReceiptAvailability {
            job_id: job_id.to_string(),
            provider_node_id: "provider-1".to_string(),
            execution_receipt_sha256: "a".repeat(64),
            bundle_sha256: "b".repeat(64),
            bundle_uri: "file:///tmp/evidence".to_string(),
            announced_at: "2026-06-04T00:00:02Z".to_string(),
        }
    }

    fn bundle(job_id: Uuid) -> ReceiptBundle {
        ReceiptBundle {
            bundle_id: Uuid::now_v7(),
            job_id,
            job_spec_sha256: "c".repeat(64),
            execution_receipt_sha256: "a".repeat(64),
            verification_receipt_sha256_list: vec!["d".repeat(64)],
            bundle_sha256: "b".repeat(64),
            artifact_index_path: "artifact_index.json".to_string(),
            chain_submission_status: ChainSubmissionStatus::Pending,
        }
    }

    fn challenge(job_id: Uuid, status: ChallengeStatus) -> ChallengeRecord {
        ChallengeRecord {
            challenge_id: Uuid::now_v7(),
            job_id,
            bundle_sha256: "b".repeat(64),
            opened_by: "verifier-1".to_string(),
            opened_by_ed25519_public_key_base64: "public-key".to_string(),
            reason_code: ChallengeReasonCode::ForbiddenJobTransition,
            reason_detail: "test challenge".to_string(),
            opened_at: "2026-06-04T00:00:05Z".to_string(),
            status,
            resolved_by: None,
            resolved_by_ed25519_public_key_base64: None,
            resolved_at: None,
            resolution_note: None,
            signature: "signature".to_string(),
        }
    }

    #[test]
    fn quorum_status_pending_without_receipts() {
        let job_id = Uuid::now_v7();
        let report = calculate_quorum_status(job_id, 1, &[]);
        assert_eq!(report.status, QuorumState::Pending);
        assert_eq!(report.accepted_verifier_count, 0);
        assert_eq!(report.rejected_verifier_count, 0);
    }

    #[test]
    fn quorum_status_accepts_when_required_count_is_met() {
        let job_id = Uuid::now_v7();
        let receipts = vec![receipt(job_id, "verifier-1", VerificationStatus::Accepted)];
        let report = calculate_quorum_status(job_id, 1, &receipts);
        assert_eq!(report.status, QuorumState::Accepted);
        assert_eq!(report.accepted_verifier_ids, vec!["verifier-1"]);
    }

    #[test]
    fn quorum_status_rejects_when_receipt_rejects_before_quorum() {
        let job_id = Uuid::now_v7();
        let receipts = vec![receipt(job_id, "verifier-1", VerificationStatus::Rejected)];
        let report = calculate_quorum_status(job_id, 2, &receipts);
        assert_eq!(report.status, QuorumState::Rejected);
        assert_eq!(report.rejected_verifier_ids, vec!["verifier-1"]);
    }

    #[test]
    fn quorum_status_deduplicates_verifiers() {
        let job_id = Uuid::now_v7();
        let receipts = vec![
            receipt(job_id, "verifier-1", VerificationStatus::Rejected),
            receipt(job_id, "verifier-1", VerificationStatus::Accepted),
            receipt(job_id, "verifier-2", VerificationStatus::Inconclusive),
        ];
        let report = calculate_quorum_status(job_id, 1, &receipts);
        assert_eq!(report.status, QuorumState::Accepted);
        assert_eq!(report.accepted_verifier_count, 1);
        assert_eq!(report.rejected_verifier_count, 0);
    }

    #[test]
    fn settlement_status_blocks_before_assignment_receipt_and_quorum() {
        let job_id = Uuid::now_v7();
        let quorum = calculate_quorum_status(job_id, 1, &[]);
        let report = calculate_settlement_status(
            job_id,
            0,
            true,
            &[],
            None,
            &[],
            &[],
            &quorum,
            &[],
            None,
            false,
            "2026-06-04T00:10:00Z".parse::<DateTime<Utc>>().unwrap(),
        );

        assert_eq!(report.lifecycle_state, JobLifecycleState::Announced);
        assert!(!report.settlement_ready);
        assert!(report
            .settlement_blockers
            .contains(&"missing_provider_assignment".to_string()));
        assert!(report
            .settlement_blockers
            .contains(&"quorum_not_accepted".to_string()));
    }

    #[test]
    fn settlement_status_blocks_during_challenge_window() {
        let job_id = Uuid::now_v7();
        let receipts = vec![accepted_receipt_at(job_id, "2026-06-04T00:00:00Z")];
        let quorum = calculate_quorum_status(job_id, 1, &receipts);
        let report = calculate_settlement_status(
            job_id,
            3600,
            true,
            &[claim(job_id)],
            Some(&assignment(job_id)),
            &[availability(job_id)],
            &receipts,
            &quorum,
            &[],
            Some(&bundle(job_id)),
            false,
            "2026-06-04T00:30:00Z".parse::<DateTime<Utc>>().unwrap(),
        );

        assert_eq!(report.lifecycle_state, JobLifecycleState::QuorumAccepted);
        assert!(!report.settlement_ready);
        assert_eq!(report.settlement_blockers, vec!["challenge_window_open"]);
    }

    #[test]
    fn settlement_status_blocks_on_open_challenge() {
        let job_id = Uuid::now_v7();
        let receipts = vec![accepted_receipt_at(job_id, "2026-06-04T00:00:00Z")];
        let quorum = calculate_quorum_status(job_id, 1, &receipts);
        let report = calculate_settlement_status(
            job_id,
            0,
            true,
            &[claim(job_id)],
            Some(&assignment(job_id)),
            &[availability(job_id)],
            &receipts,
            &quorum,
            &[challenge(job_id, ChallengeStatus::Open)],
            Some(&bundle(job_id)),
            false,
            "2026-06-04T00:00:01Z".parse::<DateTime<Utc>>().unwrap(),
        );

        assert_eq!(report.lifecycle_state, JobLifecycleState::ChallengeOpen);
        assert!(!report.settlement_ready);
        assert_eq!(report.active_challenge_count, 1);
    }

    #[test]
    fn settlement_status_ready_after_challenge_rejected() {
        let job_id = Uuid::now_v7();
        let receipts = vec![accepted_receipt_at(job_id, "2026-06-04T00:00:00Z")];
        let quorum = calculate_quorum_status(job_id, 1, &receipts);
        let report = calculate_settlement_status(
            job_id,
            3600,
            true,
            &[claim(job_id)],
            Some(&assignment(job_id)),
            &[availability(job_id)],
            &receipts,
            &quorum,
            &[challenge(job_id, ChallengeStatus::ResolvedRejected)],
            Some(&bundle(job_id)),
            false,
            "2026-06-04T00:10:00Z".parse::<DateTime<Utc>>().unwrap(),
        );

        assert_eq!(report.lifecycle_state, JobLifecycleState::SettlementReady);
        assert!(report.settlement_ready);
        assert!(report.settlement_blockers.is_empty());
    }

    #[test]
    fn settlement_status_ready_after_window_expires() {
        let job_id = Uuid::now_v7();
        let receipts = vec![accepted_receipt_at(job_id, "2026-06-04T00:00:00Z")];
        let quorum = calculate_quorum_status(job_id, 1, &receipts);
        let report = calculate_settlement_status(
            job_id,
            60,
            true,
            &[claim(job_id)],
            Some(&assignment(job_id)),
            &[availability(job_id)],
            &receipts,
            &quorum,
            &[],
            Some(&bundle(job_id)),
            false,
            "2026-06-04T00:01:01Z".parse::<DateTime<Utc>>().unwrap(),
        );

        assert_eq!(report.lifecycle_state, JobLifecycleState::SettlementReady);
        assert!(report.settlement_ready);
    }

    #[test]
    fn provider_network_status_marks_open_claim_as_claiming() {
        let provider = StoredProviderCapability {
            node_id: "provider-1".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            status: "online_idle".to_string(),
            current_load: 0.0,
            active_job_count: 0,
            pricing_hint: Some("mock".to_string()),
            updated_at: "2026-06-04T00:00:00Z".to_string(),
        };
        let claim = StoredJobClaim {
            job_id: "job-1".to_string(),
            provider_node_id: "provider-1".to_string(),
            claimed_at: "2026-06-04T00:00:01Z".to_string(),
            claim_note: None,
        };

        let report = build_provider_network_status(&[provider], &[claim], &[], &[]);

        assert_eq!(report.provider_count, 1);
        assert_eq!(report.claiming_provider_count, 1);
        assert_eq!(
            report.providers[0].availability,
            ProviderAvailability::Claiming
        );
        assert_eq!(report.providers[0].open_claimed_job_ids, vec!["job-1"]);
    }

    #[test]
    fn provider_network_status_marks_assignment_as_tasked() {
        let provider = StoredProviderCapability {
            node_id: "provider-1".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            status: "online_idle".to_string(),
            current_load: 0.0,
            active_job_count: 0,
            pricing_hint: None,
            updated_at: "2026-06-04T00:00:00Z".to_string(),
        };
        let assignment = StoredJobAssignment {
            job_id: "job-1".to_string(),
            assigned_provider_node_id: "provider-1".to_string(),
            assigner_node_id: "enterprise-1".to_string(),
            assignment_reason: "manual_assignment".to_string(),
            assigned_at: "2026-06-04T00:00:02Z".to_string(),
        };

        let report = build_provider_network_status(&[provider], &[], &[assignment], &[]);

        assert_eq!(report.tasked_provider_count, 1);
        assert_eq!(
            report.providers[0].availability,
            ProviderAvailability::Tasked
        );
        assert_eq!(report.providers[0].assigned_job_ids, vec!["job-1"]);
    }

    #[test]
    fn provider_network_status_marks_completed_claim_as_free() {
        let provider = StoredProviderCapability {
            node_id: "provider-1".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            status: "online_idle".to_string(),
            current_load: 0.0,
            active_job_count: 0,
            pricing_hint: None,
            updated_at: "2026-06-04T00:00:00Z".to_string(),
        };
        let claim = StoredJobClaim {
            job_id: "job-1".to_string(),
            provider_node_id: "provider-1".to_string(),
            claimed_at: "2026-06-04T00:00:01Z".to_string(),
            claim_note: None,
        };
        let availability = StoredReceiptAvailability {
            job_id: "job-1".to_string(),
            provider_node_id: "provider-1".to_string(),
            execution_receipt_sha256: "a".repeat(64),
            bundle_sha256: "b".repeat(64),
            bundle_uri: "file:///tmp/evidence".to_string(),
            announced_at: "2026-06-04T00:00:02Z".to_string(),
        };

        let assignment = StoredJobAssignment {
            job_id: "job-1".to_string(),
            assigned_provider_node_id: "provider-1".to_string(),
            assigner_node_id: "enterprise-1".to_string(),
            assignment_reason: "manual_assignment".to_string(),
            assigned_at: "2026-06-04T00:00:02Z".to_string(),
        };

        let report =
            build_provider_network_status(&[provider], &[claim], &[assignment], &[availability]);

        assert_eq!(report.free_provider_count, 1);
        assert_eq!(report.providers[0].availability, ProviderAvailability::Free);
        assert!(report.providers[0].open_claimed_job_ids.is_empty());
        assert_eq!(report.providers[0].completed_job_ids, vec!["job-1"]);
    }

    fn peer_presence(node_id: &str, role: &str, status: &str, last_seen_at: &str) -> StoredPeerPresence {
        StoredPeerPresence {
            node_id: node_id.to_string(),
            role: role.to_string(),
            status: status.to_string(),
            current_load: 0.0,
            active_job_count: 0,
            last_seen_at: last_seen_at.to_string(),
        }
    }

    fn inference_capability(
        node_id: &str,
        status: NodeStatus,
        active_job_count: u32,
        updated_at: &str,
    ) -> ProviderCapability {
        ProviderCapability {
            node_id: node_id.to_string(),
            ed25519_public_key_base64: "provider-public-key".to_string(),
            host_class: "interactive-inference".to_string(),
            gpu_model: "NVIDIA A10G".to_string(),
            gpu_count: 1,
            vram_gb: 24.0,
            cuda_available: true,
            mps_available: false,
            supported_job_types: vec![JobType::InferenceEconomics],
            supported_runtimes: vec!["llama-cpp".to_string()],
            pricing_hint: None,
            current_load: if active_job_count == 0 { 0.0 } else { 0.8 },
            active_job_count,
            status,
            updated_at: updated_at.to_string(),
            signature: "signature".to_string(),
        }
    }

    #[test]
    fn inference_readiness_counts_healthy_providers_slots_and_verifiers() {
        let now = Utc::now();
        let fresh = now.to_rfc3339();
        let stale = (now - Duration::minutes(16)).to_rfc3339();
        let presences = vec![
            peer_presence("verifier-1", "verifier", "online_idle", &fresh),
            peer_presence("verifier-2", "verifier", "online_busy", &fresh),
            peer_presence("verifier-3", "verifier", "online_idle", &stale),
        ];
        let capabilities = vec![
            inference_capability("provider-1", NodeStatus::OnlineIdle, 0, &fresh),
            inference_capability("provider-2", NodeStatus::OnlineBusy, 1, &fresh),
            inference_capability("provider-3", NodeStatus::Degraded, 0, &fresh),
            inference_capability("provider-4", NodeStatus::OnlineIdle, 0, &stale),
        ];

        let report =
            build_inference_readiness_report("osciris-qwen3-4b-q4-v1", &presences, &capabilities);

        assert_eq!(report.profile_id, "osciris-qwen3-4b-q4-v1");
        assert_eq!(report.healthy_providers, 2);
        assert_eq!(report.provider_gap, 2);
        assert_eq!(report.available_slots, 1);
        assert_eq!(report.slot_gap, 2);
        assert_eq!(report.online_verifiers, 2);
        assert_eq!(report.verifier_gap, 0);
        assert_eq!(report.compatible_provider_ids, vec!["provider-1", "provider-2"]);
        assert_eq!(report.available_provider_ids, vec!["provider-1"]);
        assert_eq!(report.online_verifier_ids, vec!["verifier-1", "verifier-2"]);
    }
}
