use std::sync::Arc;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::error::AppError;
use crate::models::authorization::PurchaseAuthorization;
use crate::models::ledger::AppendEventRequest;
use crate::models::paypal_orders::{
    CaptureOrderResponse, CreateOrderResponse, PayPalAmount, PayPalCaptureOrderResponse,
    PayPalCreateOrderRequest, PayPalCreateOrderResponse, PayPalExperienceContext,
    PayPalExperienceWrapper, PayPalOrderDetails, PayPalOrderStatusResponse, PayPalPaymentSource,
    PayPalPurchaseUnitRequest,
};
use crate::models::shopping::SelectionRecord;
use crate::services::auth::PayPalAuthService;
use crate::services::ledger::LedgerService;

/// Service for managing PayPal Orders v2 API interactions and recording verifiable payment milestones.
#[derive(Clone)]
pub struct PayPalOrdersService {
    config: Arc<Config>,
    http_client: reqwest::Client,
    auth_service: Arc<PayPalAuthService>,
    ledger_service: Arc<LedgerService>,
}

impl PayPalOrdersService {
    /// Creates a new `PayPalOrdersService` instance.
    pub fn new(
        config: Arc<Config>,
        http_client: reqwest::Client,
        auth_service: Arc<PayPalAuthService>,
        ledger_service: Arc<LedgerService>,
    ) -> Self {
        Self {
            config,
            http_client,
            auth_service,
            ledger_service,
        }
    }

    /// Creates a PayPal v2 checkout order with CAPTURE intent after verifying customer authorization.
    pub async fn create_order(&self, session_id: &str) -> Result<CreateOrderResponse, AppError> {
        let session = self.ledger_service.get_session(session_id).await?;
        if session.root_hash.is_some() {
            return Err(AppError::SessionFinalized(format!(
                "Session '{}' is already finalized",
                session_id
            )));
        }

        // Idempotency / duplicate check: verify order has not already been created
        if let Some(existing_order_id) = session.paypal_order_id {
            return Err(AppError::InvalidPayload(format!(
                "PayPal order '{}' has already been created for this session",
                existing_order_id
            )));
        }

        // Fetch session events to verify selection and purchase authorization
        let events = self.ledger_service.get_session_events(session_id).await?;

        let selection_event = events
            .iter()
            .find(|e| e.action_type == "PRODUCT_SELECTED")
            .ok_or_else(|| {
                AppError::InvalidPayload(
                    "Cannot create PayPal order: no product selected in this session".to_string(),
                )
            })?;
        let selection: SelectionRecord = serde_json::from_str(&selection_event.payload)?;

        let auth_event = events
            .iter()
            .find(|e| e.action_type == "PURCHASE_AUTHORIZED")
            .ok_or_else(|| {
                AppError::UnauthorizedPurchase(
                    "Cannot create PayPal order: explicit customer purchase authorization is required before checkout"
                        .to_string(),
                )
            })?;
        let authorization: PurchaseAuthorization = serde_json::from_str(&auth_event.payload)?;

        // Ensure authorization matches product and covers price
        if authorization.selected_product_id != selection.selected_product.product_id {
            return Err(AppError::UnauthorizedPurchase(
                "Authorized product ID does not match selected product".to_string(),
            ));
        }

        if authorization.maximum_authorized_amount < selection.price {
            return Err(AppError::UnauthorizedPurchase(format!(
                "Authorized limit (${:.2}) is less than product price (${:.2})",
                authorization.maximum_authorized_amount, selection.price
            )));
        }

        // Get OAuth access token
        let token = self.auth_service.get_valid_access_token().await?;

        let url = format!("{}/v2/checkout/orders", self.config.paypal_base_url);
        let formatted_price = format!("{:.2}", selection.price);

        let order_req = PayPalCreateOrderRequest {
            intent: "CAPTURE".to_string(),
            purchase_units: vec![PayPalPurchaseUnitRequest {
                reference_id: format!("session-{}", session_id),
                description: selection.selected_product.name.clone(),
                custom_id: session_id.to_string(),
                amount: PayPalAmount {
                    currency_code: selection.currency.clone(),
                    value: formatted_price.clone(),
                },
            }],
            payment_source: Some(PayPalPaymentSource {
                paypal: PayPalExperienceWrapper {
                    experience_context: PayPalExperienceContext {
                        brand_name: Some("AgenticCommerce".to_string()),
                        locale: Some("en-US".to_string()),
                        landing_page: Some("LOGIN".to_string()),
                        user_action: Some("PAY_NOW".to_string()),
                        payment_method_preference: Some("IMMEDIATE_PAYMENT_REQUIRED".to_string()),
                        return_url: Some(format!(
                            "http://127.0.0.1:{}/api/shopping/checkout/success",
                            self.config.port
                        )),
                        cancel_url: Some(format!(
                            "http://127.0.0.1:{}/api/shopping/checkout/cancel",
                            self.config.port
                        )),
                    },
                },
            }),
        };

        debug!(
            session_id = %session_id,
            url = %url,
            "Sending POST /v2/checkout/orders to PayPal Sandbox"
        );

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .header("PayPal-Request-Id", format!("{}-order", session_id))
            .json(&order_req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            error!(
                status = %status,
                body = %err_body,
                "PayPal Orders API returned error"
            );
            return Err(AppError::PayPalApi(format!(
                "PayPal Orders API error ({}): {}",
                status, err_body
            )));
        }

        let order_resp: PayPalCreateOrderResponse = resp.json().await?;

        // Extract buyer approval URL
        let approval_url = order_resp
            .links
            .iter()
            .find(|l| l.rel == "payer-action" || l.rel == "approve")
            .map(|l| l.href.clone());

        // Update database with PayPal order ID
        self.ledger_service
            .set_paypal_order_id(session_id, &order_resp.id)
            .await?;

        // Record order creation in the cryptographic ledger
        self.ledger_service
            .append_event(
                session_id,
                AppendEventRequest {
                    action_type: "PAYPAL_ORDER_CREATED".to_string(),
                    payload: serde_json::json!({
                        "order_id": order_resp.id,
                        "status": order_resp.status,
                        "intent": "CAPTURE",
                        "product_id": selection.selected_product.product_id,
                        "amount": formatted_price,
                        "currency": selection.currency,
                        "approval_url": approval_url,
                    }),
                },
            )
            .await?;

        info!(
            session_id = %session_id,
            order_id = %order_resp.id,
            "PayPal Sandbox order created successfully"
        );

        Ok(CreateOrderResponse {
            success: true,
            session_id: session_id.to_string(),
            order_id: order_resp.id,
            status: order_resp.status,
            approval_url,
        })
    }

