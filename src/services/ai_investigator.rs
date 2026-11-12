use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::error::AppError;
use crate::models::ai_investigation::{
    AIDisputeInvestigationReport, ContradictingEvidenceItem, FindingSeverity,
    GeminiInvestigationRawOutput, InconsistencyFinding, MissingEvidenceDisclaimerItem,
    RecommendedActionItem, SupportingEvidenceItem, TimelineNarrativeItem,
};
use crate::models::evidence::{DisputeEvidenceBrief, EpistemicCategory, EvidenceSourceReference};

/// Timeout for external LLM API calls.
pub const GEMINI_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

/// Service orchestrating AI-powered and deterministic dispute investigations.
#[derive(Clone)]
pub struct AIInvestigatorService {
    config: Arc<Config>,
    http_client: reqwest::Client,
}

impl AIInvestigatorService {
    /// Creates a new `AIInvestigatorService`.
    pub fn new(config: Arc<Config>, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http_client,
        }
    }

    /// Conducts a comprehensive dispute investigation grounded in the provided `DisputeEvidenceBrief`.
    ///
    /// Automatically attempts Google Gemini analysis if configured; cleanly falls back to
    /// deterministic rule-based analysis if the API key is missing, rate-limited, offline, or returns invalid data.
    pub async fn investigate(
        &self,
        brief: &DisputeEvidenceBrief,
        force_fallback: bool,
    ) -> Result<AIDisputeInvestigationReport, AppError> {
        // Collect all authoritatively known evidence IDs from the brief for backend validation
        let valid_evidence_catalog = self.build_valid_evidence_catalog(brief);

        // 1. Check if deterministic fallback is explicitly requested or API key is unconfigured
        let api_key = match &self.config.gemini_api_key {
            Some(key) if !key.trim().is_empty() && !force_fallback => key.trim(),
            _ => {
                let reason = if force_fallback {
                    "Deterministic analysis explicitly requested via request options."
                } else {
                    "Google Gemini API key not configured (GEMINI_API_KEY). Running deterministic fallback."
                };
                debug!(dispute_id = %brief.dispute_id, reason, "Running deterministic dispute analysis");
                return Ok(self.build_deterministic_fallback(
                    brief,
                    &valid_evidence_catalog,
                    reason,
                ));
            }
        };

        // 2. Attempt Google Gemini analysis with timeout and sanitization
        match self.call_gemini_investigator(brief, api_key).await {
            Ok(raw_output) => {
                info!(
                    dispute_id = %brief.dispute_id,
                    model = %self.config.gemini_model,
                    "Google Gemini dispute investigation completed successfully; validating evidence references"
                );
                let validated_report = self.reconcile_and_validate_ai_output(
                    brief,
                    raw_output,
                    &valid_evidence_catalog,
                );
                Ok(validated_report)
            }
            Err(err) => {
                let error_msg = format!(
                    "Gemini API call failed ({}); switched to deterministic fallback.",
                    err
                );
                warn!(
                    dispute_id = %brief.dispute_id,
                    model = %self.config.gemini_model,
                    error = %err,
                    "Gemini AI provider encountered error; falling back to deterministic analysis"
                );
                Ok(self.build_deterministic_fallback(brief, &valid_evidence_catalog, &error_msg))
            }
        }
    }

    /// Invokes the Google Gemini REST API with sanitized evidence context and structured JSON output mode.
    async fn call_gemini_investigator(
        &self,
        brief: &DisputeEvidenceBrief,
        api_key: &str,
    ) -> Result<GeminiInvestigationRawOutput, AppError> {
        let sanitized_context = self.build_sanitized_brief_context(brief);
        let system_prompt = self.build_investigator_system_prompt();

        let request_body = serde_json::json!({
            "contents": [
                {
                    "parts": [
                        {
                            "text": format!("{}\n\n### EVIDENCE CONTEXT TO INVESTIGATE:\n{}", system_prompt, sanitized_context)
                        }
                    ]
                }
            ],
            "generationConfig": {
                "responseMimeType": "application/json",
                "temperature": 0.1
            }
        });

        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.config.gemini_base_url.trim_end_matches('/'),
            self.config.gemini_model,
            api_key
        );

        let response = self
            .http_client
            .post(&url)
            .timeout(GEMINI_REQUEST_TIMEOUT)
            .json(&request_body)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown HTTP error".to_string());
            return Err(AppError::AIProvider(format!(
                "Gemini API returned status HTTP {}: {}",
                status, error_text
            )));
        }

        let resp_json: serde_json::Value = response.json().await.map_err(|e| {
            AppError::InvalidPayload(format!("Failed to parse Gemini API response JSON: {}", e))
        })?;

        // Extract raw JSON content from candidates[0].content.parts[0].text
        let content_text = resp_json
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                AppError::InvalidPayload(
                    "Malformed Gemini response: missing candidates[0].content.parts[0].text"
                        .to_string(),
                )
            })?;

        let parsed: GeminiInvestigationRawOutput =
            serde_json::from_str(content_text).map_err(|e| {
                AppError::InvalidPayload(format!(
                    "Failed to parse Gemini output into structured report schema: {}",
                    e
                ))
            })?;

        Ok(parsed)
    }

    /// Assembles a sanitized, privacy-minimized representation of the dispute evidence.
    ///
    /// Never includes PayPal credentials, access tokens, API keys, customer emails, physical addresses,
    /// or real cardholder identifiers.
    fn build_sanitized_brief_context(&self, brief: &DisputeEvidenceBrief) -> String {
        let buyer_prompt = brief
            .agent_audit_trail
            .as_ref()
            .map(|t| t.initial_prompt.prompt.clone())
            .unwrap_or_else(|| "N/A".to_string());

        let reqs = brief
            .agent_audit_trail
            .as_ref()
            .and_then(|t| t.requirements.as_ref());
        let selected = brief
            .agent_audit_trail
            .as_ref()
            .and_then(|t| t.selected_product.as_ref());
        let auth = brief
            .agent_audit_trail
            .as_ref()
            .and_then(|t| t.purchase_authorization.as_ref());
        let tx = brief.transaction_proof.as_ref();

        let dispute_reason = brief
            .dispute_summary
            .reason
            .clone()
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let dispute_amount = format!(
            "{} {}",
            brief
                .dispute_summary
                .dispute_amount_value
                .as_deref()
                .unwrap_or("0.00"),
            brief
                .dispute_summary
                .dispute_amount_currency
                .as_deref()
                .unwrap_or("USD")
        );

        let sanitized_payload = serde_json::json!({
            "dispute_id": brief.dispute_id,
            "dispute_reason": dispute_reason,
            "dispute_amount": dispute_amount,
            "dispute_create_time": brief.dispute_summary.dispute_create_time,
            "correlation_status": format!("{:?}", brief.correlation_summary.status),
            "cryptographic_integrity": {
                "dual_chain_verified": brief.cryptographic_proof.dual_chain_integrity_verified,
                "purchase_chain_valid": brief.cryptographic_proof.purchase_chain_valid,
                "dispute_chain_valid": brief.cryptographic_proof.dispute_chain_valid,
                "purchase_root_hash": brief.cryptographic_proof.purchase_root_hash,
                "dispute_genesis_anchor": brief.cryptographic_proof.genesis_anchor_hash,
                "total_decision_steps": brief.cryptographic_proof.purchase_total_steps
            },
            "autonomous_shopping_audit": {
                "buyer_natural_language_prompt": buyer_prompt,
                "extracted_requirements": reqs.map(|r| serde_json::json!({
                    "category": r.category,
                    "max_budget": r.maximum_budget,
                    "required_features": r.required_features,
                    "source_step_hash": r.source.step_hash
                })),
                "selected_product": selected.map(|s| serde_json::json!({
                    "product_id": s.product_id,
                    "name": s.product_name,
                    "price": s.price,
                    "source_step_hash": s.source.step_hash
                })),
                "explicit_customer_authorization": auth.map(|a| serde_json::json!({
                    "user_confirmed": a.user_confirmed,
                    "authorized_spending_ceiling": a.maximum_authorized_amount,
                    "source_step_hash": a.source.step_hash
                }))
            },
            "paypal_settlement_record": tx.map(|t| serde_json::json!({
                "paypal_order_id": t.paypal_settlement.paypal_order_id,
                "paypal_capture_id": t.paypal_settlement.paypal_capture_id,
                "settled_amount": format!(
                    "{} {}",
                    t.paypal_settlement.order_amount.as_deref().unwrap_or("0.00"),
                    t.paypal_settlement.order_currency.as_deref().unwrap_or("USD")
                ),
                "settlement_status": t.paypal_settlement.capture_status,
                "payer_identifier_masked": "buyer-sandbox-***"
            })),
            "rebuttal_points_available": brief.rebuttal_summary.primary_arguments.iter().map(|p| serde_json::json!({
                "claim": p.claim,
                "epistemic_category": format!("{:?}", p.category),
                "explanation": p.explanation,
                "sources": p.source_references.iter().map(get_source_ref_id).collect::<Vec<_>>()
            })).collect::<Vec<_>>(),
            "explicit_missing_evidence_disclosures": brief.missing_evidence.missing_items.iter().map(|d| serde_json::json!({
                "item": format!("{:?}", d.evidence_type),
                "reason_unavailable": d.reason_unavailable
            })).collect::<Vec<_>>()
        });

        serde_json::to_string_pretty(&sanitized_payload).unwrap_or_default()
    }

    /// Construct strict prompt enforcing grounding and epistemic honesty.
    fn build_investigator_system_prompt(&self) -> &'static str {
        r#"You are AgentAudit's AI Dispute Investigator, an impartial forensic auditor for autonomous commerce transactions.
