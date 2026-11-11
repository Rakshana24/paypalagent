import React from 'react';
import type { DisputeSummaryFact, CorrelationSummary } from '../types/api';

interface DisputeSummaryCardProps {
  disputeSummary: DisputeSummaryFact;
  correlationSummary: CorrelationSummary;
}

export const DisputeSummaryCard: React.FC<DisputeSummaryCardProps> = ({
  disputeSummary,
  correlationSummary,
}) => {
  const isInternalInquiry =
    disputeSummary.dispute_lifecycle_stage?.toUpperCase() === 'INQUIRY' ||
    disputeSummary.dispute_channel?.toUpperCase() === 'INTERNAL';

  const isExternalChargeback =
    disputeSummary.dispute_lifecycle_stage?.toUpperCase() === 'CHARGEBACK' ||
    disputeSummary.dispute_channel?.toUpperCase() === 'EXTERNAL';

  return (
    <div className="section-card">
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag paypal-data">RECORDED_PAYPAL_DATA</span>
          <h3 className="section-card-title">1. PayPal Dispute & Correlation Summary</h3>
        </div>
        <div className="header-badges">
          <span className={`status-badge-outline ${correlationSummary.status.toLowerCase()}`}>
            Correlation: {correlationSummary.status}
          </span>
          {correlationSummary.matched_by && (
            <span className="status-badge-outline neutral">
              Matched By: {correlationSummary.matched_by}
            </span>
          )}
        </div>
      </div>

      {/* Case Stage & Channel Clarification Banner */}
      {isInternalInquiry ? (
        <div className="banner-notice info">
          <div className="banner-icon">ℹ️</div>
          <div>
            <strong>Case Stage Notice: Seller-Resolution / Buyer Inquiry</strong>
            <p>
              This case is recorded in the internal <strong>Inquiry (Seller-Resolution)</strong> stage.
              The buyer and merchant communicate directly to reach an amicable resolution.
              It is <em>not</em> an escalated formal PayPal claim or bank chargeback.
            </p>
          </div>
        </div>
      ) : isExternalChargeback ? (
        <div className="banner-notice warning">
          <div className="banner-icon">⚠️</div>
          <div>
            <strong>Case Stage Notice: Formal Bank Chargeback</strong>
            <p>
              This dispute represents an external financial institution chargeback initiated through the buyer's card issuer.
            </p>
          </div>
        </div>
      ) : (
        <div className="banner-notice neutral">
          <div className="banner-icon">📋</div>
          <div>
            <strong>Case Stage: {disputeSummary.dispute_lifecycle_stage || 'Standard Dispute'}</strong>
            <p>Formal PayPal dispute requiring merchant response.</p>
          </div>
        </div>
      )}

      {/* Metadata Table */}
      <div className="meta-grid">
        <div className="meta-item">
          <span className="meta-label">Dispute Identifier</span>
          <span className="meta-value mono highlight">{disputeSummary.dispute_id}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Dispute Status</span>
          <span className="meta-value status-text">{disputeSummary.status}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Dispute Reason</span>
          <span className="meta-value mono">{disputeSummary.reason || 'Not Specified'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Disputed Amount</span>
          <span className="meta-value amount">
            {disputeSummary.dispute_amount_value && disputeSummary.dispute_amount_currency
              ? `${disputeSummary.dispute_amount_value} ${disputeSummary.dispute_amount_currency}`
              : 'N/A'}
          </span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Dispute Channel</span>
          <span className="meta-value">{disputeSummary.dispute_channel || 'N/A'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Lifecycle Stage</span>
          <span className="meta-value">{disputeSummary.dispute_lifecycle_stage || 'N/A'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">PayPal Order ID</span>
          <span className="meta-value mono">{disputeSummary.paypal_order_id || 'None'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Seller Transaction (Capture)</span>
          <span className="meta-value mono">{disputeSummary.seller_transaction_id || 'None'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Opened Timestamp</span>
          <span className="meta-value mono text-sm">{disputeSummary.dispute_create_time || 'N/A'}</span>
        </div>
        <div className="meta-item">
          <span className="meta-label">Updated Timestamp</span>
          <span className="meta-value mono text-sm">{disputeSummary.dispute_update_time || 'N/A'}</span>
        </div>
      </div>

      {/* Correlation Explanation */}
      <div className="correlation-box">
        <div className="correlation-box-header">
          <span className="epistemic-tag deterministic">DETERMINISTIC_INFERENCE</span>
          <span className="evaluator-label">Evaluator: {correlationSummary.evaluator}</span>
        </div>
        <p className="correlation-explanation">{correlationSummary.explanation}</p>
        {correlationSummary.session_id && (
          <div className="linked-session-tag">
            Linked Session: <code>{correlationSummary.session_id}</code>
          </div>
        )}
      </div>
    </div>
  );
};
