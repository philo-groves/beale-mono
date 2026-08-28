import type { JSX } from 'react';
import { Activity, FileJson, RefreshCw } from 'lucide-react';
import type { ProfilingReport, ProfilingState } from '@shared/types';
import { Modal } from '../../app/Modal';
import { profilingLagFindings } from '../../view-models/profilingAnalysis';

export function ProfilingModal({
  state,
  report,
  onClose,
  onFlush,
  onSetEnabled
}: {
  state: ProfilingState | null;
  report: ProfilingReport | null;
  onClose: () => void;
  onFlush: () => void;
  onSetEnabled: (enabled: boolean) => Promise<void>;
}): JSX.Element {
  const enabled = state?.enabled ?? false;
  const topTimings = report?.timings.slice().sort((a, b) => b.maxMs - a.maxMs).slice(0, 8) ?? [];
  const topRenders = report?.renders.slice().sort((a, b) => b.renders - a.renders).slice(0, 8) ?? [];
  const findings = profilingLagFindings(report);
  const worstSamples = report?.samples
    ?.filter((sample) => sample.durationMs !== null)
    .sort((left, right) => (right.durationMs ?? 0) - (left.durationMs ?? 0))
    .slice(0, 8) ?? [];

  return (
    <Modal
      title="Profiling"
      wide
      onClose={onClose}
      footer={
        <>
          <button type="button" onClick={() => void onSetEnabled(!enabled)}>
            <Activity size={15} />
            {enabled ? 'Stop Profiling' : 'Start Profiling'}
          </button>
          <button type="button" disabled={!enabled} onClick={onFlush}>
            <RefreshCw size={15} />
            Flush Now
          </button>
          <button type="button" onClick={onClose}>
            Done
          </button>
        </>
      }
    >
      <div className="profiling-overview">
        <section className="provider-card profiling-card">
          <div className="provider-heading">
            <div className="status-icon">
              <Activity size={18} />
            </div>
            <div>
              <h4>{enabled ? 'Profiling enabled' : 'Profiling disabled'}</h4>
              <p>{enabled ? 'Renderer reports plus main IPC and OpenAI stream timings are being written as JSONL.' : 'Structured profiling is disabled.'}</p>
            </div>
          </div>
          <div className="provider-grid profiling-grid">
            <div>
              <span>Reports</span>
              <strong>{state?.reportCount ?? 0}</strong>
            </div>
            <div>
              <span>Started</span>
              <strong>{formatDateTime(state?.startedAt)}</strong>
            </div>
            <div>
              <span>Last report</span>
              <strong>{formatDateTime(state?.lastReportAt)}</strong>
            </div>
            <div>
              <span>Last flush</span>
              <strong>{formatDateTime(report?.generatedAt)}</strong>
            </div>
          </div>
          <div className="profiling-path-row">
            <FileJson size={15} />
            <code>{state?.outputPath ?? 'No profiling file yet'}</code>
          </div>
        </section>

        <section className="profiling-table-card profiling-findings-card">
          <h4>Likely Lag Causes</h4>
          {findings.length > 0 ? (
            <div className="profiling-findings-list">
              {findings.map((finding) => (
                <article className={`profiling-finding ${finding.severity}`} key={finding.id}>
                  <strong>{finding.title}</strong>
                  <p>{finding.evidence}</p>
                  <small>{finding.recommendation}</small>
                </article>
              ))}
            </div>
          ) : (
            <p>No lag threshold crossings have been captured in this report.</p>
          )}
        </section>

        <section className="profiling-columns">
          <div className="profiling-table-card">
            <h4>Slowest Timings</h4>
            {topTimings.length > 0 ? (
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Avg</th>
                    <th>Max</th>
                    <th>Count</th>
                  </tr>
                </thead>
                <tbody>
                  {topTimings.map((row) => (
                    <tr key={row.name}>
                      <td>{row.name}</td>
                      <td>{row.avgMs}ms</td>
                      <td>{row.maxMs}ms</td>
                      <td>{row.count}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p>No timing report has been flushed yet.</p>
            )}
          </div>
          <div className="profiling-table-card">
            <h4>Render Counts</h4>
            {topRenders.length > 0 ? (
              <table>
                <thead>
                  <tr>
                    <th>Surface</th>
                    <th>Renders</th>
                    <th>Last</th>
                  </tr>
                </thead>
                <tbody>
                  {topRenders.map((row) => (
                    <tr key={row.surface}>
                      <td>{row.surface}</td>
                      <td>{row.renders}</td>
                      <td>{row.lastRender}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p>No render report has been flushed yet.</p>
            )}
          </div>
          <div className="profiling-table-card">
            <h4>Worst Correlated Samples</h4>
            {worstSamples.length > 0 ? (
              <table>
                <thead>
                  <tr>
                    <th>Name</th>
                    <th>Duration</th>
                    <th>Time</th>
                  </tr>
                </thead>
                <tbody>
                  {worstSamples.map((sample, index) => (
                    <tr key={`${sample.at}:${sample.name}:${index}`}>
                      <td>{sample.name}</td>
                      <td>{sample.durationMs}ms</td>
                      <td>{formatTime(sample.at)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p>No correlated timing samples have been captured yet.</p>
            )}
          </div>
        </section>
      </div>
    </Modal>
  );
}

function formatTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '-' : date.toLocaleTimeString([], { minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 });
}

function formatDateTime(value: string | null | undefined): string {
  if (!value) return '-';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '-';
  return date.toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  });
}
