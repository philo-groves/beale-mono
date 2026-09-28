import { memo, useMemo } from 'react';
import type { JSX } from 'react';
import { ArrowLeft, ArrowRight, Lightbulb } from 'lucide-react';
import type { ResearchGoalPhase } from '@shared/types';

const NEXT_STEP_COUNT = 3;

export interface ResearchGoalSeed {
  sentence: string;
  phase: ResearchGoalPhase;
  promptMarkdown?: string;
}

export const SessionNextStepsWidget = memo(function SessionNextStepsWidget({
  loading,
  suggestions,
  error,
  title = 'Suggestions',
  suggestionLimit = NEXT_STEP_COUNT,
  onBack,
  onSelect
}: {
  loading: boolean;
  suggestions: readonly string[];
  error: string | null;
  title?: string | null;
  suggestionLimit?: number;
  onBack?: () => void;
  onSelect: (sentence: string) => void;
}): JSX.Element {
  const visibleSuggestions = useMemo(
    () => suggestions.slice(0, suggestionLimit),
    [suggestionLimit, suggestions]
  );
  return (
    <section className="session-next-steps" aria-label="Suggestions" aria-busy={loading}>
      <header className="session-next-steps-header">
        {title ? <h3>{title}</h3> : null}
        {onBack ? (
          <button type="button" className="session-next-steps-back" onClick={onBack}>
            <ArrowLeft size={14} aria-hidden="true" />
            Categories
          </button>
        ) : null}
      </header>
      <div className="session-next-steps-list">
        {loading
          ? Array.from({ length: suggestionLimit }, (_, index) => (
              <div className="session-next-step-skeleton" key={index} aria-hidden="true">
                <span />
              </div>
            ))
          : error
            ? <div className="session-next-steps-error">{error}</div>
            : visibleSuggestions.length === 0
              ? <div className="session-next-steps-empty">No suggestions to show.</div>
              : visibleSuggestions.map((suggestion) => (
                <button
                  type="button"
                  className="session-next-step-button"
                  key={suggestion}
                  onClick={() => onSelect(suggestion)}
                >
                  <Lightbulb className="session-next-step-icon" size={14} aria-hidden="true" />
                  <span>{suggestion}</span>
                  <ArrowRight size={14} aria-hidden="true" />
                </button>
              ))}
      </div>
    </section>
  );
});