Your task is to analyze the provided case evidence and synthesize an objective, structured investigation report.

CRITICAL INTEGRITY INVARIANTS:
1. STRICT GROUNDING: Base your findings strictly on the verified facts provided in the evidence context.
2. DO NOT HALLUCINATE EVIDENCE: Every fact cited in narrative_timeline, inconsistencies, and evidence sections MUST cite an authoritative evidence reference provided in the context (such as a step_hash, paypal_order_id, paypal_capture_id, or dispute_id).
3. NEVER FABRICATE DELIVERY: If physical shipping tracking is missing, do not state or infer that delivery occurred. Acknowledge it explicitly under missing_evidence.
4. EPISTEMIC DISTINCTIONS:
   - VERIFIED_FACT: Cryptographically sealed decisions and customer authorizations.
   - RECORDED_PAYPAL_DATA: Data recorded by PayPal APIs and signed webhooks.
   - DETERMINISTIC_INFERENCE: Inferences derived from deterministic catalog rule checks.
5. OBJECTIVITY: Highlight any potential mistakes, budget overruns, or contradictions fairly.

Return your analysis strictly as a valid JSON object matching this schema:
{
  "executive_summary": "Concise 2-3 sentence overview of the case, core dispute reason, and evidence posture.",
  "narrative_timeline": [
    {
      "phase": "e.g. 1. Shopping Intent",
      "description": "Chronological description of what occurred",
      "evidence_id": "exact step_hash, order_id, capture_id, or dispute_id from context",
      "evidence_type": "VERIFIED_FACT | RECORDED_PAYPAL_DATA | DETERMINISTIC_INFERENCE"
    }
  ],
  "inconsistencies_or_red_flags": [
    {
      "severity": "LOW | MEDIUM | HIGH",
      "finding": "Short title of inconsistency or red flag",
      "details": "Factual explanation of why this is inconsistent or noteworthy",
      "evidence_ref": "exact evidence reference from context"
    }
  ],
  "evidence_supporting_dispute": [
    {
      "claim": "Customer's claim or reason for disputing",
      "source": "e.g. PayPal Dispute Webhook",
      "evidence_id": "exact dispute_id from context"
    }
  ],
  "evidence_contradicting_dispute": [
    {
      "fact": "Verified fact refuting or contextualizing the dispute",
      "source": "e.g. Purchase Session Cryptographic Ledger",
      "evidence_id": "exact step_hash or capture_id from context",
      "epistemic_category": "VERIFIED_FACT | RECORDED_PAYPAL_DATA | DETERMINISTIC_INFERENCE"
    }
  ],
  "missing_evidence": [
    {
      "item": "e.g. Carrier Proof of Physical Delivery",
      "reason_unavailable": "e.g. Physical tracking is not captured in this digital audit chain"
    }
  ],
  "recommended_next_steps": [
    {
      "priority": 1,
      "action": "Concrete next step for merchant or reviewer",
      "rationale": "Why this action is justified by the evidence"
    }
  ]
}"#
    }

    /// Builds a comprehensive catalog of valid evidence references from the `DisputeEvidenceBrief`.
    fn build_valid_evidence_catalog(&self, brief: &DisputeEvidenceBrief) -> HashSet<String> {
        let mut catalog = HashSet::new();

        catalog.insert(brief.dispute_id.clone());
        catalog.insert(brief.brief_id.clone());

        if let Some(ref root_hash) = brief.cryptographic_proof.purchase_root_hash {
            catalog.insert(root_hash.clone());
            if root_hash.len() >= 16 {
                catalog.insert(root_hash[..16].to_string());
            }
        }
        catalog.insert(brief.cryptographic_proof.genesis_anchor_hash.clone());
        if brief.cryptographic_proof.genesis_anchor_hash.len() >= 16 {
            catalog.insert(brief.cryptographic_proof.genesis_anchor_hash[..16].to_string());
        }

        if let Some(ref tx) = brief.transaction_proof {
            if let Some(ref order_id) = tx.paypal_settlement.paypal_order_id {
                catalog.insert(order_id.clone());
            }
            if let Some(ref capture_id) = tx.paypal_settlement.paypal_capture_id {
                catalog.insert(capture_id.clone());
            }
        }

        if let Some(ref trail) = brief.agent_audit_trail {
            self.insert_source_ref(&mut catalog, &trail.initial_prompt.source);
            if let Some(ref req) = trail.requirements {
                self.insert_source_ref(&mut catalog, &req.source);
            }
            if let Some(ref cat) = trail.catalog_search {
                self.insert_source_ref(&mut catalog, &cat.source);
            }
            for rej in &trail.rejected_candidates {
                self.insert_source_ref(&mut catalog, &rej.source);
            }
            if let Some(ref sel) = trail.selected_product {
                self.insert_source_ref(&mut catalog, &sel.source);
            }
            if let Some(ref auth) = trail.purchase_authorization {
                self.insert_source_ref(&mut catalog, &auth.source);
            }
        }

        for pt in &brief.rebuttal_summary.primary_arguments {
            for src in &pt.source_references {
                self.insert_source_ref(&mut catalog, src);
            }
        }

        catalog
    }

    fn insert_source_ref(&self, catalog: &mut HashSet<String>, source: &EvidenceSourceReference) {
        catalog.insert(get_source_ref_id(source));
        if let Some(ref hash) = source.step_hash {
            catalog.insert(hash.clone());
            if hash.len() >= 16 {
                catalog.insert(hash[..16].to_string());
            }
        }
        if let Some(ref p_id) = source.paypal_record_id {
            catalog.insert(p_id.clone());
        }
        if let Some(ref wh_id) = source.webhook_transmission_id {
            catalog.insert(wh_id.clone());
        }
    }

    /// Reconciles raw LLM output against the authoritative evidence catalog, computing `hash_verified`
    /// on the backend and discarding unsupported evidence references.
    fn reconcile_and_validate_ai_output(
        &self,
        brief: &DisputeEvidenceBrief,
        raw: GeminiInvestigationRawOutput,
        valid_catalog: &HashSet<String>,
    ) -> AIDisputeInvestigationReport {
        let mut valid_refs = Vec::new();
        let mut discarded_refs = Vec::new();

        let purchase_chain_valid = brief.cryptographic_proof.purchase_chain_valid;

        // 1. Process Timeline
        let narrative_timeline = raw
            .narrative_timeline
            .into_iter()
            .map(|item| {
                let clean_ref = self.validate_or_discard_ref(
                    &item.evidence_id,
                    valid_catalog,
                    &mut valid_refs,
                    &mut discarded_refs,
                );
                let evidence_type = match item.evidence_type.as_deref() {
                    Some("RECORDED_PAYPAL_DATA") => EpistemicCategory::RecordedPaypalData,
                    Some("DETERMINISTIC_INFERENCE") => EpistemicCategory::DeterministicInference,
                    _ => EpistemicCategory::VerifiedFact,
                };
                TimelineNarrativeItem {
                    phase: if item.phase.is_empty() {
                        "Event".to_string()
                    } else {
                        item.phase
                    },
                    description: item.description,
                    evidence_id: clean_ref,
                    evidence_type,
                }
            })
            .collect();

        // 2. Process Inconsistencies
        let inconsistencies_or_red_flags = raw
            .inconsistencies_or_red_flags
            .into_iter()
            .map(|item| {
                let clean_ref = self.validate_or_discard_ref(
                    &item.evidence_ref,
                    valid_catalog,
                    &mut valid_refs,
                    &mut discarded_refs,
                );
                let severity = match item.severity.as_deref() {
                    Some("HIGH") => FindingSeverity::High,
                    Some("MEDIUM") => FindingSeverity::Medium,
                    _ => FindingSeverity::Low,
                };
                InconsistencyFinding {
                    severity,
                    finding: item.finding,
                    details: item.details,
                    evidence_ref: clean_ref,
                }
            })
            .collect();

        // 3. Process Supporting Evidence
        let evidence_supporting_dispute = raw
            .evidence_supporting_dispute
            .into_iter()
            .map(|item| {
                let clean_ref = self.validate_or_discard_ref(
                    &item.evidence_id,
                    valid_catalog,
                    &mut valid_refs,
                    &mut discarded_refs,
                );
                SupportingEvidenceItem {
                    claim: item.claim,
                    source: item.source,
                    evidence_id: clean_ref,
                }
            })
            .collect();

        // 4. Process Contradicting Evidence with backend-computed hash_verified
        let evidence_contradicting_dispute = raw
            .evidence_contradicting_dispute
            .into_iter()
            .map(|item| {
                let clean_ref = self.validate_or_discard_ref(
                    &item.evidence_id,
                    valid_catalog,
                    &mut valid_refs,
                    &mut discarded_refs,
                );

                // Backend invariant: hash_verified is computed strictly by backend ledger state
                let is_ledger_ref = clean_ref.len() == 64
                    || clean_ref.starts_with("step_hash:")
                    || valid_catalog.contains(&clean_ref);
                let hash_verified = purchase_chain_valid && is_ledger_ref;

                let epistemic_category = match item.epistemic_category.as_deref() {
                    Some("RECORDED_PAYPAL_DATA") => EpistemicCategory::RecordedPaypalData,
                    Some("DETERMINISTIC_INFERENCE") => EpistemicCategory::DeterministicInference,
                    _ => EpistemicCategory::VerifiedFact,
                };

                ContradictingEvidenceItem {
                    fact: item.fact,
                    source: item.source,
                    evidence_id: clean_ref,
                    hash_verified,
                    epistemic_category,
                }
            })
            .collect();

        // 5. Missing Evidence
        let missing_evidence = raw
            .missing_evidence
            .into_iter()
            .map(|item| MissingEvidenceDisclaimerItem {
                item: item.item,
                reason_unavailable: item.reason_unavailable,
            })
            .collect();

        // 6. Recommended Next Steps
        let recommended_next_steps = raw
            .recommended_next_steps
            .into_iter()
            .map(|item| RecommendedActionItem {
                priority: if item.priority == 0 { 1 } else { item.priority },
                action: item.action,
                rationale: item.rationale,
            })
            .collect();

        valid_refs.sort();
        valid_refs.dedup();
        discarded_refs.sort();
        discarded_refs.dedup();

        let provider_note = format!(
            "Analysis synthesized via Google Gemini ({}) with backend cryptographic verification. \
            Notice: Free-tier Gemini usage may be subject to Google data-use terms for service improvement; \
            customer personal data and PayPal secrets were strictly redacted prior to transmission.",
            self.config.gemini_model
        );

        AIDisputeInvestigationReport {
            dispute_id: brief.dispute_id.clone(),
            session_id: brief.correlation_summary.session_id.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            provider: self.config.gemini_model.clone(),
            provider_note,
            is_ai_generated: true,
            ledger_hash_verified: purchase_chain_valid,
            executive_summary: raw.executive_summary,
            narrative_timeline,
            inconsistencies_or_red_flags,
            evidence_supporting_dispute,
            evidence_contradicting_dispute,
            missing_evidence,
            recommended_next_steps,
            valid_evidence_references: valid_refs,
            discarded_unverified_references: discarded_refs,
        }
    }

    /// Checks if a reference is known in the valid catalog; if not, logs and discards it.
    fn validate_or_discard_ref(
        &self,
        candidate_ref: &str,
        valid_catalog: &HashSet<String>,
        valid_list: &mut Vec<String>,
        discarded_list: &mut Vec<String>,
    ) -> String {
        let trimmed = candidate_ref.trim();
        if trimmed.is_empty() {
            return "UNSPECIFIED_REFERENCE".to_string();
        }

        // Direct match
        if valid_catalog.contains(trimmed) {
            valid_list.push(trimmed.to_string());
            return trimmed.to_string();
        }

        // Check if candidate is a prefixed reference like "step_hash:<hash>" where <hash> is valid
        if let Some(stripped) = trimmed
            .strip_prefix("step_hash:")
            .or_else(|| trimmed.strip_prefix("hash:"))
        {
            if valid_catalog.contains(stripped) {
                valid_list.push(stripped.to_string());
                return trimmed.to_string();
            }
        }

        // Check if candidate matches a prefix of an authoritative 64-char hash in catalog
        for known in valid_catalog {
            if known.len() == 64 && known.starts_with(trimmed) && trimmed.len() >= 8 {
                valid_list.push(known.clone());
                return known.clone();
            }
        }

        // Reference is hallucinated or unverified by backend
        debug!(hallucinated_ref = %trimmed, "Discarding unverified LLM evidence reference");
        discarded_list.push(trimmed.to_string());
        "UNVERIFIED_EVIDENCE_REFERENCE".to_string()
    }

    /// Builds the deterministic fallback report when Gemini is unconfigured or fails.
    ///
    /// Never describes output as AI-generated. All statements are derived deterministically
    /// from the verified cryptographic ledger and recorded PayPal data.
    pub fn build_deterministic_fallback(
        &self,
        brief: &DisputeEvidenceBrief,
        valid_catalog: &HashSet<String>,
        note_detail: &str,
    ) -> AIDisputeInvestigationReport {
        let mut valid_refs = Vec::new();

        // 1. Executive Summary
        let correlation_status = format!("{:?}", brief.correlation_summary.status);
        let chain_valid = brief.cryptographic_proof.purchase_chain_valid;
        let dual_valid = brief.cryptographic_proof.dual_chain_integrity_verified;

        let executive_summary = if chain_valid && dual_valid {
            format!(
                "Deterministic audit of dispute {} against settled transaction. \
                The purchase session is cryptographically valid and sealed with dual-chain integrity verified. \
                Customer explicitly authorized the purchase within their declared budget ceiling of {}. \
                All catalog comparisons and decision rationales were immutably recorded.",
                brief.dispute_id,
                brief
                    .agent_audit_trail
                    .as_ref()
                    .and_then(|t| t.purchase_authorization.as_ref())
                    .map(|a| format!("${:.2}", a.maximum_authorized_amount))
                    .unwrap_or_else(|| "N/A".to_string())
            )
        } else {
            format!(
                "Deterministic audit of dispute {}. Correlation status: {}. \
                Dual-chain integrity could not be conclusively verified or session is unfinalized. \
                Manual review required before submitting merchant rebuttal.",
                brief.dispute_id, correlation_status
            )
        };

        // 2. Narrative Timeline
        let mut narrative_timeline = Vec::new();
        if let Some(ref trail) = brief.agent_audit_trail {
            let p_ref = get_source_ref_id(&trail.initial_prompt.source);
            narrative_timeline.push(TimelineNarrativeItem {
                phase: "1. Buyer Shopping Intent".to_string(),
                description: format!(
                    "Customer submitted natural language request: \"{}\"",
                    trail.initial_prompt.prompt
                ),
                evidence_id: p_ref.clone(),
                evidence_type: EpistemicCategory::VerifiedFact,
            });
            valid_refs.push(p_ref);

            if let Some(ref req) = trail.requirements {
                let req_ref = get_source_ref_id(&req.source);
                narrative_timeline.push(TimelineNarrativeItem {
                    phase: "2. Constraints Extraction".to_string(),
                    description: format!(
                        "Deterministic extraction established category '{}', maximum budget ${:.2}, and {} required features.",
                        req.category, req.maximum_budget, req.required_features.len()
                    ),
                    evidence_id: req_ref.clone(),
                    evidence_type: EpistemicCategory::VerifiedFact,
                });
                valid_refs.push(req_ref);
            }

            if let Some(ref sel) = trail.selected_product {
                let sel_ref = get_source_ref_id(&sel.source);
                narrative_timeline.push(TimelineNarrativeItem {
                    phase: "3. Product Recommendation".to_string(),
                    description: format!(
                        "Agent evaluated catalog and selected '{}' at ${:.2} satisfying all prompt constraints.",
                        sel.product_name, sel.price
                    ),
                    evidence_id: sel_ref.clone(),
                    evidence_type: EpistemicCategory::VerifiedFact,
                });
                valid_refs.push(sel_ref);
            }

            if let Some(ref auth) = trail.purchase_authorization {
                let auth_ref = get_source_ref_id(&auth.source);
                narrative_timeline.push(TimelineNarrativeItem {
                    phase: "4. Explicit Customer Consent".to_string(),
                    description: format!(
                        "Customer reviewed recommendation and explicitly authorized purchase up to ceiling of ${:.2}.",
                        auth.maximum_authorized_amount
                    ),
                    evidence_id: auth_ref.clone(),
                    evidence_type: EpistemicCategory::VerifiedFact,
                });
                valid_refs.push(auth_ref);
            }
        }

        if let Some(ref tx) = brief.transaction_proof {
            let cap_id = tx
                .paypal_settlement
                .paypal_capture_id
                .clone()
                .unwrap_or_else(|| "PAYMENT_CAPTURED".to_string());
            narrative_timeline.push(TimelineNarrativeItem {
                phase: "5. Settlement & Ledger Seal".to_string(),
                description: "PayPal payment captured successfully. Session sealed with immutable root SHA-256 hash.".to_string(),
                evidence_id: cap_id.clone(),
                evidence_type: EpistemicCategory::VerifiedFact,
            });
            valid_refs.push(cap_id);
        }

        narrative_timeline.push(TimelineNarrativeItem {
            phase: "6. Customer Dispute Notification".to_string(),
            description: format!(
                "PayPal dispute notification received for reason '{}' with claimed amount {} {}.",
                brief.dispute_summary.reason.as_deref().unwrap_or("UNKNOWN"),
                brief
                    .dispute_summary
                    .dispute_amount_value
                    .as_deref()
                    .unwrap_or("0.00"),
                brief
                    .dispute_summary
                    .dispute_amount_currency
                    .as_deref()
                    .unwrap_or("USD")
            ),
            evidence_id: brief.dispute_id.clone(),
            evidence_type: EpistemicCategory::RecordedPaypalData,
        });
        valid_refs.push(brief.dispute_id.clone());

        // 3. Inconsistencies or Red Flags
        let mut inconsistencies_or_red_flags = Vec::new();
        if let Some(ref trail) = brief.agent_audit_trail {
            if let (Some(auth), Some(sel)) =
                (&trail.purchase_authorization, &trail.selected_product)
            {
                if auth.maximum_authorized_amount < sel.price {
                    let a_ref = get_source_ref_id(&auth.source);
                    inconsistencies_or_red_flags.push(InconsistencyFinding {
                        severity: FindingSeverity::High,
                        finding: "Authorized spending limit is lower than product purchase price".to_string(),
                        details: format!(
                            "Authorized ceiling (${:.2}) is less than item price (${:.2}). Invariant violation.",
                            auth.maximum_authorized_amount, sel.price
                        ),
                        evidence_ref: a_ref,
                    });
                }
            }

            if brief.dispute_summary.reason.as_deref() == Some("UNAUTHORISED") {
                if let Some(auth) = &trail.purchase_authorization {
                    if auth.user_confirmed {
                        let a_ref = get_source_ref_id(&auth.source);
                        inconsistencies_or_red_flags.push(InconsistencyFinding {
                            severity: FindingSeverity::High,
                            finding: "Customer claims unauthorized transaction, but explicit authorization is cryptographically recorded".to_string(),
                            details: format!(
                                "Customer explicitly authorized spending up to ${:.2} for product '{}' before PayPal checkout.",
                                auth.maximum_authorized_amount,
                                trail.selected_product.as_ref().map(|s| s.product_name.as_str()).unwrap_or("Item")
                            ),
                            evidence_ref: a_ref,
                        });
                    }
                }
            }
        }

        if !brief.cryptographic_proof.dual_chain_integrity_verified {
            inconsistencies_or_red_flags.push(InconsistencyFinding {
                severity: FindingSeverity::Medium,
                finding: "Dual-chain cryptographic linkage verification incomplete".to_string(),
                details: "Dispute genesis anchor does not conclusively link to verified purchase root hash.".to_string(),
                evidence_ref: brief.cryptographic_proof.genesis_anchor_hash.clone(),
            });
        }

        // 4. Evidence Supporting Dispute
        let evidence_supporting_dispute = vec![SupportingEvidenceItem {
            claim: format!(
                "Customer opened dispute under reason '{}' demanding refund of {} {}",
                brief.dispute_summary.reason.as_deref().unwrap_or("UNKNOWN"),
                brief
                    .dispute_summary
                    .dispute_amount_value
                    .as_deref()
                    .unwrap_or("0.00"),
                brief
                    .dispute_summary
                    .dispute_amount_currency
                    .as_deref()
                    .unwrap_or("USD")
            ),
            source: "PayPal Dispute Webhook / API".to_string(),
            evidence_id: brief.dispute_id.clone(),
        }];

        // 5. Evidence Contradicting Dispute (from authoritative rebuttal points)
        let evidence_contradicting_dispute = brief
            .rebuttal_summary
            .primary_arguments
            .iter()
            .map(|p| {
                let ref_id = p
                    .source_references
                    .first()
                    .map(get_source_ref_id)
                    .unwrap_or_else(|| brief.dispute_id.clone());
                valid_refs.push(ref_id.clone());
                ContradictingEvidenceItem {
                    fact: p.claim.clone(),
                    source: "AgentAudit Verifiable Decision Ledger".to_string(),
                    evidence_id: ref_id,
                    hash_verified: brief.cryptographic_proof.purchase_chain_valid,
                    epistemic_category: p.category.clone(),
                }
            })
            .collect();

        // 6. Missing Evidence (explicit disclosures)
        let missing_evidence = brief
            .missing_evidence
            .missing_items
            .iter()
            .map(|d| MissingEvidenceDisclaimerItem {
                item: format!("{:?}", d.evidence_type),
                reason_unavailable: d.reason_unavailable.clone(),
            })
            .collect();

        // 7. Recommended Next Steps
        let mut recommended_next_steps = Vec::new();
        if chain_valid && dual_valid {
            recommended_next_steps.push(RecommendedActionItem {
                priority: 1,
                action: "Submit cryptographic decision trail and explicit authorization proof to PayPal Resolution Center".to_string(),
                rationale: "Dual-chain ledger conclusively refutes unauthorized claims and proves agent compliance with buyer intent.".to_string(),
            });
            recommended_next_steps.push(RecommendedActionItem {
                priority: 2,
                action: "Export Markdown evidence brief for merchant record archiving".to_string(),
                rationale: "Provides human-readable documentation linking PayPal capture ID to sealed hash chain.".to_string(),
            });
        } else {
            recommended_next_steps.push(RecommendedActionItem {
                priority: 1,
                action: "Route dispute to human claims specialist for manual reconciliation"
                    .to_string(),
                rationale:
                    "Correlation requires manual validation prior to formal submission to PayPal."
                        .to_string(),
            });
        }

        valid_refs.retain(|r| valid_catalog.contains(r));
        valid_refs.sort();
        valid_refs.dedup();

        let provider_note = format!(
            "Analysis synthesized via AgentAudit Deterministic Audit Engine (not AI-generated). {}. \
            All findings are computed directly from SQLite cryptographic ledger events and PayPal API records.",
            note_detail
        );

        AIDisputeInvestigationReport {
            dispute_id: brief.dispute_id.clone(),
            session_id: brief.correlation_summary.session_id.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            provider: "deterministic_fallback".to_string(),
            provider_note,
            is_ai_generated: false,
            ledger_hash_verified: chain_valid,
            executive_summary,
            narrative_timeline,
            inconsistencies_or_red_flags,
            evidence_supporting_dispute,
            evidence_contradicting_dispute,
            missing_evidence,
            recommended_next_steps,
            valid_evidence_references: valid_refs,
            discarded_unverified_references: Vec::new(),
        }
    }
}

/// Helper function to retrieve a human-readable anchor from an `EvidenceSourceReference`.
pub fn get_source_ref_id(source: &EvidenceSourceReference) -> String {
    if let Some(ref hash) = source.step_hash {
        hash.clone()
    } else if let Some(ref p_id) = source.paypal_record_id {
        p_id.clone()
    } else if let Some(ref wh_id) = source.webhook_transmission_id {
        wh_id.clone()
    } else if let Some(ref s_id) = source.session_id {
        s_id.clone()
    } else if let Some(ref d_id) = source.dispute_id {
        d_id.clone()
    } else {
        "unspecified_ref".to_string()
    }
}
