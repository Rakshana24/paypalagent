import React from 'react';
import type { AgentAuditTrailSummary, TransactionProof } from '../types/api';

interface AgentDecisionTimelineProps {
  trail?: AgentAuditTrailSummary;
  trans?: TransactionProof;
}

export const AgentDecisionTimeline: React.FC<AgentDecisionTimelineProps> = ({
  trail,
  trans,
}) => {
  if (!trail) {
    return (
      <div className="section-card">
        <div className="section-card-header">
          <div className="section-title-wrap">
            <span className="epistemic-tag unavailable">UNAVAILABLE_EVIDENCE</span>
            <h3 className="section-card-title">3. Autonomous Agent Decision Timeline</h3>
          </div>
        </div>
        <div className="empty-state-notice">
          <p>
            No purchase session audit trail is linked to this dispute brief.
            The dispute is either uncorrelated, manual review required, or purchase ledger verification failed.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="section-card">
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag verified-fact">VERIFIED_FACT</span>
          <h3 className="section-card-title">3. Autonomous Agent Decision Timeline</h3>
        </div>
        <span className="section-card-subtitle">
          Sequential cryptographically chained audit events from customer prompt to payment capture
        </span>
      </div>

      <div className="timeline-container">
        {/* Step 1: Initial Prompt */}
        <div className="timeline-item">
          <div className="timeline-marker">
            <span className="marker-number">{trail.initial_prompt.source.sequence_index ?? 0}</span>
          </div>
          <div className="timeline-content">
            <div className="timeline-item-header">
              <span className="action-tag">INITIAL_PROMPT</span>
              <span className="timeline-timestamp">
                {trail.initial_prompt.source.timestamp || 'Recorded in ledger'}
              </span>
            </div>
            <p className="timeline-text prompt-quote">
              &ldquo;{trail.initial_prompt.prompt}&rdquo;
            </p>
            <div className="source-ref-badge">
              Step Hash: <code>{trail.initial_prompt.source.step_hash?.slice(0, 16)}...</code>
            </div>
          </div>
        </div>

        {/* Step 2: Extracted Requirements */}
        {trail.requirements && (
          <div className="timeline-item">
            <div className="timeline-marker">
              <span className="marker-number">{trail.requirements.source.sequence_index ?? 1}</span>
            </div>
            <div className="timeline-content">
              <div className="timeline-item-header">
                <span className="action-tag">REQUIREMENTS_EXTRACTED</span>
                <span className="timeline-timestamp">
                  {trail.requirements.source.timestamp || 'Recorded in ledger'}
                </span>
              </div>
              <div className="req-pills">
                <span className="req-pill">Category: {trail.requirements.category}</span>
                <span className="req-pill">Budget Limit: ${trail.requirements.maximum_budget.toFixed(2)}</span>
                {trail.requirements.preferred_brand && (
                  <span className="req-pill">Brand: {trail.requirements.preferred_brand}</span>
                )}
                {trail.requirements.minimum_battery_hours && (
                  <span className="req-pill">Min Battery: {trail.requirements.minimum_battery_hours}h</span>
                )}
                {trail.requirements.required_features.map((f, i) => (
                  <span key={i} className="req-pill feature">
                    {f}
                  </span>
                ))}
              </div>
              <div className="source-ref-badge">
                Step Hash: <code>{trail.requirements.source.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}

        {/* Step 3: Catalog Search */}
        {trail.catalog_search && (
          <div className="timeline-item">
            <div className="timeline-marker">
              <span className="marker-number">{trail.catalog_search.source.sequence_index ?? 2}</span>
            </div>
            <div className="timeline-content">
              <div className="timeline-item-header">
                <span className="action-tag">CATALOG_SEARCH</span>
                <span className="timeline-timestamp">
                  {trail.catalog_search.source.timestamp || 'Recorded in ledger'}
                </span>
              </div>
              <p className="timeline-text">
                Autonomous agent evaluated <strong>{trail.catalog_search.candidates_evaluated_count}</strong> catalog products
                against extracted constraints.
              </p>
              <div className="source-ref-badge">
                Step Hash: <code>{trail.catalog_search.source.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}

        {/* Step 4: Rejected Products */}
        {trail.rejected_candidates.length > 0 && (
          <div className="timeline-item">
            <div className="timeline-marker">
              <span className="marker-number">
                {trail.rejected_candidates[0].source.sequence_index ?? 3}
              </span>
            </div>
            <div className="timeline-content">
              <div className="timeline-item-header">
                <span className="action-tag reject">PRODUCT_REJECTED ({trail.rejected_candidates.length})</span>
                <span className="timeline-timestamp">Evaluated before checkout</span>
              </div>
              <div className="rejected-list">
                {trail.rejected_candidates.map((rej, idx) => (
                  <div key={idx} className="rejected-entry">
                    <div className="rejected-name-row">
                      <span className="rejected-name">{rej.product_name}</span>
                      <span className="rejected-price">${rej.price.toFixed(2)} {rej.currency}</span>
                    </div>
                    <p className="rejection-reason">
                      <strong>Rejection Rationale:</strong> {rej.rejection_rationale.statement}
                    </p>
                    <div className="epistemic-disclaimer-sub">
                      Notice: {rej.rejection_rationale.disclaimer}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}

        {/* Step 5: Selected Product */}
        {trail.selected_product && (
          <div className="timeline-item">
            <div className="timeline-marker highlight">
              <span className="marker-number">{trail.selected_product.source.sequence_index ?? 4}</span>
            </div>
            <div className="timeline-content selected-product-box">
              <div className="timeline-item-header">
                <span className="action-tag select">PRODUCT_SELECTED</span>
                <span className="timeline-timestamp">
                  {trail.selected_product.source.timestamp || 'Recorded in ledger'}
                </span>
              </div>
              <div className="product-summary-row">
                <div>
                  <h4 className="prod-title">{trail.selected_product.product_name}</h4>
                  <span className="prod-meta">
                    Brand: {trail.selected_product.brand} | Category: {trail.selected_product.category} | ID: {trail.selected_product.product_id}
                  </span>
                </div>
                <div className="prod-price-tag">
                  ${trail.selected_product.price.toFixed(2)} {trail.selected_product.currency}
                </div>
              </div>
              <div className="selection-rationale-box">
                <strong>Selection Rationale:</strong> {trail.selected_product.selection_rationale.statement}
                <div className="epistemic-disclaimer-sub">
                  Notice: {trail.selected_product.selection_rationale.disclaimer}
                </div>
              </div>
              <div className="source-ref-badge">
                Step Hash: <code>{trail.selected_product.source.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}

        {/* Step 6: Explicit Customer Purchase Authorization */}
        {trail.purchase_authorization && (
          <div className="timeline-item">
            <div className="timeline-marker highlight-auth">
              <span className="marker-number">
                {trail.purchase_authorization.source.sequence_index ?? 5}
              </span>
            </div>
            <div className="timeline-content auth-box">
              <div className="timeline-item-header">
                <span className="action-tag auth">PURCHASE_AUTHORIZED</span>
                <span className="timeline-timestamp">
                  {trail.purchase_authorization.authorized_at}
                </span>
              </div>
              <div className="auth-details-grid">
                <div>
                  <span className="auth-lbl">Selected Product:</span>
                  <span className="auth-val mono">{trail.purchase_authorization.selected_product_id}</span>
                </div>
                <div>
                  <span className="auth-lbl">Authorized Ceiling:</span>
                  <span className="auth-val font-semibold">
                    ${trail.purchase_authorization.maximum_authorized_amount.toFixed(2)}
                  </span>
                </div>
                <div>
                  <span className="auth-lbl">User Confirmed Flag:</span>
                  <span className="auth-val">{trail.purchase_authorization.user_confirmed ? 'TRUE' : 'FALSE'}</span>
                </div>
              </div>
              {/* Identity Disclaimer */}
              <div className="identity-disclaimer-callout">
                <strong>Identity Scope Limitation:</strong> {trail.purchase_authorization.identity_disclaimer}
              </div>
              <div className="source-ref-badge">
                Step Hash: <code>{trail.purchase_authorization.source.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}

        {/* Step 7: PayPal Order Created */}
        {trans?.ledger_settlement.order_created_step && (
          <div className="timeline-item">
            <div className="timeline-marker">
              <span className="marker-number">
                {trans.ledger_settlement.order_created_step.sequence_index ?? 6}
              </span>
            </div>
            <div className="timeline-content">
              <div className="timeline-item-header">
                <span className="action-tag paypal">PAYPAL_ORDER_CREATED</span>
                <span className="timeline-timestamp">
                  {trans.ledger_settlement.order_created_step.timestamp || 'Recorded in ledger'}
                </span>
              </div>
              <p className="timeline-text">
                PayPal order <code>{trans.paypal_settlement.paypal_order_id}</code> created for{' '}
                <strong>
                  {trans.paypal_settlement.order_amount} {trans.paypal_settlement.order_currency}
                </strong>{' '}
                (Status: {trans.paypal_settlement.order_status || 'CREATED'}).
              </p>
              <div className="source-ref-badge">
                Step Hash: <code>{trans.ledger_settlement.order_created_step.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}

        {/* Step 8: PayPal Payment Captured */}
        {trans?.ledger_settlement.payment_captured_step && (
          <div className="timeline-item">
            <div className="timeline-marker highlight-capture">
              <span className="marker-number">
                {trans.ledger_settlement.payment_captured_step.sequence_index ?? 7}
              </span>
            </div>
            <div className="timeline-content capture-box">
              <div className="timeline-item-header">
                <span className="action-tag capture">PAYPAL_PAYMENT_CAPTURED</span>
                <span className="timeline-timestamp">
                  {trans.paypal_settlement.capture_timestamp ||
                    trans.ledger_settlement.payment_captured_step.timestamp ||
                    'Recorded in ledger'}
                </span>
              </div>
              <p className="timeline-text">
                PayPal capture <code>{trans.paypal_settlement.paypal_capture_id}</code> completed successfully.
                Payment status confirmed as <strong>{trans.paypal_settlement.capture_status || 'COMPLETED'}</strong>.
              </p>
              <div className="source-ref-badge">
                Step Hash: <code>{trans.ledger_settlement.payment_captured_step.step_hash?.slice(0, 16)}...</code>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
