import React, { useState } from 'react';
import type { Product, ShoppingRequirements, PurchaseAuthorization } from '../types/api';

interface PurchaseAuthorizationCardProps {
  product: Product;
  requirements: ShoppingRequirements;
  authorization: PurchaseAuthorization | null;
  isAuthorizing: boolean;
  error: string | null;
  onAuthorize: (approved: boolean, ceiling?: number) => Promise<unknown>;
  sequenceIndex?: number;
  currentHash?: string;
}

export const PurchaseAuthorizationCard: React.FC<PurchaseAuthorizationCardProps> = ({
  product,
  requirements,
  authorization,
  isAuthorizing,
  error,
  onAuthorize,
  sequenceIndex,
  currentHash,
}) => {
  // Ceiling defaults to the exact product price
  const [ceilingInput, setCeilingInput] = useState<string>(product.price.toFixed(2));
  const [consentChecked, setConsentChecked] = useState<boolean>(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  const ceilingNum = parseFloat(ceilingInput);
  const isValidNumber = !isNaN(ceilingNum);
  const isCeilingSufficient = isValidNumber && ceilingNum >= product.price;
  const headroom = isValidNumber ? ceilingNum - product.price : 0;

  const isApproved =
    authorization &&
    (authorization.authorization_status === 'APPROVED' ||
      String(authorization.authorization_status).toUpperCase() === 'APPROVED');
  const isDeclined =
    authorization &&
    (authorization.authorization_status === 'REJECTED' ||
      String(authorization.authorization_status).toUpperCase() === 'REJECTED');

  const handleCeilingChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const val = e.target.value;
    setCeilingInput(val);
    const num = parseFloat(val);
    if (isNaN(num)) {
      setValidationError('Please enter a valid numeric authorization limit.');
    } else if (num < product.price) {
      setValidationError(
        `Authorization ceiling ($${num.toFixed(2)}) cannot be less than the product price ($${product.price.toFixed(2)}).`
      );
    } else {
      setValidationError(null);
    }
  };

  const handleApprove = async () => {
    if (!isCeilingSufficient || !consentChecked || isAuthorizing) return;
    try {
      await onAuthorize(true, ceilingNum);
    } catch {
      // Error handled by hook
    }
  };

  const handleDecline = async () => {
    if (isAuthorizing) return;
    try {
      await onAuthorize(false, ceilingNum);
    } catch {
      // Error handled by hook
    }
  };

  return (
    <div className={`section-card purchase-authorization-card ${isApproved ? 'card-authorized' : isDeclined ? 'card-declined' : ''}`}>
      <div className="section-card-header">
        <div className="card-header-titles">
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
            <span className={`section-badge ${isApproved ? 'success' : isDeclined ? 'error' : 'primary'}`}>
              {isApproved ? '✓ Authorized' : isDeclined ? '✕ Declined' : 'Step 4 • Deliberate Consent'}
            </span>
            <span className="step-pill">
              {isApproved
                ? `Ledger Step #${sequenceIndex || 'Next'} • PURCHASE_AUTHORIZED`
                : 'Customer Purchase Authorization Gate'}
            </span>
          </div>
          <h3 className="section-card-title" style={{ marginTop: '0.25rem' }}>
            Customer Purchase Authorization
          </h3>
          <span className="section-card-subtitle">
            Autonomous agent recommendation must receive deliberate customer authorization before any payment order is generated
          </span>
        </div>

        {currentHash && isApproved && (
          <div className="selection-hash-badge" title={currentHash}>
            <span className="hash-label">State Hash:</span>
            <code className="hash-code">{currentHash.slice(0, 14)}...</code>
          </div>
        )}
      </div>

      {/* When Already Authorized */}
      {isApproved && (
        <div className="authorization-confirmed-box">
          <div className="confirmed-header">
            <span className="confirmed-icon">✓</span>
            <div>
              <h4 className="confirmed-title">Purchase Explicitly Authorized</h4>
              <p className="confirmed-sub">
                Committed to the tamper-evident ledger. PayPal order creation is now unlocked.
              </p>
            </div>
          </div>

          <div className="auth-record-grid">
            <div className="auth-record-item">
              <span className="record-label">Authorized Item</span>
              <strong className="record-value">{product.name}</strong>
              <span className="record-meta">SKU: {product.product_id}</span>
            </div>
            <div className="auth-record-item">
              <span className="record-label">Approved Price</span>
              <strong className="record-value price-green">
                ${product.price.toFixed(2)} {product.currency}
              </strong>
              <span className="record-meta">Exact merchant charge</span>
            </div>
            <div className="auth-record-item">
              <span className="record-label">Authorized Spending Ceiling</span>
              <strong className="record-value">
                ${authorization.maximum_authorized_amount.toFixed(2)} {authorization.currency}
              </strong>
              <span className="record-meta">Hard limit cap</span>
            </div>
            <div className="auth-record-item">
              <span className="record-label">Authorization Timestamp</span>
              <strong className="record-value timestamp-val">
                {new Date(authorization.authorization_timestamp).toLocaleTimeString([], {
                  hour: '2-digit',
                  minute: '2-digit',
                  second: '2-digit',
                })}
              </strong>
              <span className="record-meta">{authorization.authorization_method}</span>
            </div>
          </div>
        </div>
      )}

      {/* When Declined */}
      {isDeclined && (
        <div className="authorization-declined-box">
          <div className="declined-header">
            <span className="declined-icon">✕</span>
            <div>
              <h4 className="declined-title">Purchase Authorization Declined</h4>
              <p className="declined-sub">
                You declined authorization for this item. In accordance with AgentAudit safety rules, no PayPal order will be created and no funds will be charged.
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Interactive Authorization Form (when pending) */}
      {!isApproved && !isDeclined && (
        <div className="authorization-form-content">
          {/* Comparison Matrix: Budget vs Price vs Ceiling */}
          <div className="authorization-comparison-row">
            {/* 1. Shopping Budget */}
            <div className="comparison-tile budget-tile">
              <span className="tile-label">1. Customer Shopping Budget</span>
              <div className="tile-amount">
                ${requirements.maximum_budget.toFixed(2)}
                <span className="tile-currency">{requirements.currency}</span>
              </div>
              <p className="tile-desc">Your original natural-language constraint.</p>
            </div>

            {/* 2. Actual Product Price */}
            <div className="comparison-tile price-tile">
              <span className="tile-label">2. Selected Product Price</span>
              <div className="tile-amount highlight-price">
                ${product.price.toFixed(2)}
                <span className="tile-currency">{product.currency}</span>
              </div>
              <p className="tile-desc">Exact price charged by merchant catalog.</p>
            </div>

            {/* 3. Explicit Spending Ceiling */}
            <div className="comparison-tile ceiling-tile">
              <span className="tile-label">3. Authorized Spending Ceiling</span>
              <div className="ceiling-input-group">
                <span className="currency-prefix">$</span>
                <input
                  type="number"
                  step="0.01"
                  min={product.price}
                  value={ceilingInput}
                  onChange={handleCeilingChange}
                  disabled={isAuthorizing}
                  className="ceiling-number-input"
                  aria-label="Authorized maximum spending ceiling"
                />
                <span className="currency-suffix">{product.currency}</span>
              </div>
              <div className="headroom-indicator">
                {headroom > 0 ? (
                  <span className="headroom-text positive">
                    +${headroom.toFixed(2)} buffer above item price
                  </span>
                ) : (
                  <span className="headroom-text exact">
                    Exact price match (No buffer)
                  </span>
                )}
              </div>
            </div>
          </div>

          {/* Explanation Callout */}
          <div className="budget-vs-ceiling-note">
            <span className="note-icon">ℹ</span>
            <div className="note-text">
              <strong>Understanding Budget vs. Ceiling:</strong> Your <em>Shopping Budget</em> was your discovery target ($
              {requirements.maximum_budget.toFixed(2)} {requirements.currency}). The <em>Authorization Ceiling</em> ($
              {isCeilingSufficient ? ceilingNum.toFixed(2) : '--'} {product.currency}) is the binding financial ceiling you permit the agent to charge for this order.
            </div>
          </div>

          {/* Validation Error */}
          {validationError && (
            <div className="alert-card alert-error" role="alert" style={{ margin: '0.75rem 0' }}>
              <strong>Invalid Spending Limit: </strong> {validationError}
            </div>
          )}

          {/* API Error */}
          {error && (
            <div className="alert-card alert-error" role="alert" style={{ margin: '0.75rem 0' }}>
              <strong>Authorization Error: </strong> {error}
            </div>
          )}

          {/* Explicit Consent Checkbox */}
          <div className="consent-checkbox-wrapper">
            <label className="consent-label" htmlFor="auth-consent-checkbox">
              <input
                type="checkbox"
                id="auth-consent-checkbox"
                checked={consentChecked}
                onChange={(e) => setConsentChecked(e.target.checked)}
                disabled={isAuthorizing || !isCeilingSufficient}
                className="consent-checkbox"
              />
              <span className="consent-text">
                I explicitly authorize <strong>AgentAudit</strong> to purchase <strong>{product.name}</strong> for{' '}
                <strong>${product.price.toFixed(2)} {product.currency}</strong> (spending ceiling of up to{' '}
                <strong>${isCeilingSufficient ? ceilingNum.toFixed(2) : '--'} {product.currency}</strong>). I acknowledge this authorization will be committed to the cryptographic ledger.
              </span>
            </label>
          </div>

          {/* Action Buttons */}
          <div className="authorization-actions-row">
            <button
              type="button"
              className="btn-secondary decline-btn"
              onClick={handleDecline}
              disabled={isAuthorizing}
            >
              Decline Purchase
            </button>

            <button
              type="button"
              className="btn-primary approve-btn"
              onClick={handleApprove}
              disabled={!consentChecked || !isCeilingSufficient || isAuthorizing}
            >
              {isAuthorizing ? (
                <>
                  <span className="spinner-inline" aria-hidden="true" />
                  <span>Recording Authorization On-Chain...</span>
                </>
              ) : (
                <>
                  <span>Approve Purchase (${isCeilingSufficient ? ceilingNum.toFixed(2) : product.price.toFixed(2)})</span>
                  <span className="btn-arrow" aria-hidden="true">→</span>
                </>
              )}
            </button>
          </div>
        </div>
      )}
    </div>
  );
};
