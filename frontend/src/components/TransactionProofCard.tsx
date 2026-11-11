import React from 'react';
import type { TransactionProof } from '../types/api';

interface TransactionProofCardProps {
  transactionProof?: TransactionProof;
}

export const TransactionProofCard: React.FC<TransactionProofCardProps> = ({
  transactionProof,
}) => {
  if (!transactionProof) {
    return (
      <div className="section-card">
        <div className="section-card-header">
          <div className="section-title-wrap">
            <span className="epistemic-tag unavailable">UNAVAILABLE_EVIDENCE</span>
            <h3 className="section-card-title">4. Transaction Proof & Settlement</h3>
          </div>
        </div>
        <div className="empty-state-notice">
          <p>No transaction settlement proof linked to this dispute brief.</p>
        </div>
      </div>
    );
  }

  const { paypal_settlement, ledger_settlement } = transactionProof;

  return (
    <div className="section-card">
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag paypal-data">RECORDED_PAYPAL_DATA</span>
          <h3 className="section-card-title">4. Transaction Proof & Settlement</h3>
        </div>
        <div className="header-badges">
          <span
            className={`provenance-pill ${
              paypal_settlement.provenance === 'REPLAYED_FROM_LEDGER_PAYLOAD'
                ? 'replayed'
                : 'live'
            }`}
          >
            Provenance: {paypal_settlement.provenance}
          </span>
        </div>
      </div>

      {/* Epistemic Provenance Disclaimer */}
      <div className="banner-notice info">
        <div className="banner-icon">ℹ️</div>
        <div>
          <strong>Provenance Transparency Notice:</strong>{' '}
          {paypal_settlement.provenance === 'REPLAYED_FROM_LEDGER_PAYLOAD' ? (
            <span>
              PayPal order and capture values shown below were extracted from the JSON payloads of internal ledger steps.
              They are classified as <code>RECORDED_PAYPAL_DATA</code> because they reflect PayPal-formatted records,
              but are not direct live REST API queries from this endpoint.
            </span>
          ) : (
            <span>
              Retrieved directly from authoritative live PayPal REST API endpoint.
            </span>
          )}
        </div>
      </div>

      <div className="trans-proof-grid">
        {/* PayPal Recorded Attributes */}
        <div className="trans-subcard">
          <div className="subcard-header">
            <span className="subcard-title">Recorded PayPal Settlement</span>
            <span className="epistemic-tag paypal-data text-xs">RECORDED_PAYPAL_DATA</span>
          </div>
          <div className="subcard-content">
            <div className="crypto-field">
              <span className="field-lbl">PayPal Order ID:</span>
              <span className="field-val mono">{paypal_settlement.paypal_order_id || 'N/A'}</span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Order Status:</span>
              <span className="field-val">{paypal_settlement.order_status || 'N/A'}</span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Order Amount:</span>
              <span className="field-val amount">
                {paypal_settlement.order_amount && paypal_settlement.order_currency
                  ? `${paypal_settlement.order_amount} ${paypal_settlement.order_currency}`
                  : 'N/A'}
              </span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">PayPal Capture ID:</span>
              <span className="field-val mono highlight">{paypal_settlement.paypal_capture_id || 'N/A'}</span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Capture Status:</span>
              <span className="field-val status-text">{paypal_settlement.capture_status || 'N/A'}</span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Capture Timestamp:</span>
              <span className="field-val mono text-xs">{paypal_settlement.capture_timestamp || 'N/A'}</span>
            </div>
          </div>
        </div>

        {/* Ledger Settlement Anchors */}
        <div className="trans-subcard">
          <div className="subcard-header">
            <span className="subcard-title">Verified Ledger Settlement Steps</span>
            <span className="epistemic-tag verified-fact text-xs">VERIFIED_FACT</span>
          </div>
          <div className="subcard-content">
            <div className="settle-step-box">
              <span className="step-tag-lbl">Order Created Step:</span>
              {ledger_settlement.order_created_step ? (
                <div className="step-info-row">
                  <div>Seq: <strong>{ledger_settlement.order_created_step.sequence_index}</strong></div>
                  <div>Hash: <code>{ledger_settlement.order_created_step.step_hash?.slice(0, 16)}...</code></div>
                  <div className="text-xs text-muted">Time: {ledger_settlement.order_created_step.timestamp}</div>
                </div>
              ) : (
                <span className="text-muted">Not recorded in ledger</span>
              )}
            </div>

            <div className="settle-step-box">
              <span className="step-tag-lbl">Payment Captured Step:</span>
              {ledger_settlement.payment_captured_step ? (
                <div className="step-info-row">
                  <div>Seq: <strong>{ledger_settlement.payment_captured_step.sequence_index}</strong></div>
                  <div>Hash: <code>{ledger_settlement.payment_captured_step.step_hash?.slice(0, 16)}...</code></div>
                  <div className="text-xs text-muted">Time: {ledger_settlement.payment_captured_step.timestamp}</div>
                </div>
              ) : (
                <span className="text-muted">Not recorded in ledger</span>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};
