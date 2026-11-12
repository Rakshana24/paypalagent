import React, { useState, useEffect, useCallback } from 'react';
import {
  fetchHealth,
  fetchPaypalAuthCheck,
  fetchDisputeEvidence,
  fetchDisputeEvidenceMarkdown,
  ApiClientError,
} from './api/client';
import type {
  HealthResponse,
  AuthCheckResponse,
  DisputeEvidenceBrief,
} from './types/api';

import { Header } from './components/Header';
import { EvidenceLookup } from './components/EvidenceLookup';
import { DisputeSummaryCard } from './components/DisputeSummaryCard';
import { CryptographicVerificationCard } from './components/CryptographicVerificationCard';
import { AgentDecisionTimeline } from './components/AgentDecisionTimeline';
import { TransactionProofCard } from './components/TransactionProofCard';
import { RebuttalSection } from './components/RebuttalSection';
import { AIDisputeInvestigatorCard } from './components/AIDisputeInvestigatorCard';
import { MarkdownViewer } from './components/MarkdownViewer';
import { PurchaseAuditPreview } from './components/PurchaseAuditPreview';
import { ShoppingStudio } from './components/ShoppingStudio';
import { useShoppingJourney } from './hooks/useShoppingJourney';

type ViewTab = 'investigation' | 'markdown' | 'json';
type DashboardMode = 'shopping-studio' | 'purchase-audit' | 'dispute-investigation';

