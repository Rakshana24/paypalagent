import React, { useState } from 'react';

interface EvidenceLookupProps {
  onSearch: (disputeId: string) => void;
  loading: boolean;
  error: { message: string; isNotFound: boolean } | null;
  onClear: () => void;
  hasResult: boolean;
}

export const EvidenceLookup: React.FC<EvidenceLookupProps> = ({
  onSearch,
  loading,
  error,
  onClear,
  hasResult,
}) => {
  const [inputVal, setInputVal] = useState('');

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = inputVal.trim();
    if (trimmed) {
      onSearch(trimmed);
    }
  };

  const handleClear = () => {
    setInputVal('');
    onClear();
  };

  return (
    <section className="panel-card lookup-panel">
      <div className="lookup-header">
        <div>
          <h2 className="panel-title">Dispute Evidence Investigation</h2>
          <p className="panel-description">
            Enter a PayPal Dispute identifier to retrieve and verify its cryptographic audit trail,
            settlement provenance, and synthesized merchant rebuttal.
          </p>
        </div>
      </div>

      <form onSubmit={handleSubmit} className="lookup-form">
        <div className="input-row">
          <input
            type="text"
            className="input-field"
            placeholder="Enter PayPal Dispute ID (e.g. PP-D-12345 or PP-D-TEST-001)"
            value={inputVal}
            onChange={(e) => setInputVal(e.target.value)}
            disabled={loading}
            autoFocus
          />
          <button
            type="submit"
            className="btn-primary"
            disabled={loading || !inputVal.trim()}
          >
            {loading ? (
              <span className="btn-loading">
                <span className="spinner"></span> Generating Brief...
              </span>
            ) : (
              'Fetch Evidence Brief'
            )}
          </button>
          {hasResult && (
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

      {/* Error state */}
      {error && (
        <div className={`alert-card ${error.isNotFound ? 'warning' : 'error'}`}>
          <div className="alert-title">
            {error.isNotFound ? 'Dispute Not Ingested in AgentAudit' : 'Investigation Error'}
          </div>
          <div className="alert-body">
            {error.isNotFound ? (
              <>
                <p>
                  <strong>{error.message}</strong>
                </p>
                <p style={{ marginTop: '0.5rem', fontSize: '0.8125rem' }}>
                  This dispute identifier is not present in AgentAudit's ingested SQLite records.
                  To generate an evidence brief, the dispute must first be ingested via PayPal webhook
                  (e.g. <code>CUSTOMER.DISPUTE.CREATED</code>) or registered through the dispute processing pipeline.
                </p>
              </>
            ) : (
              <p>{error.message}</p>
            )}
          </div>
        </div>
      )}
    </section>
  );
};
