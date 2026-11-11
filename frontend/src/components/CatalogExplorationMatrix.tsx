import React, { useState } from 'react';
import type { RejectionRecord, Product } from '../types/api';

interface CatalogExplorationMatrixProps {
  candidatesFound: number;
  rejectedCandidates: RejectionRecord[];
  selectedProductId?: string | null;
  catalogProducts?: Product[];
}

export const CatalogExplorationMatrix: React.FC<CatalogExplorationMatrixProps> = ({
  candidatesFound,
  rejectedCandidates,
  selectedProductId,
  catalogProducts = [],
}) => {
  const [filterMode, setFilterMode] = useState<'all' | 'rejected' | 'eligible'>('all');

  // Build a lookup map of rejected candidates
  const rejectionMap = new Map<string, RejectionRecord>();
  rejectedCandidates.forEach((r) => {
    rejectionMap.set(r.product_id, r);
  });

  // Calculate items to display
  // If catalogProducts is provided, we can categorize each item
  const displayItems = catalogProducts.length > 0
    ? catalogProducts.map((p) => {
        const rejection = rejectionMap.get(p.product_id);
        const isSelected = selectedProductId === p.product_id;
        const isRejected = !!rejection;

        return {
          productId: p.product_id,
          name: p.name,
          brand: p.brand,
          price: p.price,
          currency: p.currency,
          batteryHours: p.specifications?.battery_life_hours,
          anc: p.specifications?.noise_cancelling,
          rating: p.rating,
          status: isSelected ? 'selected' : isRejected ? 'rejected' : 'eligible',
          rejectionReason: rejection?.rejection_reason,
        };
      })
    : rejectedCandidates.map((r) => ({
        productId: r.product_id,
        name: r.product_name,
        brand: r.product_name.split(' ')[0] || 'Unknown',
        price: r.price,
        currency: r.currency,
        batteryHours: undefined,
        anc: undefined,
        rating: undefined,
        status: 'rejected' as const,
        rejectionReason: r.rejection_reason,
      }));

  const filteredItems = displayItems.filter((item) => {
    if (filterMode === 'rejected') return item.status === 'rejected';
    if (filterMode === 'eligible') return item.status === 'eligible' || item.status === 'selected';
    return true;
  });

  const rejectedCount = rejectedCandidates.length;
  const eligibleCount = Math.max(0, candidatesFound - rejectedCount);

  return (
    <div className="section-card catalog-exploration-card">
      <div className="section-card-header">
        <div className="card-header-titles">
          <div style={{ display: 'flex', alignItems: 'center', gap: '0.5rem', flexWrap: 'wrap' }}>
            <span className="section-badge info">Deterministic Evaluation</span>
            <span className="step-pill">Ledger Steps • CATALOG_SEARCH & CANDIDATE_REJECTED</span>
          </div>
          <h3 className="section-card-title" style={{ marginTop: '0.25rem' }}>
            Catalog Exploration & Rejection Matrix
          </h3>
          <span className="section-card-subtitle">
            Every candidate evaluated against prompt constraints with immutable cryptographic rejection audit
          </span>
        </div>

        <div className="candidate-counters">
          <span className="counter-chip total">
            <strong>{candidatesFound || displayItems.length}</strong> Evaluated
          </span>
          <span className="counter-chip rejected">
            <strong>{rejectedCount}</strong> Rejected
          </span>
          <span className="counter-chip eligible">
            <strong>{eligibleCount > 0 ? eligibleCount : (selectedProductId ? 1 : 0)}</strong> Qualified
          </span>
        </div>
      </div>

      {/* Filter Tabs */}
      <div className="matrix-filter-bar">
        <div className="matrix-filter-tabs">
          <button
            type="button"
            className={`matrix-tab-btn ${filterMode === 'all' ? 'active' : ''}`}
            onClick={() => setFilterMode('all')}
          >
            All Candidates ({displayItems.length})
          </button>
          <button
            type="button"
            className={`matrix-tab-btn ${filterMode === 'rejected' ? 'active' : ''}`}
            onClick={() => setFilterMode('rejected')}
          >
            Rejected ({rejectedCount})
          </button>
          <button
            type="button"
            className={`matrix-tab-btn ${filterMode === 'eligible' ? 'active' : ''}`}
            onClick={() => setFilterMode('eligible')}
          >
            Qualified / Selected ({eligibleCount > 0 ? eligibleCount : (selectedProductId ? 1 : 0)})
          </button>
        </div>
      </div>

      {/* Candidates Table */}
      {filteredItems.length === 0 ? (
        <div className="empty-state-box" style={{ padding: '2rem 1rem', textAlign: 'center' }}>
          <p className="text-muted">No candidates match the selected filter.</p>
        </div>
      ) : (
        <div className="matrix-table-wrapper">
          <table className="matrix-table">
            <thead>
              <tr>
                <th>Product Candidate</th>
                <th>Price</th>
                <th>Battery</th>
                <th>Evaluation Status</th>
                <th>Rejection / Selection Rationale</th>
              </tr>
            </thead>
            <tbody>
              {filteredItems.map((item) => {
                const isSelected = item.status === 'selected';
                const isRejected = item.status === 'rejected';

                return (
                  <tr
                    key={item.productId}
                    className={`matrix-row ${
                      isSelected ? 'row-selected' : isRejected ? 'row-rejected' : 'row-eligible'
                    }`}
                  >
                    <td className="product-col">
                      <div className="product-ident-group">
                        <span className="product-table-name">{item.name}</span>
                        <span className="product-table-id">ID: {item.productId}</span>
                      </div>
                    </td>
                    <td className="price-col">
                      <span className="price-tag">
                        ${item.price.toFixed(2)} {item.currency}
                      </span>
                    </td>
                    <td className="battery-col">
                      {item.batteryHours !== undefined ? `${item.batteryHours} hrs` : '—'}
                    </td>
                    <td className="status-col">
                      {isSelected ? (
                        <span className="status-pill status-selected">
                          ★ Recommended
                        </span>
                      ) : isRejected ? (
                        <span className="status-pill status-rejected">
                          ✕ Non-Compliant
                        </span>
                      ) : (
                        <span className="status-pill status-eligible">
                          ✓ Qualified
                        </span>
                      )}
                    </td>
                    <td className="rationale-col">
                      {isRejected ? (
                        <div className="rejection-reason-box">
                          <span className="reason-icon">✕</span>
                          <span className="reason-text">{item.rejectionReason}</span>
                        </div>
                      ) : isSelected ? (
                        <div className="selection-reason-box">
                          <span className="reason-icon">✓</span>
                          <span className="reason-text">
                            Ranked #1 by deterministic multi-factor scoring
                          </span>
                        </div>
                      ) : (
                        <span className="text-muted" style={{ fontSize: '0.8125rem' }}>
                          Met constraints; scored lower than top pick
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <div className="matrix-footer-note">
        <span className="info-icon">•</span>
        <span>
          Every candidate rejection is recorded on the tamper-evident hash ledger as a distinct{' '}
          <code>CANDIDATE_REJECTED</code> event with full constraint audit trail.
        </span>
      </div>
    </div>
  );
};
