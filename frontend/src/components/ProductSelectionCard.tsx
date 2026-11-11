import React from 'react';
import type { Product, ShoppingRequirements } from '../types/api';

interface ProductSelectionCardProps {
  product: Product;
  rationale?: string | null;
  requirements?: ShoppingRequirements | null;
  latestHash?: string;
  sequenceIndex?: number;
  onProceedToAuthorization?: () => void;
}

export const ProductSelectionCard: React.FC<ProductSelectionCardProps> = ({
  product,
  rationale,
  requirements,
  latestHash,
  sequenceIndex = 4,
  onProceedToAuthorization,
}) => {
  // Budget comparison calculations
  const maxBudget = requirements?.maximum_budget;
  const price = product.price;
  const isUnderBudget = maxBudget !== undefined && price <= maxBudget;
  const budgetDiff = maxBudget !== undefined ? maxBudget - price : null;
  const savingsPercent =
    maxBudget !== undefined && maxBudget > 0
      ? ((budgetDiff || 0) / maxBudget) * 100
      : null;

  return (
    <div className="section-card product-selection-card">
      <div className="section-card-header">
        <div className="card-header-titles">
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
            <span className="section-badge success">★ Top Recommendation</span>
            <span className="step-pill">Ledger Step #{sequenceIndex} • PRODUCT_SELECTED</span>
          </div>
          <h3 className="section-card-title" style={{ marginTop: '0.25rem' }}>
            Agent Selected Product
          </h3>
          <span className="section-card-subtitle">
            Ranked highest based on price-to-performance ratio, battery endurance, and constraint satisfaction
          </span>
        </div>

        {latestHash && (
          <div className="selection-hash-badge" title={latestHash}>
            <span className="hash-label">Ledger State Hash:</span>
            <code className="hash-code">{latestHash.slice(0, 16)}...</code>
          </div>
        )}
      </div>

      <div className="product-showcase-layout">
        {/* Left Column: Product Information & Specifications */}
        <div className="product-details-col">
          <div className="product-headline">
            <span className="product-brand-tag">{product.brand}</span>
            <h4 className="product-full-name">{product.name}</h4>
            <div className="product-rating-row">
              <span className="rating-stars" aria-label={`${product.rating.toFixed(1)} out of 5 stars`}>
                {'★'.repeat(Math.floor(product.rating))}
                {product.rating % 1 >= 0.3 ? '½' : ''}
              </span>
              <span className="rating-num">{product.rating.toFixed(1)} / 5.0</span>
              <span className="stock-badge in-stock">✓ In Stock ({product.available_stock} available)</span>
              <span className="product-id-tag">SKU: {product.product_id}</span>
            </div>
          </div>

          <p className="product-description-text">{product.description}</p>

          {/* Specifications Grid */}
          <div className="specs-container">
            <span className="specs-title">Technical Specifications:</span>
            <div className="specs-grid">
              <div className="spec-item">
                <span className="spec-label">Battery Playtime</span>
                <span className="spec-value">
                  {product.specifications.battery_life_hours} Hours
                </span>
              </div>
              <div className="spec-item">
                <span className="spec-label">Noise Cancellation</span>
                <span className="spec-value">
                  {product.specifications.noise_cancelling
                    ? product.specifications.anc_type || 'Active ANC'
                    : 'None'}
                </span>
              </div>
              <div className="spec-item">
                <span className="spec-label">Connectivity</span>
                <span className="spec-value">
                  {product.specifications.wireless ? 'Wireless' : 'Wired'} • BT {product.specifications.bluetooth_version}
                </span>
              </div>
              <div className="spec-item">
                <span className="spec-label">Weight</span>
                <span className="spec-value">
                  {product.specifications.weight_grams}g
                </span>
              </div>
              <div className="spec-item">
                <span className="spec-label">Microphone</span>
                <span className="spec-value">
                  {product.specifications.microphone ? 'Built-in' : 'No'}
                </span>
              </div>
              <div className="spec-item">
                <span className="spec-label">Water Resistance</span>
                <span className="spec-value">
                  {product.specifications.water_resistant ? 'Yes' : 'No (Indoor)'}
                </span>
              </div>
            </div>
          </div>

          {/* Feature Badges */}
          {product.features && product.features.length > 0 && (
            <div className="product-feature-badges">
              {product.features.map((feat, idx) => (
                <span key={idx} className="feature-chip">
                  {feat}
                </span>
              ))}
            </div>
          )}
        </div>

        {/* Right Column: Pricing, Budget Impact & Action */}
        <div className="product-pricing-col">
          <div className="pricing-card">
            <span className="pricing-label">Item Price</span>
            <div className="pricing-amount">
              ${product.price.toFixed(2)}
              <span className="pricing-currency">{product.currency}</span>
            </div>

            {/* Budget Delta Box */}
            {maxBudget !== undefined && (
              <div className={`budget-comparison-box ${isUnderBudget ? 'under-budget' : 'over-budget'}`}>
                <div className="budget-delta-header">
                  <span className="budget-icon">{isUnderBudget ? '✓' : '!'}</span>
                  <span className="budget-status">
                    {isUnderBudget
                      ? `$${(budgetDiff || 0).toFixed(2)} Under Budget`
                      : `$${Math.abs(budgetDiff || 0).toFixed(2)} Exceeds Budget`}
                  </span>
                </div>
                <div className="budget-breakdown">
                  <div className="budget-line">
                    <span>Authorized Ceiling:</span>
                    <strong>${maxBudget.toFixed(2)} {requirements?.currency}</strong>
                  </div>
                  <div className="budget-line">
                    <span>Selected Price:</span>
                    <strong>${product.price.toFixed(2)} {product.currency}</strong>
                  </div>
                  {savingsPercent !== null && isUnderBudget && (
                    <div className="budget-savings-pill">
                      {savingsPercent.toFixed(1)}% savings below maximum limit
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* Selection Rationale */}
            {rationale && (
              <div className="agent-rationale-box">
                <span className="rationale-header">Agent Selection Rationale:</span>
                <p className="rationale-body">{rationale}</p>
              </div>
            )}

            {/* Mandatory Epistemic Disclaimer */}
            <div className="authorization-disclaimer-card">
              <div className="disclaimer-title-row">
                <span className="disclaimer-icon">⚠</span>
                <strong>Customer Authorization Required</strong>
              </div>
              <p className="disclaimer-text">
                Product recommendation only. <strong>No payment has been authorized or charged yet.</strong> In accordance with AgentAudit safety invariants, the user must explicitly approve the purchase amount before PayPal Sandbox order creation.
              </p>
            </div>

            {/* Stage Proceed CTA */}
            {onProceedToAuthorization && (
              <button
                type="button"
                className="btn-primary proceed-btn"
                onClick={onProceedToAuthorization}
              >
                <span>Review Purchase & Prepare Authorization</span>
                <span className="btn-arrow" aria-hidden="true">→</span>
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
