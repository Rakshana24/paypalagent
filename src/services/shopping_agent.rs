use std::sync::Arc;
use tracing::info;

use crate::error::AppError;
use crate::models::authorization::{
    AuthorizationStatus, AuthorizePurchaseRequest, AuthorizePurchaseResponse, PurchaseAuthorization,
};
use crate::models::catalog::Product;
use crate::models::ledger::AppendEventRequest;
use crate::models::paypal_orders::ShoppingSessionDetail;
use crate::models::shopping::{
    CandidateEvaluation, CandidateSummary, RejectionRecord, SelectionRecord, ShoppingRequirements,
    ShoppingSessionResponse,
};
use crate::services::catalog::CatalogService;
use crate::services::ledger::LedgerService;
use crate::services::requirement_extractor::RequirementExtractor;
use crate::services::verification::VerificationService;

/// Core AI shopping agent orchestrating requirement extraction, catalog exploration,
/// product comparison, explicit purchase authorization, and cryptographic ledger recording.
#[derive(Clone)]
pub struct ShoppingAgentService {
    catalog_service: CatalogService,
    ledger_service: Arc<LedgerService>,
    verification_service: Arc<VerificationService>,
}

impl ShoppingAgentService {
    /// Creates a new `ShoppingAgentService` instance.
    pub fn new(
        catalog_service: CatalogService,
        ledger_service: Arc<LedgerService>,
        verification_service: Arc<VerificationService>,
    ) -> Self {
        Self {
            catalog_service,
            ledger_service,
            verification_service,
        }
    }

