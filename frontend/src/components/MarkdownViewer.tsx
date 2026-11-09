import React, { useState } from 'react';

interface MarkdownViewerProps {
  markdown: string | null;
  loading: boolean;
  disputeId: string;
}

export const MarkdownViewer: React.FC<MarkdownViewerProps> = ({
  markdown,
  loading,
  disputeId,
}) => {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    if (markdown) {
      void navigator.clipboard.writeText(markdown);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  if (loading) {
    return (
      <div className="section-card">
        <div className="empty-state-notice">
          <span className="spinner"></span>
          <p>Loading authoritative Markdown evidence brief from backend...</p>
        </div>
      </div>
    );
  }

  if (!markdown) {
    return (
      <div className="section-card">
        <div className="empty-state-notice">
          <p>No Markdown report available for dispute {disputeId}.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="section-card">
      <div className="section-card-header">
        <div>
          <h3 className="section-card-title">6. Authoritative Markdown Evidence Brief</h3>
          <span className="section-card-subtitle">
            Exportable report generated directly by <code>GET /api/disputes/{disputeId}/evidence/markdown</code>
          </span>
        </div>
        <button className="btn-secondary" onClick={handleCopy}>
          {copied ? '✓ Copied to Clipboard!' : 'Copy Markdown'}
        </button>
      </div>

      <div className="markdown-report-view">
        <pre className="markdown-code-block">{markdown}</pre>
      </div>
    </div>
  );
};
