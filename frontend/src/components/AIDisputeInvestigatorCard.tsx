import React, { useState } from 'react';
import { runDisputeAiInvestigation, ApiClientError } from '../api/client';
import type {
  AIDisputeInvestigationReport,
  FindingSeverity,
  EpistemicCategory,
} from '../types/api';

interface AIDisputeInvestigatorCardProps {
  disputeId: string;
}

export const AIDisputeInvestigatorCard: React.FC<AIDisputeInvestigatorCardProps> = ({
  disputeId,
}) => {
  const [report, setReport] = useState<AIDisputeInvestigationReport | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [error, setError] = useState<string | null>(null);
  const [currentAction, setCurrentAction] = useState<'ai' | 'fallback' | null>(null);

  const handleRunInvestigation = async (forceFallback = false) => {
    if (!disputeId.trim()) return;

    setLoading(true);
    setError(null);
    setCurrentAction(forceFallback ? 'fallback' : 'ai');

    try {
      const result = await runDisputeAiInvestigation(disputeId, {
        force_fallback: forceFallback,
      });
      setReport(result);
    } catch (err: unknown) {
      if (err instanceof ApiClientError) {
        setError(`Investigation failed (${err.status}): ${err.message}`);
      } else if (err instanceof Error) {
        setError(err.message);
      } else {
        setError('An unexpected error occurred while running the dispute investigation.');
      }
    } finally {
      setLoading(false);
      setCurrentAction(null);
    }
  };

  const renderEpistemicTag = (cat: EpistemicCategory | string) => {
    switch (cat) {
      case 'VERIFIED_FACT':
        return <span className="epistemic-tag verified-fact">VERIFIED_FACT</span>;
      case 'RECORDED_PAYPAL_DATA':
        return <span className="epistemic-tag paypal-data">RECORDED_PAYPAL_DATA</span>;
      case 'DETERMINISTIC_INFERENCE':
        return <span className="epistemic-tag deterministic">DETERMINISTIC_INFERENCE</span>;
      case 'UNAVAILABLE_EVIDENCE':
        return <span className="epistemic-tag unavailable">UNAVAILABLE_EVIDENCE</span>;
      default:
        return <span className="epistemic-tag deterministic">{cat}</span>;
    }
  };

  const renderSeverityBadge = (severity: FindingSeverity) => {
    const s = severity.toUpperCase();
    if (s === 'HIGH') {
      return <span className="severity-badge severity-high">HIGH SEVERITY</span>;
    } else if (s === 'MEDIUM') {
      return <span className="severity-badge severity-medium">MEDIUM SEVERITY</span>;
    }
    return <span className="severity-badge severity-low">LOW SEVERITY</span>;
  };

  return (
    <div className="section-card ai-investigator-card">
      {/* Header */}
      <div className="section-card-header">
        <div className="section-title-wrap">
          <span className="epistemic-tag ai-tag">
            {report?.is_ai_generated ? 'GEMINI_AI_SYNTHESIS' : 'AI_DISPUTE_INVESTIGATOR'}
          </span>
          <h3 className="section-card-title">AI Dispute Investigator</h3>
        </div>

        <div className="header-badges">
          {report && (
            <span
              className={`provider-badge ${
                report.is_ai_generated ? 'provider-ai' : 'provider-deterministic'
              }`}
            >
              {report.is_ai_generated ? '⚡ Google Gemini' : '🛡️ Deterministic Engine'} (
              {report.provider})
            </span>
          )}
          {report?.ledger_hash_verified && (
            <span className="status-badge-outline matched">Ledger Chain: VERIFIED</span>
          )}
        </div>
      </div>

      <p className="ai-investigator-purpose">
        Impartial forensic synthesis of autonomous shopping agent actions, cryptographically sealed
        ledger steps, and recorded PayPal settlement events to assess dispute posture and detect
        potential inconsistencies.
      </p>

      {/* Action Bar (Trigger Buttons) */}
      <div className="ai-actions-bar">
        <button
          type="button"
          className="btn-primary btn-ai"
          disabled={loading || !disputeId}
          onClick={() => void handleRunInvestigation(false)}
        >
          {loading && currentAction === 'ai' ? (
            <>
              <span className="spinner-sm" /> Running AI Investigation...
            </>
          ) : (
            <>✨ {report ? 'Re-run AI Investigation' : 'Run AI Investigation'}</>
          )}
        </button>

        <button
          type="button"
          className="btn-secondary btn-fallback"
          disabled={loading || !disputeId}
          onClick={() => void handleRunInvestigation(true)}
          title="Run 100% deterministic local rule checks bypassing external AI API calls"
        >
          {loading && currentAction === 'fallback' ? (
            <>
              <span className="spinner-sm" /> Running Local Audit...
            </>
          ) : (
            <>⚙️ Run Deterministic Engine (Offline Fallback)</>
          )}
        </button>
      </div>

      {/* Loading Indicator */}
      {loading && (
        <div className="ai-loading-box">
          <div className="ai-loading-spinner" />
          <div className="ai-loading-text">
            <strong>
              {currentAction === 'ai'
                ? 'Synthesizing Dispute Forensics with Gemini...'
                : 'Evaluating Deterministic Rule Matrices...'}
            </strong>
            <p>
              Auditing dual-chain hashes, matching buyer authorization limits against item prices,
              and cross-referencing PayPal webhook settlement.
            </p>
          </div>
        </div>
      )}

      {/* Error Banner with Retry */}
      {error && !loading && (
        <div className="banner-notice warning">
          <div className="banner-icon">⚠️</div>
          <div style={{ flex: 1 }}>
            <strong>Investigation Error:</strong>
            <p>{error}</p>
            <button
              type="button"
              className="btn-secondary btn-sm"
              style={{ marginTop: '0.5rem' }}
              onClick={() => void handleRunInvestigation(false)}
            >
              Retry AI Investigation
            </button>
          </div>
        </div>
      )}

      {/* Results View */}
      {report && !loading && (
        <div className="ai-report-content">
          {/* Discarded References Warning (Hallucination Prevention) */}
          {report.discarded_unverified_references &&
            report.discarded_unverified_references.length > 0 && (
              <div className="banner-notice warning discarded-warning">
                <div className="banner-icon">🛡️</div>
                <div>
                  <strong>Backend Grounding Guard Discarded Unverified Reference(s):</strong>
                  <p>
                    The following reference(s) generated by the model could not be verified
                    against the cryptographic evidence brief and were safely excluded from the
                    authoritative report to prevent hallucination:
                  </p>
                  <div className="discarded-tags-list">
                    {report.discarded_unverified_references.map((ref, idx) => (
                      <span key={idx} className="discarded-tag mono">
                        {ref}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            )}

          {/* Provider Notice & Privacy Transparency */}
          <div className="ai-provider-note">
            <span className="provider-note-title">Provider & Privacy Notice:</span>{' '}
            {report.provider_note}
          </div>

          {/* 1. Executive Summary */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">1. Executive Summary</h4>
            <div className="executive-summary-box">
              <p>{report.executive_summary}</p>
            </div>
          </div>

          {/* 2. Potential Inconsistencies / Mistakes */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">
              2. Potential Inconsistencies & Operational Flags (
              {report.inconsistencies_or_red_flags.length})
            </h4>

            {report.inconsistencies_or_red_flags.length === 0 ? (
              <div className="empty-subnotice">
                <span>✓</span> No logical contradictions, budget overruns, or operational red flags
                were detected.
              </div>
            ) : (
              <div className="inconsistencies-grid">
                {report.inconsistencies_or_red_flags.map((item, idx) => (
                  <div key={idx} className="inconsistency-card">
                    <div className="inconsistency-header">
                      {renderSeverityBadge(item.severity)}
                      <strong className="inconsistency-finding">{item.finding}</strong>
                    </div>
                    <p className="inconsistency-details">{item.details}</p>
                    {item.evidence_ref && (
                      <div className="inconsistency-evidence">
                        <span className="evidence-lbl">Cited Evidence Anchor:</span>
                        <code className="mono evidence-code">{item.evidence_ref}</code>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* 3. Chronological Narrative Timeline */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">3. Chronological Narrative Timeline</h4>
            <div className="ai-narrative-timeline">
              {report.narrative_timeline.map((item, idx) => (
                <div key={idx} className="narrative-step">
                  <div className="narrative-step-marker">{idx + 1}</div>
                  <div className="narrative-step-body">
                    <div className="narrative-step-header">
                      <strong className="narrative-phase">{item.phase}</strong>
                      {renderEpistemicTag(item.evidence_type)}
                    </div>
                    <p className="narrative-desc">{item.description}</p>
                    {item.evidence_id && (
                      <div className="narrative-anchor">
                        <span className="anchor-label">Anchor ID:</span>
                        <code className="mono anchor-code">{item.evidence_id}</code>
                      </div>
                    )}
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* 4. Dual Evidence Comparison (Supporting vs Contradicting) */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">4. Dual Evidence Comparison</h4>
            <div className="dual-evidence-grid">
              {/* Supporting Dispute Column */}
              <div className="evidence-column supporting-col">
                <div className="column-header">
                  <span className="col-badge supporting-badge">Customer Position</span>
                  <h5>Evidence Supporting Dispute ({report.evidence_supporting_dispute.length})</h5>
                </div>
                {report.evidence_supporting_dispute.length === 0 ? (
                  <div className="empty-subnotice">No affirmative customer evidence recorded.</div>
                ) : (
                  <div className="evidence-items-stack">
                    {report.evidence_supporting_dispute.map((item, idx) => (
                      <div key={idx} className="evidence-mini-card">
                        <div className="mini-card-header">
                          <span className="source-label">{item.source}</span>
                          {renderEpistemicTag('RECORDED_PAYPAL_DATA')}
                        </div>
                        <p className="claim-text">{item.claim}</p>
                        {item.evidence_id && (
                          <div className="evidence-ref-tag">
                            ID: <code className="mono">{item.evidence_id}</code>
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {/* Contradicting Dispute Column */}
              <div className="evidence-column contradicting-col">
                <div className="column-header">
                  <span className="col-badge contradicting-badge">Merchant Defense</span>
                  <h5>
                    Evidence Contradicting Dispute (
                    {report.evidence_contradicting_dispute.length})
                  </h5>
                </div>
                {report.evidence_contradicting_dispute.length === 0 ? (
                  <div className="empty-subnotice">No contradiction proofs recorded.</div>
                ) : (
                  <div className="evidence-items-stack">
                    {report.evidence_contradicting_dispute.map((item, idx) => (
                      <div key={idx} className="evidence-mini-card merchant-defense">
                        <div className="mini-card-header">
                          <span className="source-label">{item.source}</span>
                          {renderEpistemicTag(item.epistemic_category)}
                          {item.hash_verified && (
                            <span className="hash-verified-pill" title="Verified by backend cryptographic ledger check">
                              ✓ Hash Verified
                            </span>
                          )}
                        </div>
                        <p className="claim-text">{item.fact}</p>
                        {item.evidence_id && (
                          <div className="evidence-ref-tag">
                            ID: <code className="mono">{item.evidence_id}</code>
                          </div>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* 5. Missing Evidence Disclaimers */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">5. Missing Evidence Disclaimers</h4>
            <div className="missing-evidence-grid">
              {report.missing_evidence.map((item, idx) => (
                <div key={idx} className="missing-evidence-card">
                  <div className="missing-card-header">
                    <span className="epistemic-tag unavailable">UNAVAILABLE_EVIDENCE</span>
                    <strong className="missing-item-name">{item.item}</strong>
                  </div>
                  <p className="missing-reason">{item.reason_unavailable}</p>
                </div>
              ))}
            </div>
          </div>

          {/* 6. Recommended Next Steps */}
          <div className="ai-report-section">
            <h4 className="ai-section-title">6. Recommended Next Steps</h4>
            <div className="recommendations-stack">
              {report.recommended_next_steps.map((item, idx) => (
                <div key={idx} className="recommendation-card">
                  <div className="rec-priority-badge">Step {item.priority || idx + 1}</div>
                  <div className="rec-body">
                    <strong className="rec-action">{item.action}</strong>
                    <p className="rec-rationale">{item.rationale}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Report Footer / Transparency Disclaimer */}
          <div className="ai-report-footer">
            <span>
              Generated at: <strong>{new Date(report.generated_at).toLocaleString()}</strong>
            </span>
            <span>
              Valid Evidence References Grounded: <strong>{report.valid_evidence_references.length}</strong>
            </span>
            <span className="disclaimer-text">
              Note: Forensic AI synthesis is designed to assist human dispute reviewers and
              adjudicators by indexing cryptographic proofs; it does not constitute a final legal or
              judicial determination.
            </span>
          </div>
        </div>
      )}
    </div>
  );
};