    /// Executes the reference shopping workflow (Milestones 1 through 4) for a customer prompt.
    pub async fn process_shopping_request(
        &self,
        prompt: &str,
    ) -> Result<ShoppingSessionResponse, AppError> {
        let trimmed_prompt = prompt.trim();
        if trimmed_prompt.is_empty() {
            return Err(AppError::InvalidPayload(
                "Customer prompt cannot be empty".to_string(),
            ));
        }

        // =========================================================================
        // MILESTONE 1: USER REQUEST & REQUIREMENT EXTRACTION
        // =========================================================================
        let session = self
            .ledger_service
            .create_session(crate::models::CreateSessionRequest {
                initial_prompt: trimmed_prompt.to_string(),
            })
            .await?;

        let session_id = session.session_id.clone();

        let requirements = RequirementExtractor::extract(trimmed_prompt)?;

        let req_event = self
            .ledger_service
            .append_event(
                &session_id,
                AppendEventRequest {
                    action_type: "REQUIREMENTS_EXTRACTED".to_string(),
                    payload: serde_json::to_value(&requirements)?,
                },
            )
            .await?;

        // =========================================================================
        // MILESTONE 2: CATALOG EXPLORATION
        // =========================================================================
        let all_candidates = self
            .catalog_service
            .search_products(&requirements.category, Some(&requirements.category));

        let candidate_summaries: Vec<CandidateSummary> = all_candidates
            .iter()
            .map(|p| CandidateSummary {
                product_id: p.product_id.clone(),
                name: p.name.clone(),
                brand: p.brand.clone(),
                price: p.price,
                currency: p.currency.clone(),
                battery_life_hours: p.specifications.battery_life_hours,
                noise_cancelling: p.specifications.noise_cancelling,
                rating: p.rating,
            })
            .collect();

        self.ledger_service
            .append_event(
                &session_id,
                AppendEventRequest {
                    action_type: "CATALOG_SEARCH".to_string(),
                    payload: serde_json::json!({
                        "category": requirements.category,
                        "candidates_retrieved_count": candidate_summaries.len(),
                        "candidates": candidate_summaries,
                    }),
                },
            )
            .await?;

        // =========================================================================
        // MILESTONE 3: PRODUCT COMPARISON & EVALUATION
        // =========================================================================
        let mut matching_evaluations: Vec<(Product, CandidateEvaluation)> = Vec::new();
        let mut rejections: Vec<RejectionRecord> = Vec::new();

        for product in all_candidates {
            // Check mandatory budget
            if product.price > requirements.maximum_budget {
                let rejection = RejectionRecord {
                    product_id: product.product_id.clone(),
                    product_name: product.name.clone(),
                    price: product.price,
                    currency: product.currency.clone(),
                    rejection_reason: format!(
                        "Price ${:.2} exceeds customer budget limit of ${:.2}",
                        product.price, requirements.maximum_budget
                    ),
                };
                self.record_rejection(&session_id, &rejection).await?;
                rejections.push(rejection);
                continue;
            }

            // Check mandatory battery life
            if let Some(min_battery) = requirements.minimum_battery_hours {
                if product.specifications.battery_life_hours < min_battery {
                    let rejection = RejectionRecord {
                        product_id: product.product_id.clone(),
                        product_name: product.name.clone(),
                        price: product.price,
                        currency: product.currency.clone(),
                        rejection_reason: format!(
                            "Battery life ({} hours) is below required minimum ({} hours)",
                            product.specifications.battery_life_hours, min_battery
                        ),
                    };
                    self.record_rejection(&session_id, &rejection).await?;
                    rejections.push(rejection);
                    continue;
                }
            }

            // Check mandatory noise cancellation if requested
            let requires_anc = requirements
                .required_features
                .iter()
                .any(|f| f == "noise-canceling");
            if requires_anc && !product.specifications.noise_cancelling {
                let rejection = RejectionRecord {
                    product_id: product.product_id.clone(),
                    product_name: product.name.clone(),
                    price: product.price,
                    currency: product.currency.clone(),
                    rejection_reason: "Product lacks active noise cancellation (ANC)".to_string(),
                };
                self.record_rejection(&session_id, &rejection).await?;
                rejections.push(rejection);
                continue;
            }

            // Check brand requirement if requested
            if let Some(ref brand) = requirements.preferred_brand {
                if !product.brand.eq_ignore_ascii_case(brand) {
                    let rejection = RejectionRecord {
                        product_id: product.product_id.clone(),
                        product_name: product.name.clone(),
                        price: product.price,
                        currency: product.currency.clone(),
                        rejection_reason: format!(
                            "Brand '{}' does not match requested brand '{}'",
                            product.brand, brand
                        ),
                    };
                    self.record_rejection(&session_id, &rejection).await?;
                    rejections.push(rejection);
                    continue;
                }
            }

            // Product satisfies all mandatory requirements; calculate deterministic score
            let brand_matched = requirements
                .preferred_brand
                .as_ref()
                .map(|b| product.brand.eq_ignore_ascii_case(b))
                .unwrap_or(false);

            // Scoring formula:
            // Base = rating * 25.0
            // + battery hours * 1.0
            // + budget savings bonus (remaining budget * 0.1)
            // + brand match bonus (10 points)
            let savings = (requirements.maximum_budget - product.price).max(0.0);
            let score = (product.rating * 25.0)
                + (product.specifications.battery_life_hours as f64 * 1.0)
                + (savings * 0.1)
                + if brand_matched { 10.0 } else { 0.0 };

            let eval = CandidateEvaluation {
                product_id: product.product_id.clone(),
                product_name: product.name.clone(),
                score,
                price: product.price,
                battery_hours: product.specifications.battery_life_hours,
                rating: product.rating,
                brand_matched,
            };

            matching_evaluations.push((product, eval));
        }

        // Record comparison matrix event
        let evaluations_summary: Vec<&CandidateEvaluation> =
            matching_evaluations.iter().map(|(_, e)| e).collect();

        self.ledger_service
            .append_event(
                &session_id,
                AppendEventRequest {
                    action_type: "PRODUCT_COMPARISON".to_string(),
                    payload: serde_json::json!({
                        "valid_candidates_evaluated": matching_evaluations.len(),
                        "evaluations": evaluations_summary,
                    }),
                },
            )
            .await?;

        // =========================================================================
        // MILESTONE 4: FINAL SELECTION
        // =========================================================================
        if matching_evaluations.is_empty() {
            info!(
                session_id = %session_id,
                "No products matched customer constraints; session preserved without selection"
            );
            return Ok(ShoppingSessionResponse {
                success: true,
                session_id: session_id.clone(),
                prompt: trimmed_prompt.to_string(),
                requirements,
                candidates_found: 0,
                rejected_candidates: rejections,
                selected_product: None,
                selection_rationale: Some(
                    "No catalog products matched all mandatory requirements and budget constraints."
                        .to_string(),
                ),
                current_sequence_index: req_event.sequence_index,
                latest_hash: req_event.current_hash,
            });
        }

        // Sort descending by score; if tied, sort ascending by price
        matching_evaluations.sort_by(|a, b| {
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.0.price
                        .partial_cmp(&b.0.price)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        let (best_product, best_eval) = &matching_evaluations[0];
        let rationale = format!(
            "Selected '{}' (Score: {:.2}): Meets all mandatory constraints with {:.1}-star rating, {}h battery, and price of ${:.2} (within ${:.2} budget).",
            best_product.name,
            best_eval.score,
            best_product.rating,
            best_product.specifications.battery_life_hours,
            best_product.price,
            requirements.maximum_budget
        );

        let select_record = SelectionRecord {
            selected_product: best_product.clone(),
            price: best_product.price,
            currency: best_product.currency.clone(),
            selection_rationale: rationale.clone(),
        };

        let select_event = self
            .ledger_service
            .append_event(
                &session_id,
                AppendEventRequest {
                    action_type: "PRODUCT_SELECTED".to_string(),
                    payload: serde_json::to_value(&select_record)?,
                },
            )
            .await?;

        info!(
            session_id = %session_id,
            product_id = %best_product.product_id,
            price = best_product.price,
            "Shopping agent selected matching product"
        );

        Ok(ShoppingSessionResponse {
            success: true,
            session_id,
            prompt: trimmed_prompt.to_string(),
            requirements,
            candidates_found: matching_evaluations.len(),
            rejected_candidates: rejections,
            selected_product: Some(best_product.clone()),
            selection_rationale: Some(rationale),
            current_sequence_index: select_event.sequence_index,
            latest_hash: select_event.current_hash,
        })
    }

    /// Records explicit customer purchase authorization into the cryptographic hash chain.
    pub async fn authorize_purchase(
        &self,
        session_id: &str,
        request: AuthorizePurchaseRequest,
    ) -> Result<AuthorizePurchaseResponse, AppError> {
        if !request.approved {
            return Err(AppError::UnauthorizedPurchase(
                "Customer explicitly declined purchase authorization".to_string(),
            ));
        }

        // Verify session exists and is not finalized
        let session = self.ledger_service.get_session(session_id).await?;
        if session.root_hash.is_some() {
            return Err(AppError::SessionFinalized(format!(
                "Session '{}' is already finalized",
                session_id
            )));
        }

        // Retrieve existing session events
        let events = self.ledger_service.get_session_events(session_id).await?;

        // Prevent duplicate authorizations
        if events
            .iter()
            .any(|e| e.action_type == "PURCHASE_AUTHORIZED")
        {
            return Err(AppError::InvalidPayload(
                "Purchase authorization has already been recorded for this session".to_string(),
            ));
        }

        // Verify selected product from ledger
        let selection_event = events
            .iter()
            .find(|e| e.action_type == "PRODUCT_SELECTED")
            .ok_or_else(|| {
                AppError::InvalidPayload(
                    "Cannot authorize purchase: no product has been selected in this session"
                        .to_string(),
                )
            })?;

        let selection: SelectionRecord = serde_json::from_str(&selection_event.payload)?;

        if selection.selected_product.product_id != request.selected_product_id {
            return Err(AppError::InvalidPayload(format!(
                "Authorized product ID '{}' does not match selected product '{}'",
                request.selected_product_id, selection.selected_product.product_id
            )));
        }

        if request.maximum_authorized_amount < selection.price {
            return Err(AppError::UnauthorizedPurchase(format!(
                "Authorized spending limit (${:.2}) is less than product price (${:.2})",
                request.maximum_authorized_amount, selection.price
            )));
        }

        let now = chrono::Utc::now().to_rfc3339();
        let authorization = PurchaseAuthorization {
            session_id: session_id.to_string(),
            selected_product_id: request.selected_product_id,
            maximum_authorized_amount: request.maximum_authorized_amount,
            currency: request.currency,
            authorization_status: AuthorizationStatus::Approved,
            authorization_timestamp: now,
            authorization_method: "CUSTOMER_EXPLICIT_APPROVAL".to_string(),
        };

        let append_res = self
            .ledger_service
            .append_event(
                session_id,
                AppendEventRequest {
                    action_type: "PURCHASE_AUTHORIZED".to_string(),
                    payload: serde_json::to_value(&authorization)?,
                },
            )
            .await?;

        info!(
            session_id = %session_id,
            authorized_amount = authorization.maximum_authorized_amount,
            "Purchase authorization recorded in cryptographic ledger"
        );

        Ok(AuthorizePurchaseResponse {
            success: true,
            session_id: session_id.to_string(),
            authorization,
            sequence_index: append_res.sequence_index,
            current_hash: append_res.current_hash,
        })
    }

    /// Retrieves detailed status of a shopping session, including all milestones and verification.
    pub async fn get_shopping_session_detail(
        &self,
        session_id: &str,
    ) -> Result<ShoppingSessionDetail, AppError> {
        let session = self.ledger_service.get_session(session_id).await?;
        let events = self.ledger_service.get_session_events(session_id).await?;
        let verification = self.verification_service.verify_session(session_id).await?;

        let mut requirements = None;
        let mut selected_product = None;
        let mut authorization = None;

        for event in &events {
            match event.action_type.as_str() {
                "REQUIREMENTS_EXTRACTED" => {
                    if let Ok(req) = serde_json::from_str::<ShoppingRequirements>(&event.payload) {
                        requirements = Some(req);
                    }
                }
                "PRODUCT_SELECTED" => {
                    if let Ok(sel) = serde_json::from_str::<SelectionRecord>(&event.payload) {
                        selected_product = Some(sel.selected_product);
                    }
                }
                "PURCHASE_AUTHORIZED" => {
                    if let Ok(auth) = serde_json::from_str::<PurchaseAuthorization>(&event.payload)
                    {
                        authorization = Some(auth);
                    }
                }
                _ => {}
            }
        }

        Ok(ShoppingSessionDetail {
            session_id: session.session_id,
            initial_prompt: session.initial_prompt,
            requirements,
            selected_product,
            authorization,
            paypal_order_id: session.paypal_order_id,
            is_finalized: session.root_hash.is_some(),
            root_hash: session.root_hash,
            event_count: events.len(),
            verification,
        })
    }

    /// Helper to record an individual product rejection event in the ledger.
    async fn record_rejection(
        &self,
        session_id: &str,
        rejection: &RejectionRecord,
    ) -> Result<(), AppError> {
        self.ledger_service
            .append_event(
                session_id,
                AppendEventRequest {
                    action_type: "PRODUCT_REJECTED".to_string(),
                    payload: serde_json::to_value(rejection)?,
                },
            )
            .await?;
        Ok(())
    }
}