    /// Captures payment for an approved PayPal Sandbox order and finalizes the audit session.
    pub async fn capture_order(&self, session_id: &str) -> Result<CaptureOrderResponse, AppError> {
        let session = self.ledger_service.get_session(session_id).await?;
        if session.root_hash.is_some() {
            return Err(AppError::SessionFinalized(format!(
                "Session '{}' is already finalized",
                session_id
            )));
        }

        let order_id = session.paypal_order_id.ok_or_else(|| {
            AppError::InvalidPayload(
                "Cannot capture payment: no PayPal order has been created for this session"
                    .to_string(),
            )
        })?;

        let token = self.auth_service.get_valid_access_token().await?;

        let url = format!(
            "{}/v2/checkout/orders/{}/capture",
            self.config.paypal_base_url, order_id
        );

        debug!(
            session_id = %session_id,
            order_id = %order_id,
            url = %url,
            "Sending POST capture request to PayPal Sandbox"
        );

        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .header("PayPal-Request-Id", format!("{}-capture", session_id))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            error!(
                status = %status,
                order_id = %order_id,
                body = %err_body,
                "PayPal order capture rejected"
            );

            // Record failed capture attempt in ledger
            let _ = self
                .ledger_service
                .append_event(
                    session_id,
                    AppendEventRequest {
                        action_type: "PAYPAL_PAYMENT_FAILED".to_string(),
                        payload: serde_json::json!({
                            "order_id": order_id,
                            "http_status": status.as_u16(),
                            "error": err_body,
                        }),
                    },
                )
                .await;

            return Err(AppError::OrderCaptureFailed(format!(
                "PayPal capture failed ({}): {}. Ensure buyer approved order in Sandbox.",
                status, err_body
            )));
        }

