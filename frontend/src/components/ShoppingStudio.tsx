import React, { useState, useEffect, useRef } from 'react';
import type { UseShoppingJourneyReturn } from '../hooks/useShoppingJourney';
import type { Product } from '../types/api';
import { fetchCatalogProducts } from '../api/client';
import { ShoppingPromptInput } from './ShoppingPromptInput';
import { RequirementAnalysisCard } from './RequirementAnalysisCard';
import { CatalogExplorationMatrix } from './CatalogExplorationMatrix';
import { ProductSelectionCard } from './ProductSelectionCard';
import { PurchaseAuthorizationCard } from './PurchaseAuthorizationCard';
import { PayPalCheckoutCard } from './PayPalCheckoutCard';

interface ShoppingStudioProps {
  journey: UseShoppingJourneyReturn;
  onNavigateToAudit?: (sessionId: string) => void;
}

export const ShoppingStudio: React.FC<ShoppingStudioProps> = ({
  journey,
  onNavigateToAudit,
}) => {
  const [catalogProducts, setCatalogProducts] = useState<Product[]>([]);
  const [sessionCopied, setSessionCopied] = useState<boolean>(false);
  const [inputPrompt, setInputPrompt] = useState<string>(
    journey.prompt || 'Buy wireless noise-canceling headphones under $100 with at least 30 hours of battery life'
  );

  const authorizationRef = useRef<HTMLDivElement | null>(null);

  // Sync if journey prompt changes externally
  useEffect(() => {
    if (journey.prompt && journey.prompt !== inputPrompt) {
      setInputPrompt(journey.prompt);
    }
  }, [journey.prompt]);

  // Fetch full catalog once for rich comparative matrix
  useEffect(() => {
    let isMounted = true;
    fetchCatalogProducts()
      .then((prods) => {
        if (isMounted) setCatalogProducts(prods);
      })
      .catch(() => {
        // Non-critical; exploration matrix will still render API candidates
      });
    return () => {
      isMounted = false;
    };
  }, []);

  const handleCopySessionId = (id: string) => {
    void navigator.clipboard.writeText(id);
    setSessionCopied(true);
    setTimeout(() => setSessionCopied(false), 2000);
  };

  const handleScrollToAuthorization = () => {
    if (authorizationRef.current) {
      authorizationRef.current.scrollIntoView({ behavior: 'smooth' });
    }
  };

  const hasSession = !!journey.sessionResponse && !!journey.sessionId;
  const session = journey.sessionResponse;
  const isAnalyzing = journey.isAnalyzing;

  const isCaptured = !!journey.capture && journey.capture.status === 'COMPLETED';
  const isAuthorized =
    !!journey.authorization &&
    (journey.authorization.authorization_status === 'APPROVED' ||
      String(journey.authorization.authorization_status).toUpperCase() === 'APPROVED');
  const hasSelectedProduct = !!session?.selected_product;

  // Determine current journey progress step
  let currentStep = 1;
  if (isCaptured) {
    currentStep = 5;
  } else if (isAuthorized) {
    currentStep = 4;
  } else if (hasSelectedProduct) {
    currentStep = 3;
  } else if (isAnalyzing) {
    currentStep = 2;
  }

  return (
    <div className="shopping-studio-container">
      {/* Journey Progress Indicator */}
      <div className="journey-stepper-card">
        <div className="stepper-track">
          {/* Step 1 */}
          <div className={`stepper-step ${currentStep >= 1 ? 'completed' : ''} ${currentStep === 1 ? 'active' : ''}`}>
            <div className="step-circle">
              {currentStep > 1 ? <span>✓</span> : <span>1</span>}
            </div>
            <div className="step-meta">
              <span className="step-title">1. Shopping Request</span>
              <span className="step-desc">Natural language intent</span>
            </div>
          </div>

          <div className={`stepper-connector ${currentStep >= 2 ? 'active' : ''}`} />

          {/* Step 2 */}
          <div className={`stepper-step ${currentStep >= 2 ? 'completed' : ''} ${currentStep === 2 ? 'active' : ''}`}>
            <div className="step-circle">
              {currentStep > 2 ? <span>✓</span> : <span>2</span>}
            </div>
            <div className="step-meta">
              <span className="step-title">2. AI Catalog Search</span>
              <span className="step-desc">Constraints & rejections</span>
            </div>
          </div>

          <div className={`stepper-connector ${currentStep >= 3 ? 'active' : ''}`} />

          {/* Step 3 */}
          <div className={`stepper-step ${currentStep >= 3 ? 'completed' : ''} ${currentStep === 3 ? 'active' : ''}`}>
            <div className="step-circle">
              {currentStep > 3 ? <span>✓</span> : <span>3</span>}
            </div>
            <div className="step-meta">
              <span className="step-title">3. Recommendation</span>
              <span className="step-desc">Verified top product</span>
            </div>
          </div>

          <div className={`stepper-connector ${currentStep >= 4 ? 'active' : ''}`} />

          {/* Step 4 */}
          <div className={`stepper-step ${currentStep >= 4 ? 'completed' : ''} ${currentStep === 4 ? 'active' : ''}`}>
            <div className="step-circle">
              {currentStep > 4 ? <span>✓</span> : <span>4</span>}
            </div>
            <div className="step-meta">
              <span className="step-title">4. Authorization</span>
              <span className="step-desc">Customer consent & ceiling</span>
            </div>
          </div>

          <div className={`stepper-connector ${currentStep >= 5 ? 'active' : ''}`} />

          {/* Step 5 */}
          <div className={`stepper-step ${currentStep >= 5 ? 'completed' : ''} ${currentStep === 5 ? 'active' : ''}`}>
            <div className="step-circle">
              {currentStep === 5 && isCaptured ? <span>✓</span> : <span>5</span>}
            </div>
            <div className="step-meta">
              <span className="step-title">5. PayPal Settlement</span>
              <span className="step-desc">Authoritative capture</span>
            </div>
          </div>
        </div>
      </div>

      {/* Primary Input Panel */}
      <ShoppingPromptInput
        prompt={inputPrompt}
        onPromptChange={setInputPrompt}
        onSubmit={(promptText) => {
          void journey.startShopping(promptText);
        }}
        loading={isAnalyzing}
        error={journey.error}
        onClearError={journey.clearError}
        disabled={isAnalyzing}
      />

      {/* Active Session Status Bar (when a session has been analyzed) */}
      {hasSession && session && (
        <div className="active-session-bar">
          <div className="session-bar-left">
            <span className="status-indicator-dot pulse" />
            <span className="session-bar-label">Active Audit Session:</span>
            <code className="session-id-code">{session.session_id}</code>
            <button
              type="button"
              className="copy-pill-btn"
              onClick={() => handleCopySessionId(session.session_id)}
              title="Copy Session ID"
            >
              {sessionCopied ? '✓ Copied' : 'Copy ID'}
            </button>
            <span className="session-step-count">
              Sequence #{session.current_sequence_index} • Hash: {session.latest_hash.slice(0, 10)}...
            </span>
          </div>

          <div className="session-bar-right">
            {onNavigateToAudit && (
              <button
                type="button"
                className="btn-secondary btn-sm"
                onClick={() => onNavigateToAudit(session.session_id)}
                title="View full cryptographic hash chain and events"
              >
                Inspect Ledger Audit →
              </button>
            )}
            <button
              type="button"
              className="btn-secondary btn-sm"
              onClick={journey.resetJourney}
              title="Clear active session and start a new request"
            >
              Start New Search
            </button>
          </div>
        </div>
      )}

      {/* Loading Banner when analyzing */}
      {isAnalyzing && (
        <div className="analysis-progress-panel">
          <div className="spinner-large" aria-hidden="true" />
          <div className="analysis-progress-text">
            <h4>Autonomous Shopping Agent at Work</h4>
            <p>
              Parsing natural language constraints • Searching catalog database •
              Recording candidate evaluations • Committing cryptographic SHA-256 blocks...
            </p>
          </div>
        </div>
      )}

      {/* Result Cards Stack */}
      {hasSession && session && (
        <div className="shopping-results-stack">
          {/* 1. Structured Requirement Analysis */}
          <RequirementAnalysisCard
            requirements={session.requirements}
            prompt={session.prompt}
            sequenceIndex={1}
          />

          {/* 2. Catalog Exploration & Rejection Matrix */}
          <CatalogExplorationMatrix
            candidatesFound={session.candidates_found}
            rejectedCandidates={session.rejected_candidates}
            selectedProductId={session.selected_product?.product_id}
            catalogProducts={catalogProducts}
          />

          {/* 3. Recommended Product Selection */}
          {session.selected_product && (
            <ProductSelectionCard
              product={session.selected_product}
              rationale={session.selection_rationale}
              requirements={session.requirements}
              latestHash={session.latest_hash}
              sequenceIndex={session.current_sequence_index}
              onProceedToAuthorization={handleScrollToAuthorization}
            />
          )}

          {/* 4. Customer Purchase Authorization Card */}
          {session.selected_product && (
            <div ref={authorizationRef}>
              <PurchaseAuthorizationCard
                product={session.selected_product}
                requirements={session.requirements}
                authorization={journey.authorization}
                isAuthorizing={journey.isAuthorizing}
                error={journey.error}
                onAuthorize={async (approved, ceiling) => {
                  return journey.authorizePurchase(approved, ceiling);
                }}
                sequenceIndex={session.current_sequence_index + 1}
                currentHash={journey.sessionResponse?.latest_hash}
              />
            </div>
          )}

          {/* 5. PayPal Sandbox Checkout & Settlement Card */}
          {isAuthorized && (
            <PayPalCheckoutCard
              sessionId={session.session_id}
              authorization={journey.authorization}
              order={journey.order}
              orderStatus={journey.orderStatus}
              capture={journey.capture}
              isCreatingOrder={journey.isCreatingOrder}
              isCheckingStatus={journey.isCheckingOrderStatus}
              isCapturing={journey.isCapturing}
              error={journey.error}
              onCreateOrder={async () => {
                return journey.createOrder();
              }}
              onOpenApproval={() => {
                return journey.openBuyerApproval();
              }}
              onCheckStatus={async () => {
                return journey.checkOrderStatus(session.session_id);
              }}
              onCapture={async () => {
                return journey.captureOrder();
              }}
              onViewInNotary={(sid) => {
                if (onNavigateToAudit) {
                  onNavigateToAudit(sid);
                }
              }}
            />
          )}
        </div>
      )}
    </div>
  );
};
