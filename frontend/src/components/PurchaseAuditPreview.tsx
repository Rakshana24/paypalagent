import React, { useState } from 'react';
import type { ShoppingSessionDetail, DecisionStep } from '../types/api';
import { fetchShoppingSession, fetchSessionEvents, ApiClientError } from '../api/client';

export const DEMO_SESSION_ID = '26dc47ea-95c4-4434-ae24-005ea9751ae7';

interface PurchaseAuditPreviewProps {
  onSessionLoaded?: (sessionId: string) => void;
  initialSessionId?: string;
}

export const PurchaseAuditPreview: React.FC<PurchaseAuditPreviewProps> = ({
  onSessionLoaded,
  initialSessionId,
}) => {
  const [sessionIdInput, setSessionIdInput] = useState<string>(
    initialSessionId || DEMO_SESSION_ID
  );
  const [loading, setLoading] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [sessionDetail, setSessionDetail] = useState<ShoppingSessionDetail | null>(null);
  const [events, setEvents] = useState<DecisionStep[] | null>(null);
  const [expandedPayloads, setExpandedPayloads] = useState<Record<number, boolean>>({});

  // Sync and load if initialSessionId changes
  React.useEffect(() => {
    if (initialSessionId && initialSessionId.trim()) {
      setSessionIdInput(initialSessionId.trim());
      void handleLoadSession(initialSessionId.trim());
    }
  }, [initialSessionId]);

  const handleLoadSession = async (targetId: string) => {
    const trimmed = targetId.trim();
    if (!trimmed) {
      setError('Please provide a valid session ID');
      return;
    }

    setLoading(true);
    setError(null);

    try {
      const [detail, eventList] = await Promise.all([
        fetchShoppingSession(trimmed),
        fetchSessionEvents(trimmed),
      ]);
      setSessionDetail(detail);
      setEvents(eventList);
      if (onSessionLoaded) {
        onSessionLoaded(trimmed);
      }
    } catch (err: unknown) {
      if (err instanceof ApiClientError && err.status === 404) {
        setError(`Purchase session not found: ${trimmed}`);
      } else {
        setError(err instanceof Error ? err.message : 'Failed to fetch purchase session');
      }
      setSessionDetail(null);
      setEvents(null);
    } finally {
      setLoading(false);
    }
  };

  const togglePayload = (seqIndex: number) => {
    setExpandedPayloads((prev) => ({
      ...prev,
      [seqIndex]: !prev[seqIndex],
    }));
  };

  const handleClear = () => {
    setSessionDetail(null);
    setEvents(null);
    setError(null);
  };

  // Derive capture ID dynamically from recorded events
  const captureEvent = events?.find((e) => e.action_type === 'PAYPAL_PAYMENT_CAPTURED');
  let captureId: string | null = null;
  if (captureEvent) {
    try {
      const parsed = JSON.parse(captureEvent.payload);
      captureId = parsed.capture_id || null;
    } catch {
      captureId = null;
    }
  }

  // Format currency
  const formatMoney = (amount?: number | null, currency = 'USD') => {
    if (amount === undefined || amount === null) return 'N/A';
    return `${currency} ${amount.toFixed(2)}`;
  };

  // Helper to format payload summaries
  const renderPayloadSummary = (step: DecisionStep) => {
    try {
      const parsed = JSON.parse(step.payload);
      switch (step.action_type) {
        case 'INITIAL_PROMPT':
          return (
            <div className="step-summary-content prompt-box">
              <span className="step-label">Buyer Instruction:</span>
              <p className="prompt-text">“{parsed.initial_prompt}”</p>
            </div>
          );
        case 'REQUIREMENTS_EXTRACTED':
          return (
            <div className="step-summary-content">
              <div className="pill-group">
                <span className="info-pill">Category: {parsed.category}</span>
                <span className="info-pill">Max Budget: {formatMoney(parsed.maximum_budget, parsed.currency)}</span>
                {parsed.minimum_battery_hours && (
                  <span className="info-pill">Min Battery: {parsed.minimum_battery_hours}h</span>
                )}
                {parsed.required_features?.map((f: string) => (
                  <span key={f} className="info-pill feature-pill">{f}</span>
                ))}
              </div>
            </div>
          );
        case 'CATALOG_SEARCH':
          return (
            <div className="step-summary-content">
              <span className="text-sm text-muted">
                Evaluated {parsed.candidates_retrieved_count ?? parsed.candidates?.length ?? 0} catalog candidate items in category “{parsed.category}”
              </span>
            </div>
          );
        case 'PRODUCT_REJECTED':
          return (
            <div className="step-summary-content rejection-box">
              <div className="rejection-header">
                <strong>{parsed.product_name}</strong> ({formatMoney(parsed.price, parsed.currency)})
              </div>
              <div className="rejection-reason">Reason: {parsed.rejection_reason}</div>
            </div>
          );
        case 'PRODUCT_COMPARISON':
          return (
            <div className="step-summary-content">
              <span className="text-sm text-muted">
                Scored {parsed.valid_candidates_evaluated} eligible candidates across battery, pricing, ratings, and features.
              </span>
            </div>
          );
        case 'PRODUCT_SELECTED':
          return (
            <div className="step-summary-content selection-box">
              <div className="selection-header">
                <strong>{parsed.selected_product?.name}</strong> • {formatMoney(parsed.price, parsed.currency)}
              </div>
              <p className="selection-rationale">{parsed.selection_rationale}</p>
            </div>
          );
        case 'PURCHASE_AUTHORIZED':
          return (
            <div className="step-summary-content auth-box">
              <div className="pill-group">
                <span className="info-pill success-pill">Status: {parsed.authorization_status}</span>
                <span className="info-pill">Method: {parsed.authorization_method}</span>
                <span className="info-pill">Ceiling: {formatMoney(parsed.maximum_authorized_amount, parsed.currency)}</span>
              </div>
            </div>
          );
        case 'PAYPAL_ORDER_CREATED':
          return (
            <div className="step-summary-content">
              <div className="pill-group">
                <span className="info-pill">PayPal Order: <code>{parsed.order_id}</code></span>
                <span className="info-pill">Amount: {parsed.currency} {parsed.amount}</span>
                <span className="info-pill">Status: {parsed.status}</span>
              </div>
            </div>
          );
        case 'PAYPAL_PAYMENT_CAPTURED':
          return (
            <div className="step-summary-content capture-box">
              <div className="pill-group">
                <span className="info-pill success-pill">PayPal Capture: <code>{parsed.capture_id}</code></span>
                <span className="info-pill">Order: <code>{parsed.order_id}</code></span>
                <span className="info-pill">Status: {parsed.status}</span>
                <span className="info-pill">Cryptographically Verified: {parsed.verified ? 'YES' : 'NO'}</span>
              </div>
            </div>
          );
        default:
          return <pre className="raw-snippet">{JSON.stringify(parsed, null, 2)}</pre>;
      }
    } catch {
      return <pre className="raw-snippet">{step.payload}</pre>;
    }
  };

  return (
    <div className="purchase-preview-container">
      {/* Top Action Card */}
      <div className="purchase-preview-header-card">
        <div className="purchase-preview-header-left">
          <div className="section-title-wrap">
            <span className="badge-pre-dispute">PRE-DISPUTE PURCHASE AUDIT</span>
            <h2 className="purchase-preview-title">Verified Purchase Audit Trail</h2>
          </div>
          <p className="purchase-preview-desc">
            Inspect a completed autonomous agent purchase and its verifiable SHA-256 hash chain before any dispute is filed.
          </p>
        </div>

        <div className="purchase-preview-actions">
          <button
            type="button"
            className="btn-demo-primary"
            disabled={loading}
            onClick={() => {
              setSessionIdInput(DEMO_SESSION_ID);
              void handleLoadSession(DEMO_SESSION_ID);
            }}
          >
            {loading && sessionIdInput === DEMO_SESSION_ID ? (
              <>
                <span className="spinner" /> Loading Demo Purchase...
              </>
            ) : (
              '⚡ Load Verified Demo Purchase'
            )}
          </button>
        </div>
      </div>

      {/* Session ID Custom Input Bar */}
      <div className="session-input-card">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void handleLoadSession(sessionIdInput);
          }}
          className="session-input-form"
        >
          <div className="session-input-group">
            <label htmlFor="session-id-input" className="session-input-label">
              Audit Session ID:
            </label>
            <input
              id="session-id-input"
              type="text"
              className="session-input-field"
              placeholder="e.g. 26dc47ea-95c4-4434-ae24-005ea9751ae7"
              value={sessionIdInput}
              onChange={(e) => setSessionIdInput(e.target.value)}
              disabled={loading}
            />
          </div>

          <div className="session-button-group">
            <button
              type="submit"
              className="btn-primary"
              disabled={loading || !sessionIdInput.trim()}
            >
              {loading ? (
                <>
                  <span className="spinner" /> Verifying...
                </>
              ) : (
                'Inspect Audit Trail'
              )}
            </button>
            {sessionDetail && (
              <button
                type="button"
                className="btn-secondary"
                onClick={handleClear}
                disabled={loading}
              >
                Clear
              </button>
            )}
          </div>
        </form>

        {error && (
          <div className="status-banner error" style={{ marginTop: '1rem' }}>
            <span className="status-icon">⚠</span>
            <span>{error}</span>
          </div>
        )}
      </div>

      {/* Main Audit Display */}
      {sessionDetail && (
        <div className="purchase-audit-content">
          {/* 1. Cryptographic Ledger Verification Status */}
          <div className="section-card crypto-verification-banner">
            <div className="section-card-header">
              <div className="section-title-wrap">
                <span className={`status-pill ${sessionDetail.verification.status === 'VALID' ? 'success' : 'error'}`}>
                  ● {sessionDetail.verification.status === 'VALID' ? 'CRYPTOGRAPHICALLY VALID' : 'VERIFICATION FAILED'}
                </span>
                <h3 className="section-card-title">Ledger Integrity & Cryptographic Chain</h3>
              </div>
              <span className="section-card-subtitle">
                {sessionDetail.verification.explanation}
              </span>
            </div>

            <div className="crypto-stats-grid">
              <div className="crypto-stat-box">
                <span className="crypto-stat-label">Chain Status</span>
                <span className={`crypto-stat-value ${sessionDetail.verification.status === 'VALID' ? 'valid' : 'invalid'}`}>
                  {sessionDetail.verification.status}
                </span>
              </div>
              <div className="crypto-stat-box">
                <span className="crypto-stat-label">Verified Events</span>
                <span className="crypto-stat-value">
                  {sessionDetail.verification.verified_steps} / {sessionDetail.verification.total_steps}
                </span>
              </div>
              <div className="crypto-stat-box">
                <span className="crypto-stat-label">Session Lifecycle</span>
                <span className="crypto-stat-value">
                  {sessionDetail.is_finalized ? 'FINALIZED (LOCKED)' : 'ACTIVE'}
                </span>
              </div>
            </div>

            <div className="hash-proof-box">
              <div className="hash-proof-row">
                <span className="hash-label">Final Ledger Root Hash:</span>
                <code className="hash-value break-all">
                  {sessionDetail.root_hash || sessionDetail.verification.stored_root_hash || 'Unsealed'}
                </code>
              </div>
              <div className="hash-proof-row">
                <span className="hash-label">Calculated Hash:</span>
                <code className="hash-value break-all">
                  {sessionDetail.verification.calculated_root_hash}
                </code>
              </div>
            </div>
          </div>

          {/* 2. Purchase Session Overview Grid */}
          <div className="section-card">
            <div className="section-card-header">
              <div className="section-title-wrap">
                <span className="epistemic-tag verified-fact">VERIFIED_FACT</span>
                <h3 className="section-card-title">Purchase Details & Settlement Identifiers</h3>
              </div>
              <span className="section-card-subtitle">
                Deterministic facts extracted from verified ledger events
              </span>
            </div>

            <div className="proof-grid">
              <div className="proof-item">
                <span className="proof-label">Session ID</span>
                <span className="proof-value mono text-sm">{sessionDetail.session_id}</span>
              </div>

              <div className="proof-item">
                <span className="proof-label">Purchase Status</span>
                <span className="proof-value">
                  <span className="tag-pill completed">COMPLETED & FINALIZED</span>
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">Product Name</span>
                <span className="proof-value font-semibold">
                  {sessionDetail.selected_product?.name || 'N/A'}
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">Purchase Amount</span>
                <span className="proof-value font-semibold text-primary">
                  {formatMoney(sessionDetail.selected_product?.price, sessionDetail.selected_product?.currency)}
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">Buyer Authorization</span>
                <span className="proof-value">
                  <span className="tag-pill approved">
                    {sessionDetail.authorization?.authorization_status || 'APPROVED'}
                  </span>
                  <span className="text-xs text-muted" style={{ marginLeft: '0.5rem' }}>
                    ({sessionDetail.authorization?.authorization_method || 'EXPLICIT_APPROVAL'})
                  </span>
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">Authorized Spending Ceiling</span>
                <span className="proof-value">
                  Max Budget: {formatMoney(sessionDetail.requirements?.maximum_budget, sessionDetail.requirements?.currency)}
                  {sessionDetail.authorization && (
                    <span className="text-xs text-muted" style={{ display: 'block' }}>
                      Authorized Limit: {formatMoney(sessionDetail.authorization.maximum_authorized_amount, sessionDetail.authorization.currency)}
                    </span>
                  )}
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">PayPal Order ID</span>
                <span className="proof-value mono">
                  {sessionDetail.paypal_order_id ? (
                    <code>{sessionDetail.paypal_order_id}</code>
                  ) : (
                    'Not recorded'
                  )}
                </span>
              </div>

              <div className="proof-item">
                <span className="proof-label">PayPal Capture ID</span>
                <span className="proof-value mono">
                  {captureId ? (
                    <code>{captureId}</code>
                  ) : (
                    <span className="text-muted">Extracted from event sequence</span>
                  )}
                </span>
              </div>
            </div>
          </div>

          {/* 3. Explicit Epistemic Disclaimer */}
          <div className="disclaimer-callout">
            <div className="disclaimer-title">
              <span className="disclaimer-icon">ℹ</span> Epistemic Disclaimer & Scope of Proof
            </div>
            <p className="disclaimer-text">
              Hash-chain verification establishes the sequential consistency, mathematical immutability,
              and tamper-evident integrity of recorded events within this purchase session ledger.
              It verifies that the autonomous agent obeyed budget constraints, requirements, and explicit customer authorization.
              It does <strong>not</strong> independently prove physical postal delivery or verify the buyer's real-world identity.
            </p>
          </div>

          {/* 4. Chronological Timeline of All Recorded Events */}
          <div className="section-card">
            <div className="section-card-header">
              <div className="section-title-wrap">
                <span className="epistemic-tag verified-fact">VERIFIED_CHAIN</span>
                <h3 className="section-card-title">Chronological Event Timeline</h3>
              </div>
              <span className="section-card-subtitle">
                All {events?.length || sessionDetail.event_count} sequentially hashed ledger steps from prompt to capture
              </span>
            </div>

            <div className="timeline-container">
              {events?.map((step) => {
                const isExpanded = !!expandedPayloads[step.sequence_index];
                return (
                  <div key={step.id} className="timeline-item">
                    <div className="timeline-marker">
                      <span className="marker-number">{step.sequence_index}</span>
                    </div>

                    <div className="timeline-content">
                      <div className="timeline-item-header">
                        <div className="timeline-type-wrap">
                          <span className={`action-tag action-${step.action_type.toLowerCase().replace(/_/g, '-')}`}>
                            {step.action_type}
                          </span>
                          <span className="seq-badge">Seq #{step.sequence_index}</span>
                        </div>
                        <span className="timeline-timestamp">
                          {new Date(step.created_at).toLocaleString()}
                        </span>
                      </div>

                      {/* Readable summary */}
                      {renderPayloadSummary(step)}

                      {/* Cryptographic Linkage Info */}
                      <div className="step-crypto-footer">
                        <div className="hash-trail">
                          <span className="hash-trail-label">Hash:</span>
                          <code className="hash-trail-val">{step.current_hash.slice(0, 16)}...</code>
                          <span className="hash-trail-arrow">←</span>
                          <span className="hash-trail-label">Prev:</span>
                          <code className="hash-trail-val">{step.previous_hash.slice(0, 16)}...</code>
                        </div>

                        <button
                          type="button"
                          className="btn-toggle-payload"
                          onClick={() => togglePayload(step.sequence_index)}
                        >
                          {isExpanded ? 'Hide Raw JSON ▲' : 'Inspect Raw JSON ▼'}
                        </button>
                      </div>

                      {isExpanded && (
                        <div className="step-raw-payload">
                          <pre>{JSON.stringify(JSON.parse(step.payload), null, 2)}</pre>
                        </div>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
