import React from 'react';
import type { ShoppingRequirements } from '../types/api';

interface RequirementAnalysisCardProps {
  requirements: ShoppingRequirements;
  prompt?: string;
  sequenceIndex?: number;
}

export const RequirementAnalysisCard: React.FC<RequirementAnalysisCardProps> = ({
  requirements,
  prompt,
  sequenceIndex = 1,
}) => {
  return (
    <div className="section-card requirement-analysis-card">
      <div className="section-card-header">
        <div className="card-header-titles">
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
            <span className="section-badge success">Extracted Requirements</span>
            <span className="step-pill">Ledger Step #{sequenceIndex} • REQUIREMENTS_EXTRACTED</span>
          </div>
          <h3 className="section-card-title" style={{ marginTop: '0.25rem' }}>
            Structured Customer Requirements
          </h3>
          <span className="section-card-subtitle">
            Deterministically extracted constraints validated against policy rules before catalog search
          </span>
        </div>
      </div>

      {prompt && (
        <div className="original-prompt-quote">
          <span className="quote-label">Input Prompt:</span>
          <p className="quote-text">“{prompt}”</p>
        </div>
      )}

      <div className="requirements-grid">
        {/* Category */}
        <div className="req-metric-box">
          <span className="req-metric-label">Product Category</span>
          <span className="req-metric-value category-value">
            {requirements.category.toUpperCase()}
          </span>
          <span className="req-metric-sub">Catalog partition</span>
        </div>

        {/* Budget Limit */}
        <div className="req-metric-box highlight-metric">
          <span className="req-metric-label">Maximum Budget Limit</span>
          <div className="req-metric-value budget-value">
            ${requirements.maximum_budget.toFixed(2)}
            <span className="req-currency">{requirements.currency}</span>
          </div>
          <span className="req-metric-sub">Strict ceiling cap</span>
        </div>

        {/* Battery Life Constraint */}
        <div className="req-metric-box">
          <span className="req-metric-label">Minimum Battery Life</span>
          <span className="req-metric-value">
            {requirements.minimum_battery_hours !== undefined && requirements.minimum_battery_hours !== null
              ? `${requirements.minimum_battery_hours} Hours`
              : 'Not specified'}
          </span>
          <span className="req-metric-sub">Continuous playback requirement</span>
        </div>

        {/* Preferred Brand */}
        <div className="req-metric-box">
          <span className="req-metric-label">Preferred Brand</span>
          <span className="req-metric-value">
            {requirements.preferred_brand ? requirements.preferred_brand : 'Any brand'}
          </span>
          <span className="req-metric-sub">
            {requirements.preferred_brand ? 'Prioritized in scoring' : 'No brand lock'}
          </span>
        </div>
      </div>

      {/* Required Features */}
      <div className="req-features-section">
        <span className="req-features-label">Mandatory Feature Filters:</span>
        <div className="req-features-tags">
          {requirements.required_features && requirements.required_features.length > 0 ? (
            requirements.required_features.map((feature, idx) => (
              <span key={idx} className="feature-pill active">
                <span className="feature-check">✓</span>
                {feature}
              </span>
            ))
          ) : (
            <span className="text-muted" style={{ fontSize: '0.8125rem' }}>
              Standard category defaults
            </span>
          )}
        </div>
      </div>
    </div>
  );
};
