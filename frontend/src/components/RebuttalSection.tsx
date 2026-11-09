import React from 'react';
import type { RebuttalSummary, MissingEvidenceDisclaimer } from '../types/api';

interface RebuttalSectionProps {
  rebuttal: RebuttalSummary;
  missingEvidence: MissingEvidenceDisclaimer;
}

export const RebuttalSection: React.FC<RebuttalSectionProps> = ({
  rebuttal,
  missingEvidence,
}) => {
  return (
    <div className="section-card">
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag deterministic">DETERMINISTIC_INFERENCE</span>
          <h3 className="section-card-title">5. Merchant Rebuttal Strategy &amp; Missing Evidence</h3>
        </div>
        <div className="header-badges">
          <span
            className={`action-pill ${
              rebuttal.recommended_action === 'CONTEST_WITH_EVIDENCE'
                ? 'contest'
                : rebuttal.recommended_action === 'CONTEST_WITH_FULFILLMENT_PROOF_REQUIRED'
                ? 'fulfillment'
                : 'manual'
            }`}
          >
            Recommended Action: {rebuttal.recommended_action}
          </span>
        </div>
      </div>

      {/* Analysis Callout */}
      <div className="rebuttal-analysis-box">
        <h4 className="analysis-heading">Strategic Adjudication Assessment:</h4>
        <p className="analysis-text">{rebuttal.reason_specific_analysis}</p>
      </div>

      {/* Primary Arguments List */}
      <div className="arguments-section">
        <h4 className="sub-heading">Primary Factual Arguments for Adjudication</h4>
        <div className="arguments-list">
          {rebuttal.primary_arguments.map((arg, idx) => (
            <div key={idx} className="argument-card">
              <div className="arg-header">
                <span className="arg-index">Argument 5.{idx + 1}</span>
                <span
                  className={`epistemic-tag text-xs ${
                    arg.category === 'VERIFIED_FACT'
                      ? 'verified-fact'
                      : arg.category === 'UNAVAILABLE_EVIDENCE'
                      ? 'unavailable'
                      : 'deterministic'
                  }`}
                >
                  {arg.category}
                </span>
                <span className="arg-claim">{arg.claim}</span>
              </div>
              <p className="arg-explanation">{arg.explanation}</p>
              {arg.source_references.length > 0 && (
                <div className="arg-sources">
                  <span className="sources-lbl">Source Anchors:</span>
                  {arg.source_references.map((src, sIdx) => (
                    <span key={sIdx} className="source-tag">
                      {src.action_type || 'Record'}{' '}
                      {src.sequence_index !== undefined ? `(seq ${src.sequence_index})` : ''}
                      {src.step_hash ? ` [${src.step_hash.slice(0, 10)}...]` : ''}
                    </span>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* Missing Evidence Disclaimers */}
      <div className="missing-evidence-section">
        <div className="missing-header">
          <span className="epistemic-tag unavailable">UNAVAILABLE_EVIDENCE</span>
          <h4 className="sub-heading">Explicit Missing Evidence Accounting</h4>
        </div>
        <p className="missing-disclaimer-note">{missingEvidence.general_disclaimer}</p>

        <div className="missing-items-table-wrap">
          <table className="missing-table">
            <thead>
              <tr>
                <th>Evidence Category</th>
                <th>Reason Unavailable in AgentAudit</th>
                <th>Adjudication Impact / Required Merchant Action</th>
              </tr>
            </thead>
            <tbody>
              {missingEvidence.missing_items.map((item, idx) => (
                <tr key={idx}>
                  <td className="item-type-cell">
                    <code>{item.evidence_type}</code>
                  </td>
                  <td className="item-reason-cell">{item.reason_unavailable}</td>
                  <td className="item-impact-cell">{item.adjudication_impact}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
};
