import { useEffect, useRef, useState } from 'react';
import type { JSX } from 'react';
import { errorMessage } from '../../lib/errors';
import { harnessFeedbackFromTrace, type HarnessFeedbackEntry } from './harnessFeedback';

export function HarnessFeedbackViewer({ runId }: { runId: string }): JSX.Element {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [entries, setEntries] = useState<HarnessFeedbackEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    setOpen(false);
    setEntries([]);
    setError(null);
  }, [runId]);
  useEffect(() => {
    if (open) closeButtonRef.current?.focus();
  }, [open]);

  const showFeedback = async (): Promise<void> => {
    setOpen(true);
    setLoading(true);
    setError(null);
    try {
      const detail = await window.beale.getRunDetail(runId, 'full');
      const feedback = harnessFeedbackFromTrace(detail.traceEvents);
      const seen = new Set(feedback.map((entry) => entry.id));
      let cursor = detail.historyCursor;
      while (cursor?.hasEarlier && cursor.beforeEventId) {
        const beforeEventId = cursor.beforeEventId;
        const page = await window.beale.getRunHistoryPage(runId, beforeEventId, 'full');
        for (const entry of harnessFeedbackFromTrace(page.traceEvents)) {
          if (!seen.has(entry.id)) {
            seen.add(entry.id);
            feedback.push(entry);
          }
        }
        cursor = page.historyCursor;
        if (cursor.beforeEventId === beforeEventId) throw new Error('Session history did not advance.');
      }
      setEntries(feedback.sort((left, right) => left.createdAt.localeCompare(right.createdAt)));
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="harness-feedback-control">
      <button type="button" className="harness-feedback-button" onClick={() => void showFeedback()}>Agent feedback</button>
      {open ? (
        <div className="harness-feedback-backdrop" onKeyDown={(event) => {
          if (event.key === 'Escape') setOpen(false);
        }} onMouseDown={(event) => {
          if (event.target === event.currentTarget) setOpen(false);
        }}>
          <section className="harness-feedback-dialog" role="dialog" aria-modal="true" aria-label="Agent feedback">
            <header>
              <h2>Agent feedback</h2>
              <button ref={closeButtonRef} type="button" aria-label="Close agent feedback" onClick={() => setOpen(false)}>Close</button>
            </header>
            {loading ? <p>Loading feedback…</p> : error ? <p role="alert">{error}</p> : entries.length === 0
              ? <p>No harness feedback was recorded in this session.</p>
              : <ol>{entries.map((entry) => (
                <li key={entry.id}>
                  <strong>{entry.kind}</strong>
                  <p>{entry.observation}</p>
                  {entry.workaround ? <p><span>Workaround: </span>{entry.workaround}</p> : null}
                </li>
              ))}</ol>}
          </section>
        </div>
      ) : null}
    </div>
  );
}
