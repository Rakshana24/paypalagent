import React from 'react';
import type { CryptographicProofSummary } from '../types/api';

interface CryptographicVerificationCardProps {
  cryptoProof: CryptographicProofSummary;
}

export const CryptographicVerificationCard: React.FC<CryptographicVerificationCardProps> = ({
  cryptoProof,
}) => {
  return (
    <div className="section-card">
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag verified-fact">VERIFIED_FACT</span>
          <h3 className="section-card-title">2. Cryptographic Proof & Ledger Verification</h3>
        </div>
        <div className="header-badges">
          <span
            className={`status-badge-outline ${
              cryptoProof.dual_chain_integrity_verified ? 'matched' : 'error'
            }`}
          >
            Dual-Chain Integrity:{' '}
            {cryptoProof.dual_chain_integrity_verified ? 'VERIFIED' : 'FAILED'}
          </span>
        </div>
      </div>

      {/* Verification Matrix Grid */}
      <div className="crypto-grid">
        {/* Purchase Session Ledger Card */}
        <div className="crypto-subcard">
          <div className="subcard-header">
            <span className="subcard-title">Purchase Session Ledger</span>
            <span
              className={`pill-status ${
                cryptoProof.purchase_chain_valid ? 'valid' : 'invalid'
              }`}
            >
              {cryptoProof.purchase_chain_valid ? 'VALID' : 'FAILED / TAMPERED'}
            </span>
          </div>
          <div className="subcard-content">
            <div className="crypto-field">
              <span className="field-lbl">Session ID:</span>
              <span className="field-val mono">
                {cryptoProof.purchase_session_id || 'N/A (Uncorrelated)'}
              </span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Verified Decision Steps:</span>
              <span className="field-val">
                {cryptoProof.purchase_total_steps} sequential steps
              </span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Sealed Purchase Root Hash:</span>
              <span className="field-val mono text-xs break-all">
                {cryptoProof.purchase_root_hash || 'None'}
              </span>
            </div>
          </div>
        </div>

        {/* Dispute Audit Trail Card */}
        <div className="crypto-subcard">
          <div className="subcard-header">
            <span className="subcard-title">Dispute Audit Chain</span>
            <span
              className={`pill-status ${
                cryptoProof.dispute_chain_valid ? 'valid' : 'invalid'
              }`}
            >
              {cryptoProof.dispute_chain_valid ? 'VALID' : 'FAILED / TAMPERED'}
            </span>
          </div>
          <div className="subcard-content">
            <div className="crypto-field">
              <span className="field-lbl">Genesis Anchor Type:</span>
              <span className="field-val mono">
                {cryptoProof.genesis_anchor_type}
              </span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Audit Steps Count:</span>
              <span className="field-val">
                {cryptoProof.dispute_total_steps} audit steps
              </span>
            </div>
            <div className="crypto-field">
              <span className="field-lbl">Genesis Anchor Hash:</span>
              <span className="field-val mono text-xs break-all">
                {cryptoProof.genesis_anchor_hash}
              </span>
            </div>
          </div>
        </div>
      </div>

      {/* Prominent Scope Disclaimer */}
      <div className="disclaimer-callout">
        <div className="callout-icon">🛡️</div>
        <div>
          <div className="callout-header">Cryptographic Scope & Tamper-Detection Boundary</div>
          <p className="callout-text">{cryptoProof.integrity_scope_statement}</p>
        </div>
      </div>
    </div>
  );
};
