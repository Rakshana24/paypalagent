import React, { useState } from 'react';
import type {
  CreateOrderResponse,
  CaptureOrderResponse,
  PayPalOrderStatusResponse,
  PurchaseAuthorization,
} from '../types/api';

interface PayPalCheckoutCardProps {
  sessionId: string;
  authorization: PurchaseAuthorization | null;
  order: CreateOrderResponse | null;
  orderStatus: PayPalOrderStatusResponse | null;
  capture: CaptureOrderResponse | null;
  isCreatingOrder: boolean;
  isCheckingStatus: boolean;
  isCapturing: boolean;
  error: string | null;
  onCreateOrder: () => Promise<unknown>;
  onOpenApproval: () => Window | null;
  onCheckStatus: () => Promise<unknown>;
  onCapture: () => Promise<unknown>;
  onViewInNotary?: (sessionId: string) => void;
}

export const PayPalCheckoutCard: React.FC<PayPalCheckoutCardProps> = ({
  sessionId,
  authorization,
  order,
  orderStatus,
  capture,
  isCreatingOrder,
  isCheckingStatus,
  isCapturing,
  error,
  onCreateOrder,
  onOpenApproval,
  onCheckStatus,
  onCapture,
  onViewInNotary,
}) => {
  const [popupOpened, setPopupOpened] = useState<boolean>(false);
  const [copiedUrl, setCopiedUrl] = useState<boolean>(false);

  const isCaptured = !!capture && capture.status === 'COMPLETED';
  const hasOrder = !!order && !!order.order_id;
  const isApproved = !!orderStatus && orderStatus.is_approved;

  const handleLaunchApproval = () => {
    const pop = onOpenApproval();
    if (pop) {
      setPopupOpened(true);
    }
  };

  const handleCopyApprovalUrl = () => {
    if (order?.approval_url) {
      void navigator.clipboard.writeText(order.approval_url);
      setCopiedUrl(true);
      setTimeout(() => setCopiedUrl(false), 2000);
    }
  };

  return (
    <div className={`section-card paypal-checkout-card ${isCaptured ? 'card-captured' : hasOrder ? 'card-order-active' : ''}`}>
      <div className="section-card-header">
        <div className="card-header-titles">
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
            <span className={`section-badge ${isCaptured ? 'success' : hasOrder ? 'info' : 'primary'}`}>
              {isCaptured ? '✓ Payment Settled' : hasOrder ? 'Step 5 • PayPal Settlement' : 'Step 5 • PayPal Order Creation'}
            </span>
            <span className="step-pill">
              {isCaptured
                ? 'Ledger Finalized • PAYPAL_PAYMENT_CAPTURED'
                : hasOrder
                ? `Order ID: ${order.order_id}`
                : 'Awaiting Order Creation'}
            </span>
          </div>
          <h3 className="section-card-title" style={{ marginTop: '0.25rem' }}>
            PayPal Sandbox Checkout & Settlement
          </h3>
          <span className="section-card-subtitle">
            Authoritative two-phase settlement: customer-authorized order creation, sandbox buyer approval confirmation, and cryptographic ledger finalization
          </span>
        </div>
      </div>

      {/* Error Banner */}
      {error && (
        <div className="alert-card alert-error" role="alert" style={{ marginBottom: '1rem' }}>
          <strong>Payment Gateway Error: </strong> {error}
        </div>
      )}

      {/* STAGE 1: ORDER NOT CREATED YET */}
      {!hasOrder && !isCaptured && (
        <div className="checkout-init-section">
          <div className="init-notice-box">
            <span className="notice-icon" aria-hidden="true">🔒</span>
            <div className="notice-body">
              <h4>Customer Authorization Verified</h4>
              <p>
                Authorization ceiling of{' '}
                <strong>${authorization?.maximum_authorized_amount.toFixed(2)} {authorization?.currency}</strong> has been committed to the hash chain. You may now create the PayPal Sandbox order.
              </p>
            </div>
          </div>

          <div className="checkout-actions-row" style={{ marginTop: '1.25rem' }}>
            <button
              type="button"
              className="btn-primary"
              onClick={() => void onCreateOrder()}
              disabled={isCreatingOrder || !authorization}
            >
              {isCreatingOrder ? (
                <>
                  <span className="spinner-inline" aria-hidden="true" />
                  <span>Creating PayPal Order...</span>
                </>
              ) : (
                <>
                  <span>Create PayPal Sandbox Order</span>
                  <span className="btn-arrow" aria-hidden="true">→</span>
                </>
              )}
            </button>
          </div>
        </div>
      )}

      {/* STAGE 2: ORDER CREATED, AWAITING AUTHORITATIVE APPROVAL & CAPTURE */}
      {hasOrder && !isCaptured && (
        <div className="checkout-active-section">
          {/* Order Metadata Tile */}
          <div className="order-metadata-tile">
            <div className="order-meta-item">
              <span className="meta-label">PayPal Order ID</span>
              <code className="meta-code">{order.order_id}</code>
            </div>
            <div className="order-meta-item">
              <span className="meta-label">Order Intent</span>
              <span className="meta-value">CAPTURE</span>
            </div>
            <div className="order-meta-item">
              <span className="meta-label">Initial Order Status</span>
              <span className="meta-badge-status">{order.status}</span>
            </div>
            <div className="order-meta-item">
              <span className="meta-label">Audit Session ID</span>
              <code className="meta-code">{sessionId.slice(0, 8)}...</code>
            </div>
          </div>

          {/* Sandbox Buyer Action Box */}
          <div className="sandbox-approval-box">
            <div className="approval-box-header">
              <span className="paypal-logo-text">PayPal</span>
              <span className="sandbox-env-tag">Sandbox Environment</span>
            </div>

            <p className="approval-instructions">
              To complete the demonstration, open the PayPal Sandbox checkout window and approve the payment with your Sandbox Personal buyer account.
            </p>

            <div className="approval-launch-row">
              <button
                type="button"
                className="btn-paypal-sandbox"
                onClick={handleLaunchApproval}
                disabled={!order.approval_url}
              >
                <span>↗ Open PayPal Sandbox Approval Window</span>
              </button>

              {order.approval_url && (
                <button
                  type="button"
                  className="btn-secondary btn-sm"
                  onClick={handleCopyApprovalUrl}
                >
                  {copiedUrl ? '✓ URL Copied' : 'Copy Approval URL'}
                </button>
              )}
            </div>

            {popupOpened && (
              <div className="popup-hint-note">
                <span className="info-icon">ℹ</span>
                <span>
                  Approval window launched. After signing into PayPal Sandbox and clicking <strong>Continue</strong> or <strong>Pay Now</strong>, return here and click <strong>Check Approval Status</strong> below.
                </span>
              </div>
            )}
          </div>

          {/* Authoritative Approval Verification Section */}
          <div className="authoritative-check-section">
            <div className="check-section-header">
              <div>
                <h4 className="check-title">Authoritative Approval Verification</h4>
                <p className="check-sub">
                  Cryptographic safety invariant: Payment capture is only unlocked when PayPal Orders v2 authoritatively confirms <code>status == "APPROVED"</code>.
                </p>
              </div>

              <button
                type="button"
                className="btn-secondary btn-sm check-status-btn"
                onClick={() => void onCheckStatus()}
                disabled={isCheckingStatus}
              >
                {isCheckingStatus ? (
                  <>
                    <span className="spinner-inline" aria-hidden="true" />
                    <span>Querying PayPal API...</span>
                  </>
                ) : (
                  <>
                    <span>↻ Check Approval Status</span>
                  </>
                )}
              </button>
            </div>

            {/* Status Display Pill */}
            {orderStatus ? (
              <div className={`approval-status-banner ${orderStatus.is_approved ? 'status-banner-approved' : 'status-banner-pending'}`}>
                <div className="status-banner-top">
                  <span className="status-icon">
                    {orderStatus.is_approved ? '✓' : '⏳'}
                  </span>
                  <div>
                    <strong className="status-title-text">
                      {orderStatus.is_approved
                        ? 'Buyer Approval Confirmed by PayPal Orders v2'
                        : `Approval Pending: ${orderStatus.status}`}
                    </strong>
                    <p className="status-explanation">
                      {orderStatus.is_approved
                        ? `PayPal returned status = "APPROVED". Buyer approval is authoritatively verified. Ready for capture.`
                        : `PayPal returned status = "${orderStatus.status}". Payer action is required in the Sandbox window before capture can be executed.`}
                    </p>
                  </div>
                </div>

                {orderStatus.payer_email && (
                  <div className="payer-info-row">
                    <span>Verified Sandbox Payer:</span>
                    <strong>{orderStatus.payer_email}</strong>
                    {orderStatus.payer_id && <code className="payer-id-pill">ID: {orderStatus.payer_id}</code>}
                  </div>
                )}
              </div>
            ) : (
              <div className="approval-status-banner status-banner-unverified">
                <span className="status-icon">ℹ</span>
                <p style={{ fontSize: '0.8125rem', color: '#475569' }}>
                  Click <strong>Check Approval Status</strong> after approving the transaction in the PayPal Sandbox window.
                </p>
              </div>
            )}
          </div>

          {/* Capture Trigger Section */}
          <div className="capture-action-section">
            <div className="capture-info-text">
              <span className="capture-step-title">Execute Capture & Freeze Ledger Hash</span>
              <p className="capture-step-desc">
                Finalizes payment with PayPal, computes the root SHA-256 hash, and locks the audit session ledger.
              </p>
            </div>

            <button
              type="button"
              className="btn-primary btn-capture"
              onClick={() => void onCapture()}
              disabled={!isApproved || isCapturing}
            >
              {isCapturing ? (
                <>
                  <span className="spinner-inline" aria-hidden="true" />
                  <span>Capturing Payment with PayPal...</span>
                </>
              ) : (
                <>
                  <span>Capture Payment & Finalize Ledger</span>
                  <span className="btn-arrow" aria-hidden="true">→</span>
                </>
              )}
            </button>
          </div>

          {!isApproved && (
            <p className="capture-disabled-note">
              • Capture button is disabled until authoritative buyer approval is confirmed by PayPal Orders v2 API.
            </p>
          )}
        </div>
      )}

      {/* STAGE 3: PAYMENT CAPTURED & LEDGER FINALIZED */}
      {isCaptured && capture && (
        <div className="checkout-captured-section">
          <div className="captured-hero-banner">
            <div className="captured-icon-circle" aria-hidden="true">✓</div>
            <div className="captured-hero-body">
              <h4 className="captured-hero-title">Payment Captured & Ledger Finalized</h4>
              <p className="captured-hero-sub">
                Funds successfully settled in PayPal Sandbox. Audit session hash chain permanently locked and anchored.
              </p>
            </div>
          </div>

          <div className="capture-summary-grid">
            <div className="summary-card-item">
              <span className="card-item-label">Capture Transaction ID</span>
              <code className="card-item-val capture-id-code">
                {capture.capture_id || 'Captured by PayPal'}
              </code>
              <span className="card-item-meta">Live PayPal capture reference</span>
            </div>

            <div className="summary-card-item">
              <span className="card-item-label">Settlement Status</span>
              <span className="settlement-status-badge">✓ COMPLETED</span>
              <span className="card-item-meta">PayPal Orders v2 verified</span>
            </div>

            <div className="summary-card-item">
              <span className="card-item-label">Final Ledger Root Hash</span>
              <code className="card-item-val hash-val" title={capture.final_root_hash}>
                {capture.final_root_hash.slice(0, 20)}...
              </code>
              <span className="card-item-meta">SHA-256 canonical Merkle root</span>
            </div>

            <div className="summary-card-item">
              <span className="card-item-label">Digital Notary Verification</span>
              <span className="notary-status-badge">✓ Cryptographically Valid</span>
              <span className="card-item-meta">Dual-chain admissible evidence</span>
            </div>
          </div>

          {/* Action Row: Inspect in Notary */}
          {onViewInNotary && (
            <div className="captured-cta-row">
              <button
                type="button"
                className="btn-primary view-notary-btn"
                onClick={() => onViewInNotary(sessionId)}
              >
                <span>Inspect Cryptographic Ledger in Digital Notary</span>
                <span className="btn-arrow" aria-hidden="true">→</span>
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
