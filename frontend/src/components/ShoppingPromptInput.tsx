import React from 'react';

export interface PromptPreset {
  id: string;
  label: string;
  badge: string;
  text: string;
}

export const PROMPT_PRESETS: PromptPreset[] = [
  {
    id: 'commute-anker',
    label: 'Under $100 with 30h+ Battery',
    badge: 'Under $100',
    text: 'Buy wireless noise-canceling headphones under $100 with at least 30 hours of battery life',
  },
  {
    id: 'premium-sony',
    label: 'Premium Sony Wireless ANC',
    badge: 'Sony XM5',
    text: 'Find premium Sony wireless noise-canceling headphones under $450 with 25+ hours battery',
  },
  {
    id: 'budget-q20',
    label: 'Budget ANC under $60',
    badge: 'Budget $60',
    text: 'Find budget wireless headphones under $60 with noise cancelling and 25 hour battery',
  },
  {
    id: 'bose-brand',
    label: 'Bose with Microphone under $350',
    badge: 'Bose QC45',
    text: 'Find Bose noise-canceling headphones under $350 with microphone and wireless',
  },
];

interface ShoppingPromptInputProps {
  prompt: string;
  onPromptChange: (val: string) => void;
  onSubmit: (promptText: string) => void;
  loading: boolean;
  error: string | null;
  onClearError?: () => void;
  disabled?: boolean;
}

export const ShoppingPromptInput: React.FC<ShoppingPromptInputProps> = ({
  prompt,
  onPromptChange,
  onSubmit,
  loading,
  error,
  onClearError,
  disabled = false,
}) => {
  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = prompt.trim();
    if (!trimmed || loading || disabled) return;
    onSubmit(trimmed);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleSubmit(e);
    }
  };

  const handleSelectPreset = (presetText: string) => {
    if (loading || disabled) return;
    onPromptChange(presetText);
    if (error && onClearError) {
      onClearError();
    }
  };

  const isBlank = prompt.trim().length === 0;

  return (
    <div className="panel-card shopping-prompt-panel">
      <div className="shopping-prompt-header">
        <div>
          <span className="section-badge primary">Natural Language Input</span>
          <h2 className="panel-title" style={{ marginTop: '0.35rem' }}>
            Autonomous AI Shopping Assistant
          </h2>
          <p className="panel-description" style={{ marginBottom: '1rem' }}>
            Enter your purchase requirements in plain English. The agent extracts your constraints,
            explores the catalog, deterministically rejects non-compliant candidates, and selects the
            optimal product with full cryptographic proof.
          </p>
        </div>
      </div>

      <form onSubmit={handleSubmit} className="shopping-prompt-form">
        <div className="textarea-wrapper">
          <textarea
            className="shopping-textarea"
            rows={3}
            value={prompt}
            onChange={(e) => {
              onPromptChange(e.target.value);
              if (error && onClearError) onClearError();
            }}
            onKeyDown={handleKeyDown}
            placeholder="e.g., Buy wireless noise-canceling headphones under $100 with at least 30 hours of battery life..."
            disabled={loading || disabled}
            aria-label="Natural language shopping request"
          />
          <div className="textarea-footer">
            <span className="keyboard-hint">
              Press <strong>⌘ + Enter</strong> or <strong>Ctrl + Enter</strong> to submit
            </span>
            <span className="char-count">{prompt.length} characters</span>
          </div>
        </div>

        {error && (
          <div className="alert-card alert-error" role="alert" style={{ marginTop: '0.75rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: '0.5rem' }}>
              <div>
                <strong>Analysis Failed: </strong>
                <span>{error}</span>
              </div>
              {onClearError && (
                <button
                  type="button"
                  className="dismiss-btn"
                  onClick={onClearError}
                  aria-label="Dismiss error"
                >
                  ✕
                </button>
              )}
            </div>
          </div>
        )}

        <div className="prompt-actions-row">
          <div className="prompt-presets-container">
            <span className="presets-label">Catalog Presets:</span>
            <div className="presets-list">
              {PROMPT_PRESETS.map((preset) => (
                <button
                  key={preset.id}
                  type="button"
                  className={`preset-chip ${prompt === preset.text ? 'selected' : ''}`}
                  onClick={() => handleSelectPreset(preset.text)}
                  disabled={loading || disabled}
                  title={preset.text}
                >
                  <span className="preset-badge">{preset.badge}</span>
                  <span className="preset-name">{preset.label}</span>
                </button>
              ))}
            </div>
          </div>

          <button
            type="submit"
            className="btn-primary analyze-submit-btn"
            disabled={isBlank || loading || disabled}
          >
            {loading ? (
              <>
                <span className="spinner-inline" aria-hidden="true" />
                <span>Analyzing Catalog...</span>
              </>
            ) : (
              <>
                <span>Analyze Products</span>
                <span className="btn-arrow" aria-hidden="true">→</span>
              </>
            )}
          </button>
        </div>
      </form>
    </div>
  );
};
