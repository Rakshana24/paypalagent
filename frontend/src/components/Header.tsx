import React from 'react';
import type { HealthResponse, AuthCheckResponse } from '../types/api';

interface HeaderProps {
  health: HealthResponse | null;
  healthLoading: boolean;
  healthError: string | null;
  authStatus: AuthCheckResponse | null;
  authLoading: boolean;
  authError: string | null;
  onRefresh: () => void;
}

export const Header: React.FC<HeaderProps> = ({
  health,
  healthLoading,
  healthError,
  authStatus,
  authLoading,
  authError,
  onRefresh,
}) => {
  return (
    <header className="app-header">
      <div className="header-content">
        <div>
          <div className="brand-title">
            <span>AgentAudit</span>
            <span className="brand-badge">Audit Evidence</span>
          </div>
          <div className="brand-subtitle">
            AI Commerce Dispute Evidence & Cryptographic Verification
          </div>
        </div>

        <div className="header-right">
          <div className="header-status-pills">
            {/* Backend Service Status */}
            <div
              className={`status-pill ${
                healthLoading
                  ? 'loading'
                  : health?.success
                  ? 'online'
                  : 'error'
              }`}
              title={healthLoading ? 'Checking backend...' : health?.message || healthError || 'Offline'}
            >
              <span className="status-dot"></span>
              <span>Backend: {healthLoading ? 'Checking...' : health?.success ? 'Online' : 'Offline'}</span>
            </div>

            {/* PayPal Sandbox Status */}
            <div
              className={`status-pill ${
                authLoading
                  ? 'loading'
                  : authStatus?.success
                  ? 'online'
                  : 'error'
              }`}
              title={
                authLoading
                  ? 'Checking PayPal Sandbox auth...'
                  : authStatus
                  ? `${authStatus.message} (${authStatus.token_type})`
                  : authError || 'Auth Failed'
              }
            >
              <span className="status-dot"></span>
              <span>
                PayPal Sandbox:{' '}
                {authLoading
                  ? 'Checking...'
                  : authStatus?.success
                  ? 'Authenticated'
                  : 'Auth Failed'}
              </span>
            </div>
          </div>

          <button className="btn-secondary btn-sm" onClick={onRefresh} title="Refresh connection status">
            Refresh Status
          </button>
        </div>
      </div>
    </header>
  );
};