        let capture_resp: PayPalCaptureOrderResponse = resp.json().await?;

        if capture_resp.status != "COMPLETED" {
            error!(
                order_id = %order_id,
                capture_status = %capture_resp.status,
                "PayPal order capture not completed"
            );

            let _ = self
                .ledger_service
                .append_event(
                    session_id,
                    AppendEventRequest {
                        action_type: "PAYPAL_PAYMENT_PENDING".to_string(),
                        payload: serde_json::json!({
                            "order_id": order_id,
                            "capture_status": capture_resp.status,
                        }),
                    },
                )
                .await;

            return Err(AppError::OrderCaptureFailed(format!(
                "PayPal capture status is '{}'; payment has not completed",
                capture_resp.status
            )));
        }

        // Extract capture ID
        let capture_id = capture_resp
            .purchase_units
            .first()
            .and_then(|pu| pu.payments.as_ref())
            .and_then(|pm| pm.captures.first())
            .map(|c| c.id.clone());

        // Record confirmed capture in cryptographic ledger
        self.ledger_service
            .append_event(
                session_id,
                AppendEventRequest {
                    action_type: "PAYPAL_PAYMENT_CAPTURED".to_string(),
                    payload: serde_json::json!({
                        "order_id": order_id,
                        "capture_id": capture_id,
                        "status": "COMPLETED",
                        "verified": true,
                    }),
                },
            )
            .await?;

        // Finalize session: locking the root hash
        let finalization = self.ledger_service.finalize_session(session_id).await?;

        info!(
            session_id = %session_id,
            order_id = %order_id,
            capture_id = ?capture_id,
            root_hash = %finalization.root_hash,
            "PayPal order captured and audit session finalized successfully"
        );

        Ok(CaptureOrderResponse {
            success: true,
            session_id: session_id.to_string(),
            order_id,
            capture_id,
            status: capture_resp.status,
            final_root_hash: finalization.root_hash,
        })
    }

    /// Queries the authoritative PayPal Orders v2 status for a session.
    pub async fn get_order_status(
        &self,
        session_id: &str,
    ) -> Result<PayPalOrderStatusResponse, AppError> {
        let session = self.ledger_service.get_session(session_id).await?;
        let order_id = session.paypal_order_id.ok_or_else(|| {
            AppError::InvalidPayload(
                "Cannot check order status: no PayPal order has been created for this session"
                    .to_string(),
            )
        })?;

        let token = self.auth_service.get_valid_access_token().await?;

        let url = format!(
            "{}/v2/checkout/orders/{}",
            self.config.paypal_base_url, order_id
        );

        debug!(
            session_id = %session_id,
            order_id = %order_id,
            url = %url,
            "Querying GET /v2/checkout/orders/:id from PayPal Sandbox"
        );

        let resp = self
            .http_client
            .get(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            error!(
                status = %status,
                order_id = %order_id,
                body = %err_body,
                "PayPal order status lookup failed"
            );
            return Err(AppError::PayPalApi(format!(
                "PayPal order status lookup error ({}): {}",
                status, err_body
            )));
        }

        let details: PayPalOrderDetails = resp.json().await?;
        let is_approved = details.status == "APPROVED";
        let is_completed = details.status == "COMPLETED";
        let payer_email = details.payer.as_ref().and_then(|p| p.email_address.clone());
        let payer_id = details.payer.as_ref().and_then(|p| p.payer_id.clone());

        info!(
            session_id = %session_id,
            order_id = %details.id,
            order_status = %details.status,
            is_approved = is_approved,
            is_completed = is_completed,
            "Authoritative PayPal order status retrieved"
        );

        Ok(PayPalOrderStatusResponse {
            success: true,
            session_id: session_id.to_string(),
            order_id: details.id,
            status: details.status,
            is_approved,
            is_completed,
            payer_email,
            payer_id,
        })
    }
}