export const App: React.FC = () => {
  // Mode selection state - AI Shopping Studio is the first destination
  const [dashboardMode, setDashboardMode] = useState<DashboardMode>('shopping-studio');
  const [selectedAuditSessionId, setSelectedAuditSessionId] = useState<string>('');

  // Hoist shopping journey state at the app level to prevent navigation resets
  const journey = useShoppingJourney();

  // System status state
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [healthError, setHealthError] = useState<string | null>(null);
  const [healthLoading, setHealthLoading] = useState<boolean>(true);

  const [authStatus, setAuthStatus] = useState<AuthCheckResponse | null>(null);
  const [authError, setAuthError] = useState<string | null>(null);
  const [authLoading, setAuthLoading] = useState<boolean>(true);

  // Evidence query state
  const [activeTab, setActiveTab] = useState<ViewTab>('investigation');
  const [currentDisputeId, setCurrentDisputeId] = useState<string>('');
  const [evidenceBrief, setEvidenceBrief] = useState<DisputeEvidenceBrief | null>(null);
  const [evidenceMarkdown, setEvidenceMarkdown] = useState<string | null>(null);
  const [evidenceLoading, setEvidenceLoading] = useState<boolean>(false);
  const [evidenceError, setEvidenceError] = useState<{
    message: string;
    isNotFound: boolean;
  } | null>(null);
  const [jsonCopied, setJsonCopied] = useState<boolean>(false);

  const loadSystemStatus = useCallback(async () => {
    setHealthLoading(true);
    setHealthError(null);
    setAuthLoading(true);
    setAuthError(null);

    try {
      const h = await fetchHealth();
      setHealth(h);
    } catch (err: unknown) {
      setHealthError(err instanceof Error ? err.message : 'Failed to reach backend');
    } finally {
      setHealthLoading(false);
    }

    try {
      const a = await fetchPaypalAuthCheck();
      setAuthStatus(a);
    } catch (err: unknown) {
      setAuthError(err instanceof Error ? err.message : 'PayPal auth check failed');
    } finally {
      setAuthLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSystemStatus();
  }, [loadSystemStatus]);

  const handleSearch = async (disputeId: string) => {
    setEvidenceLoading(true);
    setEvidenceError(null);
    setEvidenceBrief(null);
    setEvidenceMarkdown(null);
    setCurrentDisputeId(disputeId);
    setJsonCopied(false);

    try {
      const [brief, md] = await Promise.all([
        fetchDisputeEvidence(disputeId),
        fetchDisputeEvidenceMarkdown(disputeId),
      ]);
      setEvidenceBrief(brief);
      setEvidenceMarkdown(md);
      setActiveTab('investigation');
    } catch (err: unknown) {
      const isNotFound = err instanceof ApiClientError && err.status === 404;
      const message =
        err instanceof Error ? err.message : 'Failed to load evidence brief';
      setEvidenceError({ message, isNotFound });
    } finally {
      setEvidenceLoading(false);
    }
  };

  const handleClear = () => {
    setEvidenceBrief(null);
    setEvidenceMarkdown(null);
    setEvidenceError(null);
    setCurrentDisputeId('');
  };

  const handleCopyJson = () => {
    if (evidenceBrief) {
      void navigator.clipboard.writeText(JSON.stringify(evidenceBrief, null, 2));
      setJsonCopied(true);
      setTimeout(() => setJsonCopied(false), 2000);
    }
  };

  return (
    <div className="app-container">
      {/* Header */}
      <Header
        health={health}
        healthLoading={healthLoading}
        healthError={healthError}
        authStatus={authStatus}
        authLoading={authLoading}
        authError={authError}
        onRefresh={() => void loadSystemStatus()}
      />

      {/* Main Content Area */}
      <main className="main-content">
        {/* Top-Level Dashboard Mode Switcher */}
        <div className="dashboard-mode-selector">
          <button
            type="button"
            className={`mode-btn ${dashboardMode === 'shopping-studio' ? 'active' : ''}`}
            onClick={() => setDashboardMode('shopping-studio')}
          >
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              <span className="mode-badge primary">Customer Journey</span>
              {journey.sessionId && (
                <span className="mode-mini-status pulse" title="Active Shopping Session in Memory">
                  Active
                </span>
              )}
            </div>
            <span className="mode-title">1. AI Shopping & Studio</span>
            <span className="mode-desc">
              Natural language shopping agent, constraints extraction and product recommendation
            </span>
          </button>

          <button
            type="button"
            className={`mode-btn ${dashboardMode === 'purchase-audit' ? 'active' : ''}`}
            onClick={() => setDashboardMode('purchase-audit')}
          >
            <span className="mode-badge demo">Verified Ledger</span>
            <span className="mode-title">2. Purchase Audit Preview</span>
            <span className="mode-desc">
              Inspect completed purchase session, event timeline, and cryptographic chain
            </span>
          </button>

          <button
            type="button"
            className={`mode-btn ${dashboardMode === 'dispute-investigation' ? 'active' : ''}`}
            onClick={() => setDashboardMode('dispute-investigation')}
          >
            <span className="mode-badge dispute">Formal Dispute</span>
            <span className="mode-title">3. Dispute Evidence Investigation</span>
            <span className="mode-desc">
              Adjudication rebuttal brief, dual-chain proofs, and PayPal settlement
            </span>
          </button>
        </div>

        {/* View 1: AI Shopping Studio (Milestone 5B) */}
        {dashboardMode === 'shopping-studio' && (
          <ShoppingStudio
            journey={journey}
            onNavigateToAudit={(sessionId) => {
              setSelectedAuditSessionId(sessionId);
              setDashboardMode('purchase-audit');
            }}
          />
        )}

        {/* View 2: Purchase Audit Preview */}
        {dashboardMode === 'purchase-audit' && (
          <PurchaseAuditPreview initialSessionId={selectedAuditSessionId} />
        )}

        {/* View 3: Dispute Evidence Investigation */}
        {dashboardMode === 'dispute-investigation' && (
          <>
            {/* Lookup Card */}
            <EvidenceLookup
              onSearch={(id) => void handleSearch(id)}
              loading={evidenceLoading}
              error={evidenceError}
              onClear={handleClear}
              hasResult={!!evidenceBrief}
            />

            {/* Evidence Brief Investigation Results */}
            {evidenceBrief && (
              <div className="investigation-wrapper">
                {/* Investigation View Switcher */}
                <div className="view-switcher-bar">
                  <div className="tab-buttons">
                    <button
                      className={`tab-btn ${activeTab === 'investigation' ? 'active' : ''}`}
                      onClick={() => setActiveTab('investigation')}
                    >
                      Structured Evidence Brief
                    </button>
                    <button
                      className={`tab-btn ${activeTab === 'markdown' ? 'active' : ''}`}
                      onClick={() => setActiveTab('markdown')}
                    >
                      Markdown Report
                    </button>
                    <button
                      className={`tab-btn ${activeTab === 'json' ? 'active' : ''}`}
                      onClick={() => setActiveTab('json')}
                    >
                      Raw JSON Payload
                    </button>
                  </div>

                  {activeTab === 'json' && (
                    <button className="btn-secondary btn-sm" onClick={handleCopyJson}>
                      {jsonCopied ? '✓ Copied JSON!' : 'Copy JSON'}
                    </button>
                  )}
                </div>

                {/* TAB 1: Structured Evidence Investigation */}
                {activeTab === 'investigation' && (
                  <div className="evidence-sections-stack">
                    {/* 1. Dispute Summary & Correlation */}
                    <DisputeSummaryCard
                      disputeSummary={evidenceBrief.dispute_summary}
                      correlationSummary={evidenceBrief.correlation_summary}
                    />

                    {/* AI Dispute Investigator */}
                    <AIDisputeInvestigatorCard disputeId={currentDisputeId} />

                    {/* 2. Cryptographic Proof & Ledger Integrity */}
                    <CryptographicVerificationCard
                      cryptoProof={evidenceBrief.cryptographic_proof}
                    />

                    {/* 3. Autonomous Agent Decision Timeline */}
                    <AgentDecisionTimeline
                      trail={evidenceBrief.agent_audit_trail}
                      trans={evidenceBrief.transaction_proof}
                    />

                    {/* 4. Transaction Proof & Settlement */}
                    <TransactionProofCard
                      transactionProof={evidenceBrief.transaction_proof}
                    />

                    {/* 5. Merchant Rebuttal Strategy & Missing Evidence */}
                    <RebuttalSection
                      rebuttal={evidenceBrief.rebuttal_summary}
                      missingEvidence={evidenceBrief.missing_evidence}
                    />
                  </div>
                )}

                {/* TAB 2: Markdown Report View */}
                {activeTab === 'markdown' && (
                  <MarkdownViewer
                    markdown={evidenceMarkdown}
                    loading={evidenceLoading}
                    disputeId={currentDisputeId}
                  />
                )}

                {/* TAB 3: Raw JSON Payload */}
                {activeTab === 'json' && (
                  <div className="section-card">
                    <div className="section-card-header">
                      <div>
                        <h3 className="section-card-title">Structured Evidence Brief JSON</h3>
                        <span className="section-card-subtitle">
                          Complete payload from <code>GET /api/disputes/{currentDisputeId}/evidence</code>
                        </span>
                      </div>
                    </div>
                    <div className="code-container">
                      <pre>{JSON.stringify(evidenceBrief, null, 2)}</pre>
                    </div>
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </main>

      {/* Footer */}
      <footer className="app-footer">
        <div>
          <strong>AgentAudit</strong> • Grounded in Immutable Cryptographic Proofs • Hackathon Edition
        </div>
        <div style={{ marginTop: '0.25rem', fontSize: '0.75rem', color: '#94a3b8' }}>
          SHA-256 Chain Verification • RFC 8785 Canonical JSON • PayPal Sandbox Integration
        </div>
      </footer>
    </div>
  );
};
